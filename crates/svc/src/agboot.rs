// aginx-boot-ok — A/B slot attribute maintenance in the GPT (M16/M14;
// N4③b 原名 agboot-ok，rcS 调).
//
// On redfin the slot state is NOT a bootloader_control block in misc —
// misc's vendor space holds recovery's "theme-dark" string and nothing
// else (probed 2026-08-31). The store is the GPT itself: the partition
// entry attribute u64 of every *_a / *_b entry, replicated across the
// per-LUN GPTs of /dev/sda../sdf (sdb/sdc carry the xbl chains).
//
// The attribute layout is Qualcomm's uefi.lnx.3.0 one (ABL r3-0.6 on
// this device), NOT the AOSP boot_control layout. Verified on device
// 2026-09-02 against QRD ABL r12 source (PartitionTableUpdate.c) and
// the observed attr transitions of live slot switches and rollbacks:
//
//   bits 48-49  priority        (max 3, MAX_PRIORITY)
//   bit  50     ACTIVE          — the slot selector's gate: GetActiveSlot()
//                                 only considers entries with this bit;
//                                 priority alone selects nothing
//   bits 51-53  tries remaining (max 7; drained one per unmarked boot)
//   bit  54     successful boot
//   bit  55     unbootable
//   bits 56-63  unused by ABL (60 is the GPT-spec readonly bit)
//
// A full switch in newer ABLs additionally swaps the _a/_b type GUIDs
// and flips the UFS boot LUN (SwitchPtnSlots/ValidateSlotGuids in the
// r12 source). This device's older ABL does not gate on either — proven
// 2026-09-02: attrs-only `set-active` switched the OS slot a→b and b→a
// with full bringup, MarkPtnActive flipping ACTIVE on every LUN, and
// ABL's own rollback (tries=0 → unbootable → alternate → cold reboot)
// running to completion. So userspace owns the whole switch here; we
// leave the GUID swap to ABL's rollback path, which performs it itself
// when it needs to.
//
// Modes:
//   aginx-boot-ok              mark the ACTIVE slot successful (rcS, after
//                             a `done ok` boot): succ+tries+ACTIVE on its
//                             boot entry, ACTIVE on its other entries —
//                             this is what stops ABL's per-boot tries drain
//   aginx-boot-ok status       dump the whole slot table, read-only
//   aginx-boot-ok set-active X switch the boot target to slot X: per ABL
//                             SetActiveSlot — boot entry of X gets pri 3 +
//                             ACTIVE + tries 7 (succ cleared, unbootable
//                             cleared), X's other entries get ACTIVE, the
//                             other boot entry loses ACTIVE and drops to
//                             pri 2. Takes effect on the next reboot; the
//                             drain then gives 7 unmarked boots before ABL
//                             rolls back to the other slot on its own.
use std::fs::{File, OpenOptions};
use std::os::unix::io::AsRawFd;

const PRI_MAX: u64 = 3;
const PRI_DEMOTE: u64 = 2; // MAX_PRIORITY - 1, per ABL SetActiveSlot
const TRIES: u64 = 7;

const ATTR_PRIORITY: u64 = 0x3 << 48;
const ATTR_ACTIVE: u64 = 1 << 50;
const ATTR_TRIES: u64 = 0x7 << 51;
const ATTR_SUCCESS: u64 = 1 << 54;
const ATTR_UNBOOTABLE: u64 = 1 << 55;

struct Gpt {
    dev: String,
    /// Logical block size — the redfin UFS LUNs are 4K-block, so the GPT
    /// header sits at byte 4096, not 512. Read per disk, never assumed.
    lbs: u64,
    hdr: Vec<u8>,
    entries: Vec<u8>,
    entry_lba: u64,
    num: usize,
    esz: usize,
    /// Backup GPT: its entries LBA and header LBA.
    bak_entries_lba: u64,
    bak_hdr_lba: u64,
    bak_hdr: Vec<u8>,
}

fn rd(f: &File, buf: &mut [u8], off: u64) -> bool {
    let n = unsafe { libc::pread(f.as_raw_fd(), buf.as_mut_ptr() as *mut _, buf.len() as _, off as libc::off_t) };
    n == buf.len() as isize
}

fn wr(f: &File, buf: &[u8], off: u64) -> bool {
    let n = unsafe { libc::pwrite(f.as_raw_fd(), buf.as_ptr() as *const _, buf.len() as _, off as libc::off_t) };
    n == buf.len() as isize
}

fn u32at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn u64at(b: &[u8], o: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(v)
}

fn put32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

impl Gpt {
    fn open(dev: &str) -> Option<Gpt> {
        let f = File::open(dev).ok()?;
        let lbs = std::fs::read_to_string(format!(
            "/sys/class/block/{}/queue/logical_block_size",
            dev.trim_start_matches("/dev/")
        ))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&b| b >= 512)
        .unwrap_or(512);
        let mut hdr = vec![0u8; lbs as usize];
        if !rd(&f, &mut hdr, lbs) {
            if dbg() { eprintln!("dbg: {dev}: header read failed"); }
            return None; // not a GPT disk (or not a disk at all)
        }
        if &hdr[0..8] != b"EFI PART" {
            if dbg() { eprintln!("dbg: {dev}: bad sig {:?}", &hdr[0..8]); }
            return None;
        }
        let num = u32at(&hdr, 80) as usize;
        let esz = u32at(&hdr, 84) as usize;
        if num == 0 || esz == 0 || num * esz > 1 << 20 {
            return None;
        }
        let entry_lba = u64at(&hdr, 72);
        let mut entries = vec![0u8; num * esz];
        if !rd(&f, &mut entries, entry_lba * lbs) {
            if dbg() { eprintln!("dbg: {dev}: entries read failed at lba {entry_lba}"); }
            return None;
        }
        if u32at(&hdr, 88) != crc32(&entries) {
            eprintln!("aginx-boot-ok: {dev}: primary entries crc mismatch — skipping");
            return None;
        }
        let bak_hdr_lba = u64at(&hdr, 32);
        let mut bak_hdr = vec![0u8; lbs as usize];
        if !rd(&f, &mut bak_hdr, bak_hdr_lba * lbs) || &bak_hdr[0..8] != b"EFI PART" {
            if dbg() { eprintln!("dbg: {dev}: backup header unreadable at lba {bak_hdr_lba}"); }
            return None; // truncated tail read; skip disk rather than guess
        }
        let bak_entries_lba = u64at(&bak_hdr, 72);
        Some(Gpt { dev: dev.to_string(), lbs, hdr, entries, entry_lba, num, esz, bak_entries_lba, bak_hdr_lba, bak_hdr })
    }

    /// (name, attrs, entry offset) for every non-empty entry.
    fn entries(&self) -> Vec<(String, u64, usize)> {
        let mut out = Vec::new();
        for i in 0..self.num {
            let e = i * self.esz;
            let raw = &self.entries[e + 56..e + 128];
            if raw.iter().all(|&b| b == 0) {
                continue;
            }
            let name: String = raw
                .chunks_exact(2)
                .take_while(|c| c != &[0, 0])
                .map(|c| u16::from_le_bytes([c[0], c[1]]) as u8 as char)
                .collect();
            out.push((name, u64at(&self.entries, e + 48), e));
        }
        out
    }

    fn set_attrs(&mut self, off: usize, attrs: u64) {
        self.entries[off + 48..off + 56].copy_from_slice(&attrs.to_le_bytes());
    }

    /// Write entries + both CRC fields back to primary and backup GPT.
    fn commit(&mut self, f: &File) -> Result<(), String> {
        let ecrc = crc32(&self.entries);
        let lbs = self.lbs;
        for (hdr, lba) in [(self.hdr.as_mut_slice(), 1u64), (self.bak_hdr.as_mut_slice(), self.bak_hdr_lba)] {
            put32(hdr, 88, ecrc);
            put32(hdr, 16, 0);
            let hcrc = crc32(&hdr[..92]);
            put32(hdr, 16, hcrc);
            if !wr(f, hdr, lba * lbs) {
                return Err(format!("header write at lba {lba}"));
            }
        }
        for (buf, lba) in [(self.entries.as_slice(), self.entry_lba), (self.entries.as_slice(), self.bak_entries_lba)] {
            if !wr(f, buf, lba * lbs) {
                return Err(format!("entries write at lba {lba}"));
            }
        }
        unsafe { libc::fsync(f.as_raw_fd()) };
        Ok(())
    }
}

fn dbg() -> bool {
    std::env::var("AGBOOT_DEBUG").is_ok()
}

/// Every LUN holding an A/B GPT. Names, not /dev/sdX guesses: resolve
/// through /dev/block/by-name so we only touch disks we can see.
fn ab_disks() -> Vec<String> {
    let mut disks: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/dev/block/by-name") {
        for e in rd.filter_map(|e| e.ok()) {
            if let Ok(t) = std::fs::read_link(e.path()) {
                if let Some(d) = t.to_str().and_then(|s| s.strip_prefix("/dev/")) {
                    let d = d.trim_start_matches("sd").trim_end_matches(|c: char| c.is_ascii_digit());
                    let dev = format!("/dev/sd{d}");
                    if !disks.contains(&dev) {
                        disks.push(dev);
                    }
                }
            }
        }
    }
    disks.sort();
    disks
}

/// Active slot from the kernel cmdline — `_a`/`_b`.
fn slot_suffix() -> Option<String> {
    std::fs::read_to_string("/proc/cmdline").ok().and_then(|c| {
        c.split_whitespace().find_map(|t| {
            let v = t.strip_prefix("androidboot.slot_suffix=")?;
            let v = v.trim_matches('"');
            (v == "_a" || v == "_b").then(|| v.to_string())
        })
    })
}

fn print_table(g: &Gpt) {
    for (n, a, _) in g.entries() {
        if n.ends_with("_a") || n.ends_with("_b") {
            println!(
                "{:14} {:18} pri={} act={} tries={} succ={} unboot={} raw={:016x}",
                g.dev,
                n,
                (a >> 48) & 0x3,
                (a >> 50) & 1,
                (a >> 51) & 0x7,
                (a >> 54) & 1,
                (a >> 55) & 1,
                a
            );
        }
    }
}

/// Switch the boot target to slot `tgt` — ABL SetActiveSlot's attribute
/// rewrite, which on this device is the whole switch (see header). Takes
/// effect at the next reboot; give the new slot a `done ok` boot within
/// `tries` boots or ABL rolls back to the other slot by itself.
fn set_active(tgt: &str, other: &str, tries: u64) -> i32 {
    let mut staged = 0usize;
    for dev in ab_disks() {
        let mut g = match Gpt::open(&dev) {
            Some(g) => g,
            None => continue,
        };
        let ents = g.entries();
        let mut changed = false;

        // SetActiveSlot: target boot entry gets pri=MAX | ACTIVE | tries
        // (clears unbootable and success); every other slot's boot entry
        // loses ACTIVE and is demoted to pri = MAX-1.
        for (n, a, off) in &ents {
            let mut na = *a;
            if n == &format!("boot{tgt}") {
                na = (na & !ATTR_PRIORITY) | (PRI_MAX << 48);
                na |= ATTR_ACTIVE;
                na = (na & !ATTR_TRIES) | (tries << 51);
                na &= !(ATTR_SUCCESS | ATTR_UNBOOTABLE);
            } else if n == &format!("boot{other}") {
                na &= !ATTR_ACTIVE;
                na = (na & !ATTR_PRIORITY) | (PRI_DEMOTE << 48);
            } else if n.ends_with(tgt) {
                // MarkPtnActive: ACTIVE rides on every entry of the target
                na |= ATTR_ACTIVE;
            } else if n.ends_with(other) {
                na &= !ATTR_ACTIVE;
            }
            if na != *a {
                if dbg() {
                    println!("stage {dev} {n}: {a:016x} → {na:016x}");
                }
                g.set_attrs(*off, na);
                changed = true;
            }
        }

        if !changed {
            continue;
        }
        let f = OpenOptions::new().write(true).open(&dev).unwrap_or_else(|e| {
            eprintln!("aginx-boot-ok: open {dev} rw: {e}");
            std::process::exit(1);
        });
        match g.commit(&f) {
            Ok(()) => staged += 1,
            Err(e) => {
                eprintln!("aginx-boot-ok: {dev}: {e} — NOT committed");
                return 1;
            }
        }
    }
    if staged == 0 {
        eprintln!("aginx-boot-ok: no *{tgt} entries found — nothing staged");
        return 1;
    }
    println!(
        "aginx-boot-ok: slot {tgt} set active on {staged} disks — reboots into it; {} unmarked boot{} before ABL auto-rolls-back",
        tries,
        if tries == 1 { "" } else { "s" }
    );
    0
}

/// Mark the active slot successful — the userspace half of the A/B
/// contract (stock Android's bootctl does this from userspace too; ABL
/// never sets the success bit itself). Boot entry of the running
/// suffix: succ + fresh tries + ACTIVE, clear unbootable. Other entries
/// of the suffix: ACTIVE only (MarkPtnActive keeps that every boot
/// anyway; ABL reads tries/succ from the boot entry alone).
fn mark_success(suffix: &str) -> i32 {
    let mut marked = 0usize;
    for dev in ab_disks() {
        let mut g = match Gpt::open(&dev) {
            Some(g) => g,
            None => continue,
        };
        let targets: Vec<(String, u64, usize)> = g
            .entries()
            .into_iter()
            .filter(|(n, _, _)| n.ends_with(suffix))
            .collect();
        if targets.is_empty() {
            continue;
        }
        let f = OpenOptions::new().write(true).open(&dev).unwrap_or_else(|e| {
            eprintln!("aginx-boot-ok: open {dev} rw: {e}");
            std::process::exit(1);
        });
        let boot_name = format!("boot{suffix}");
        for (n, a, off) in &targets {
            let na = if *n == boot_name {
                ((a & !ATTR_TRIES) & !ATTR_UNBOOTABLE) | (TRIES << 51) | ATTR_SUCCESS | ATTR_ACTIVE
            } else {
                *a | ATTR_ACTIVE
            };
            if na == *a {
                continue;
            }
            g.set_attrs(*off, na);
            println!("aginx-boot-ok: {dev} {n}: {a:016x} → {na:016x}");
            marked += 1;
        }
        if let Err(e) = g.commit(&f) {
            eprintln!("aginx-boot-ok: {dev}: {e} — NOT committed");
        }
    }
    if marked == 0 {
        eprintln!("aginx-boot-ok: slot {suffix} already marked, nothing to do");
    } else {
        println!("aginx-boot-ok: slot {suffix} marked successful on {marked} entries");
    }
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str);

    if mode == Some("set-active") {
        // `aginx-boot-ok set-active <a|b> [tries]` — stage the given slot for
        // the next boot (M14). See set_active() for exactly what this
        // covers and what it deliberately cannot (the UFS boot LUN).
        let tgt = match args.get(1).map(String::as_str) {
            Some("a") => "_a",
            Some("b") => "_b",
            _ => {
                eprintln!("usage: aginx-boot-ok set-active <a|b> [tries]");
                std::process::exit(2);
            }
        };
        let other = if tgt == "_a" { "_b" } else { "_a" };
        let tries: u64 = args.get(2).and_then(|t| t.parse().ok()).unwrap_or(TRIES).min(7);
        std::process::exit(set_active(tgt, other, tries));
    }
    let status = mode == Some("status");

    let suffix = slot_suffix().unwrap_or_else(|| {
        eprintln!("aginx-boot-ok: no androidboot.slot_suffix on cmdline, assuming _a");
        "_a".to_string()
    });
    if dbg() {
        eprintln!("dbg: suffix={suffix} disks={:?}", ab_disks());
    }

    if status {
        for dev in ab_disks() {
            if let Some(g) = Gpt::open(&dev) {
                print_table(&g);
            }
        }
        return;
    }
    std::process::exit(mark_success(&suffix));
}

/// Standard CRC-32 (reflected 0xEDB88320, init/xorout 0xFFFFFFFF) — the
/// same zlib crc32 the GPT spec and gpt_utils use.
fn crc32(buf: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut crc = 0xFFFF_FFFFu32;
    for &b in buf {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}
