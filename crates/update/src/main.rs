//! aginx-update — A/B self-updater (M14; N5① 吸收并修三条死路径).
//!
//! Flow: pull manifest → download images → verify sha256 → write the
//! INACTIVE slot's partitions → `aginx-boot-ok set-active` (GPT attrs) →
//! reboot. Fully autonomous: on this ABL (r3-0.6) the GPT attributes
//! alone select the OS slot — proven on device 2026-09-02 with
//! attrs-only switches in both directions, no bootloader involvement.
//!
//! Rollback (all observed on device 2026-09-02): the staged slot boots
//! with succ=0/tries=7; ABL drains one try per boot that rcS does not
//! mark successful; at tries=0 ABL marks the slot unbootable,
//! re-activates the still-successful other slot, and cold-reboots into
//! it by itself. Failure classes that do NOT auto-rollback and need
//! host rescue: a zeroed/garbled boot header (ABL drops to fastboot),
//! and a kernel that hangs early (device sits dark — no watchdog yet;
//! a forced power-cycle drains one more try). sha256 verification at
//! apply time is what keeps those classes from ever being written.
//!
//! The GPT surgery deliberately lives in aginx-svc's aginx-boot-ok —
//! one audited implementation of the 4K-block multi-LUN attribute
//! rewrite. This crate only orchestrates and streams bytes.
//!
//! Ordering rule (why this is crash-safe mid-update): partition bytes are
//! written and fsynced FIRST; the GPT attr flip is the last write before
//! reboot. A crash at any earlier point leaves both slots exactly as they
//! were. A crash after the flip but before reboot is harmless — the next
//! boot just takes the new slot early.
//!
//! Manifest (JSON, local path or https URL; image urls likewise):
//!   { "version": "…",
//!     "boot":          { "url": "…", "sha256": "hex", "size": N },
//!     "vendor_boot":   { … },   // optional, only when modules move
//!     "dtbo":          { … },   // optional
//!     "vbmeta":        { … },   // optional — must chain with the images
//!     "vbmeta_system": { … } }  // optional
//!
//! M21: every manifest must have a detached ed25519 signature — base64
//! in `<manifest>.sig` next to it (local sibling file, or URL + ".sig")
//! — verified against the public key compiled in below BEFORE the
//! manifest is parsed or a single image byte is fetched. Sign with the
//! host tool: `aginx-sign sign .local/keys/aginx.key manifest.json`.
//! The private key never leaves the developer machine.
//!
//! HTTPS goes through aginx-download (M10) — the phone's only TLS
//! fetcher.
//!
//! N5① note: the frozen first-gen binary spawned `/usr/bin/agdl`,
//! `/usr/bin/agboot-ok` and `/bin/reboot2` — all renamed by the D13
//! sweep, so on the N4 image apply died at the set-active step. The
//! three spawn paths are now consts below, pinned by test.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::io::AsRawFd;
use std::process::Command;

use serde::Deserialize;

#[derive(Deserialize)]
struct Image {
    url: String,
    sha256: String,
    size: Option<u64>,
    /// M22: body already streamed to the swap area on userdata (by the
    /// host over adb — a sparse 1-2 GiB mke2fs image cannot ride the
    /// small live fs). aginx-update then hashes the staged blocks
    /// against this manifest's sha256 and writes the commit header.
    /// Production https updates (pre_staged absent) download through
    /// aginx-download as usual — which will need direct-to-offset
    /// streaming once images approach the fs size (download seek;
    /// follow-up).
    #[serde(default)]
    pre_staged: bool,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    boot: Image,
    vendor_boot: Option<Image>,
    dtbo: Option<Image>,
    vbmeta: Option<Image>,
    vbmeta_system: Option<Image>,
    /// M22: new rootfs image for the userdata partition. Carried through
    /// the signed manifest like everything else; the swap itself happens
    /// in the initramfs trampoline on next boot (see SWAP_OFF below).
    rootfs: Option<Image>,
}

// --- N5① D13 device paths (spawn targets; pinned by test) ---
const DOWNLOAD_BIN: &str = "/usr/bin/aginx-download";
const BOOT_OK_BIN: &str = "/usr/bin/aginx-boot-ok";
const REBOOT_BIN: &str = "/usr/bin/aginx-reboot";

// --- M22 rootfs-swap protocol (MUST match crates/aginxos-init) ---
//
// The live rootfs IS the userdata partition, so it cannot be rewritten
// from userspace. Instead aginx-update stages the new image ON the same
// partition, beyond the ext4 (fs is 2 GiB; if it is ever grown past
// ~7 GiB these offsets must move):
//   8 GiB         SWAP_OFF:  4096-byte header (the commit point)
//   8 GiB + 4096  SWAP_BODY: new rootfs image, payload_len bytes
//   32 GiB        BAK_OFF:   trampoline's copy of the old fs
// Header layout: magic[8]="AGXROOT1", u32le version=1, u32le flags
// (1=pending), u64le payload_len, sha256 as 64 lowercase hex chars,
// u64le old_len (bytes of the current fs, for the trampoline's
// backup). Write order is crash-safe: body first + fsync, header LAST
// — a crash before the header leaves the old rootfs untouched and the
// stray body harmless. The trampoline (new vendor_boot, A/B-protected
// by M14) sees the marker, verifies the body hash, backs up the old
// fs to 32 GiB, dd's the new image to offset 0, clears the marker,
// and only then mounts. That is why a rootfs update MUST ship with
// the matching vendor_boot: an old trampoline ignores the marker.
const SWAP_OFF: u64 = 8 << 30;
const SWAP_HDR: u64 = 4096;
const BAK_OFF: u64 = 32 << 30;
const SWAP_MAGIC: &[u8; 8] = b"AGXROOT1";
// Irreplaceable device state (Wi-Fi psk, relay identity in /home/.aginx,
// logs) staged at 64 GiB with its own ASCII marker — MUST match
// /etc/init.d/state-restore. M26 added /var/lib: aginx-pkg
// skills/units/stamps and the aginx-done markers live there and are NOT
// re-downloadable (a rootfs swap without them would silently wipe every
// package's skill + provision memory). Re-downloadable things (/var/bin
// binaries) deliberately still do NOT ride along: the new rootfs
// re-provisions them.
const STATE_OFF: u64 = 64 << 30;
const STATE_MAGIC: &[u8; 8] = b"AGXSTATE";
const STATE_MAX: u64 = 512 << 20;

fn swap_header(payload_len: u64, sha256_hex: &str, old_len: u64) -> Vec<u8> {
    let mut h = vec![0u8; SWAP_HDR as usize];
    h[..8].copy_from_slice(SWAP_MAGIC);
    h[8..12].copy_from_slice(&1u32.to_le_bytes()); // version
    h[12..16].copy_from_slice(&1u32.to_le_bytes()); // flags: pending
    h[16..24].copy_from_slice(&payload_len.to_le_bytes());
    h[24..88].copy_from_slice(sha256_hex.as_bytes());
    h[88..96].copy_from_slice(&old_len.to_le_bytes());
    h
}

/// Stream a file to `off` on an already-open block device, fsync, return
/// bytes written.
fn pwrite_file_at(fd: i32, staged: &str, off: u64) -> u64 {
    let mut src = File::open(staged).unwrap_or_else(|e| die(&format!("open {staged}: {e}")));
    let len = src.metadata().unwrap_or_else(|e| die(&format!("stat {staged}: {e}"))).len();
    let mut buf = vec![0u8; 4 << 20];
    let mut off = off;
    let mut left = len;
    while left > 0 {
        let n = src.read(&mut buf).unwrap_or_else(|e| die(&format!("read {staged}: {e}")));
        if n == 0 { die(&format!("{staged}: short read at {}", len - left)); }
        let mut done = 0usize;
        while done < n {
            let w = unsafe {
                libc::pwrite(fd, buf[done..n].as_ptr() as *const _, (n - done) as _, off as libc::off_t)
            };
            if w <= 0 { die(&format!("pwrite at {off}: {}", std::io::Error::last_os_error())); }
            done += w as usize;
            off += w as u64;
        }
        left -= n as u64;
    }
    len
}

/// Capture the irreplaceable set as a tar (busybox, absolute paths) and
/// stage it at STATE_OFF with an ASCII header: magic(8) + 16 decimal
/// digits of length + newline — parseable by /etc/init.d/state-restore
/// with nothing but dd/head/cut. Marker last = crash-safe.
///
/// The tar builds on the REAL fs (/var/tmp), never on the manifest's
/// staging dir: that sits on tmpfs, and on 2026-09-03 tar died mid-file
/// there (tmpfs/memory pressure, killed at 235491328 bytes into
/// /root/bin/codex) — a partial tar the swap boot then extracted,
/// leaving a truncated codex. tar's exit status is now fatal: a killed
/// or ENOSPC'd capture must refuse the update, not ship half a state.
fn stage_state_tar() {
    let _ = std::fs::create_dir_all("/var/tmp");
    let tar_path = "/var/tmp/aginx-update-state.tar";
    let _ = std::fs::remove_file(tar_path);
    // best-effort: an agent mid-write means one file is torn, not lost
    let st = Command::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "tar -cf {tar_path} /etc/wifi.conf /etc/aginx /home /root /var/log /var/power /var/lib 2>/dev/null"
        ))
        .status()
        .unwrap_or_else(|e| die(&format!("spawn tar: {e}")));
    if !st.success() {
        die(&format!(
            "state capture tar exited {:?} — refusing to update (would ship a torn state)",
            st.code()
        ));
    }
    let len = match std::fs::metadata(&tar_path) {
        Ok(m) => m.len(),
        Err(_) => die("state capture produced no tar — refusing to update (would be a factory reset)"),
    };
    if len == 0 || len > STATE_MAX {
        die(&format!("state tar is {len} bytes (max {STATE_MAX}) — refusing"));
    }
    let dev = "/dev/block/by-name/userdata";
    let dst = OpenOptions::new().write(true).open(dev)
        .unwrap_or_else(|e| die(&format!("open {dev} rw: {e}")));
    let fd = dst.as_raw_fd();
    let wrote = pwrite_file_at(fd, &tar_path, STATE_OFF + SWAP_HDR);
    if wrote != len {
        die(&format!("state tar wrote {wrote} != {len}"));
    }
    unsafe { libc::fsync(fd) };
    let mut h = vec![0u8; SWAP_HDR as usize];
    h[..8].copy_from_slice(STATE_MAGIC);
    h[8..24].copy_from_slice(format!("{len:016}").as_bytes());
    h[24] = b'\n';
    let mut done = 0usize;
    while done < h.len() {
        let w = unsafe {
            libc::pwrite(fd, h[done..].as_ptr() as *const _, (h.len() - done) as _, STATE_OFF as libc::off_t)
        };
        if w <= 0 { die(&format!("pwrite state header: {}", std::io::Error::last_os_error())); }
        done += w as usize;
    }
    unsafe { libc::fsync(fd) };
    println!("aginx-update: state tar staged at {STATE_OFF} ({len} bytes)");
    let _ = std::fs::remove_file(&tar_path);
}

/// Verify the pre-staged body ON the block device (hash over
/// SWAP_OFF+SWAP_HDR .. len) against the signed manifest, then write the
/// commit header. The signature is the trust anchor: the host streamed
/// the bytes, the manifest says what they must hash to.
fn commit_rootfs_swap(img: &Image) {
    let dev = "/dev/block/by-name/userdata";
    // read+write: the pre-staged body is verified by pread ON the blkdev —
    // a write-only fd fails that pread with EBADF (observed 2026-09-02).
    let dst = OpenOptions::new().read(true).write(true).open(dev)
        .unwrap_or_else(|e| die(&format!("open {dev} rw: {e}")));
    let fd = dst.as_raw_fd();
    let len = match std::fs::metadata(&img.url) {
        // a local image tells us its length directly
        Ok(m) => m.len(),
        Err(_) => img.size.unwrap_or_else(|| die("pre-staged rootfs needs a local url or size")),
    };
    if len == 0 || len % 4096 != 0 {
        die(&format!("pre-staged rootfs len {len} — want non-zero 4K-aligned"));
    }
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    let mut done: u64 = 0;
    while done < len {
        let want = ((len - done) as usize).min(buf.len());
        let n = unsafe {
            libc::pread(fd, buf[..want].as_mut_ptr() as *mut _, want as _, (SWAP_OFF + SWAP_HDR + done) as libc::off_t)
        };
        if n <= 0 {
            die(&format!("read staged body at {done}: {}", std::io::Error::last_os_error()));
        }
        h.update(&buf[..n as usize]);
        done += n as u64;
    }
    let got = hex(&h.finish());
    if !got.eq_ignore_ascii_case(&img.sha256) {
        die(&format!("pre-staged rootfs body sha256 {got} != manifest {} — re-stream it", img.sha256));
    }
    println!("aginx-update: pre-staged rootfs body verified ({len} bytes)");

    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    let cst = std::ffi::CString::new("/").unwrap();
    if unsafe { libc::statfs(cst.as_ptr(), &mut st) } != 0 {
        die(&format!("statfs /: {}", std::io::Error::last_os_error()));
    }
    let old_len = st.f_blocks as u64 * st.f_bsize as u64;
    if old_len + (4 << 20) >= SWAP_OFF {
        die(&format!("rootfs fs is {old_len} bytes — grown into the {SWAP_OFF} swap area, refusing"));
    }
    let hdr = swap_header(len, &got, old_len);
    let mut w = 0usize;
    while w < hdr.len() {
        let k = unsafe {
            libc::pwrite(fd, hdr[w..].as_ptr() as *const _, (hdr.len() - w) as _, SWAP_OFF as libc::off_t)
        };
        if k <= 0 { die(&format!("pwrite swap header: {}", std::io::Error::last_os_error())); }
        w += k as usize;
    }
    unsafe { libc::fsync(fd) };
    println!("aginx-update: rootfs swap committed at {SWAP_OFF} (len {len}, old fs {old_len})");
}

/// Stage the verified rootfs image into the swap area on the userdata
/// partition. Nothing at offset 0 is touched — the running fs stays
/// valid no matter when we crash.
fn stage_rootfs_swap(staged: &str, sha256_hex: &str) -> u64 {
    let dev = "/dev/block/by-name/userdata";
    let dst = OpenOptions::new().write(true).open(dev)
        .unwrap_or_else(|e| die(&format!("open {dev} rw: {e}")));
    let src = File::open(staged).unwrap_or_else(|e| die(&format!("open {staged}: {e}")));
    let len = src.metadata().unwrap_or_else(|e| die(&format!("stat {staged}: {e}"))).len();
    if len == 0 || len % 4096 != 0 {
        die(&format!("{staged}: len {len} — want a non-zero 4K-aligned image"));
    }
    // old fs extent (for the trampoline's backup): the mounted rootfs
    // knows its own size.
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    let cst = std::ffi::CString::new("/").unwrap();
    if unsafe { libc::statfs(cst.as_ptr(), &mut st) } != 0 {
        die(&format!("statfs /: {}", std::io::Error::last_os_error()));
    }
    let old_len = st.f_blocks as u64 * st.f_bsize as u64;
    if old_len + (4 << 20) >= SWAP_OFF {
        die(&format!("rootfs fs is {old_len} bytes — grown into the {SWAP_OFF} swap area, refusing"));
    }

    let fd = dst.as_raw_fd();
    let wrote = pwrite_file_at(fd, staged, SWAP_OFF + SWAP_HDR);
    if wrote != len {
        die(&format!("rootfs image wrote {wrote} != {len}"));
    }
    unsafe { libc::fsync(fd) };
    // commit point: the header, last
    let h = swap_header(len, sha256_hex, old_len);
    let mut done = 0usize;
    while done < h.len() {
        let w = unsafe {
            libc::pwrite(fd, h[done..].as_ptr() as *const _, (h.len() - done) as _, SWAP_OFF as libc::off_t)
        };
        if w <= 0 { die(&format!("pwrite swap header: {}", std::io::Error::last_os_error())); }
        done += w as usize;
    }
    unsafe { libc::fsync(fd) };
    println!("aginx-update: rootfs staged at {SWAP_OFF} on userdata (len {len}, old fs {old_len}, backup target {BAK_OFF})");
    len
}

fn die(msg: &str) -> ! {
    eprintln!("aginx-update: {msg}");
    std::process::exit(1);
}

/// Active slot from the kernel cmdline — `_a`/`_b`. Never guessed: an
/// updater that flashes the wrong slot is a brick.
fn active_slot() -> String {
    std::fs::read_to_string("/proc/cmdline")
        .ok()
        .and_then(|c| {
            c.split_whitespace().find_map(|t| {
                let v = t.strip_prefix("androidboot.slot_suffix=")?;
                let v = v.trim_matches('"');
                (v == "_a" || v == "_b").then(|| v.to_string())
            })
        })
        .unwrap_or_else(|| die("no androidboot.slot_suffix on cmdline — refusing to flash"))
}

fn other(slot: &str) -> String {
    if slot == "_a" { "_b".into() } else { "_a".into() }
}

/// Download `url` to `out` if remote (via aginx-download), or point at
/// it if local.
fn stage(url: &str, out: &str) -> String {
    if url.starts_with('/') {
        if !std::path::Path::new(url).is_file() {
            die(&format!("local source {url} missing"));
        }
        return url.to_string();
    }
    if !url.starts_with("https://") && !url.starts_with("http://") {
        die(&format!("unsupported url {url}"));
    }
    let st = Command::new(DOWNLOAD_BIN)
        .arg(url)
        .arg(out)
        .status()
        .unwrap_or_else(|e| die(&format!("cannot run aginx-download: {e}")));
    if !st.success() {
        die(&format!("aginx-download failed on {url}"));
    }
    out.to_string()
}

fn sha256_hex_file(path: &str) -> String {
    let mut f = File::open(path).unwrap_or_else(|e| die(&format!("open {path}: {e}")));
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let n = f.read(&mut buf).unwrap_or_else(|e| die(&format!("read {path}: {e}")));
        if n == 0 { break; }
        h.update(&buf[..n]);
    }
    hex(&h.finish())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Partition byte size via BLKGETSIZE64 — the image must fit, full stop.
fn part_size(dev: &str) -> u64 {
    let f = File::open(dev).unwrap_or_else(|e| die(&format!("open {dev}: {e}")));
    let mut sz: libc::c_ulonglong = 0;
    unsafe {
        // ioctl's request arg is c_int on linux but c_ulong on darwin —
        // `as _` keeps this host-testable; the bits are the same ioctl.
        if libc::ioctl(f.as_raw_fd(), 0x80081272u32 as _, &mut sz) != 0 {
            die(&format!("BLKGETSIZE64 on {dev}: {}", std::io::Error::last_os_error()));
        }
    }
    sz
}

/// Stream `img` onto raw partition `dev`, fsync at the end. Nothing is
/// read back — the sha256 was checked on the staging file.
fn write_part(img: &str, dev: &str) -> u64 {
    let ps = part_size(dev);
    let mut src = File::open(img).unwrap_or_else(|e| die(&format!("open {img}: {e}")));
    let meta = src.metadata().unwrap_or_else(|e| die(&format!("stat {img}: {e}")));
    if meta.len() > ps {
        die(&format!("{img} is {} bytes, {dev} only {ps}", meta.len()));
    }
    let dst = OpenOptions::new().write(true).open(dev)
        .unwrap_or_else(|e| die(&format!("open {dev} rw: {e} — is the slot mounted?")));
    let mut buf = vec![0u8; 4 << 20];
    let mut off: u64 = 0;
    loop {
        let n = src.read(&mut buf).unwrap_or_else(|e| die(&format!("read {img}: {e}")));
        if n == 0 { break; }
        let mut done = 0usize;
        while done < n {
            let w = unsafe {
                libc::pwrite(dst.as_raw_fd(), buf[done..n].as_ptr() as *const _, (n - done) as _, off as libc::off_t)
            };
            if w <= 0 {
                die(&format!("pwrite {dev} at {off}: {}", std::io::Error::last_os_error()));
            }
            done += w as usize;
            off += w as u64;
        }
    }
    unsafe { libc::fsync(dst.as_raw_fd()) };
    off
}

fn space_at(dir: &str) -> u64 {
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    let c = match std::ffi::CString::new(dir) { Ok(c) => c, Err(_) => return 0 };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return 0;
    }
    (st.f_bavail as i64).max(0) as u64 * st.f_bsize as u64
}

/// First usable staging dir — userdata (`/` on this rootfs) keeps large
/// downloads off tmpfs.
fn staging_root() -> String {
    for d in ["/data/update", "/update", "/tmp/aginx-update"] {
        let _ = std::fs::create_dir_all(d);
        if space_at(d) > 0 {
            return d.to_string();
        }
    }
    die("no usable staging directory")
}

/// M21 signature gate: the detached sig must verify over the RAW
/// manifest bytes (no canonicalization — the file is signed exactly as
/// fetched) against the compiled-in key. Called before parse, so an
/// unsigned or tampered manifest dies without a single download.
/// M26: the key + verify live in the agsign lib — one key, one chain,
/// shared with agpkg's package manifest gate. N5①: that lib is now
/// aginx-sign in this repo; same key pair, verified byte-identical.
fn verify_manifest_sig(body: &str, sig_b64: &str) {
    aginx_sign::verify(body.as_bytes(), sig_b64)
        .unwrap_or_else(|e| die(&format!("manifest signature INVALID — refusing this update ({e})")));
}

fn fetch_manifest(src: &str) -> Manifest {
    let (body, sig) = if src.starts_with('/') {
        let body = std::fs::read_to_string(src).unwrap_or_else(|e| die(&format!("read {src}: {e}")));
        let sig = std::fs::read_to_string(format!("{src}.sig"))
            .unwrap_or_else(|e| die(&format!("read {src}.sig: {e} — unsigned manifests are rejected (M21)")));
        (body, sig)
    } else {
        let staged = stage(src, &format!("{}/manifest.json", staging_root()));
        let body = std::fs::read_to_string(&staged).unwrap_or_else(|e| die(&format!("read {staged}: {e}")));
        let sig_staged = stage(&format!("{src}.sig"), &format!("{}/manifest.json.sig", staging_root()));
        let sig = std::fs::read_to_string(&sig_staged).unwrap_or_else(|e| die(&format!("read {sig_staged}: {e}")));
        (body, sig)
    };
    verify_manifest_sig(&body, &sig);
    serde_json::from_str(&body).unwrap_or_else(|e| die(&format!("manifest parse: {e}")))
}

fn verify(img: &Image, path: &str) {
    let meta = std::fs::metadata(path).unwrap_or_else(|e| die(&format!("stat {path}: {e}")));
    if let Some(sz) = img.size {
        if meta.len() != sz {
            die(&format!("{path}: size {} != manifest {}", meta.len(), sz));
        }
    }
    let got = sha256_hex_file(path);
    if !got.eq_ignore_ascii_case(&img.sha256) {
        die(&format!("{path}: sha256 {got} != manifest {}", img.sha256));
    }
}

fn current_version() -> String {
    std::fs::read_to_string("/etc/aginx-version").map(|s| s.trim().to_string()).unwrap_or_else(|_| "unknown".into())
}

fn cmd_apply(src: &str, no_reboot: bool) {
    let m = fetch_manifest(src);
    let act = active_slot();
    let tgt = other(&act);
    println!("aginx-update: running {} → applying {} to slot {tgt}", current_version(), m.version);

    let root = staging_root();
    let parts: Vec<(&str, &Image)> = vec![("boot", &m.boot)].into_iter()
        .chain(m.vendor_boot.iter().map(|i| ("vendor_boot", i)))
        .chain(m.dtbo.iter().map(|i| ("dtbo", i)))
        .chain(m.vbmeta.iter().map(|i| ("vbmeta", i)))
        .chain(m.vbmeta_system.iter().map(|i| ("vbmeta_system", i)))
        .collect();
    for (name, img) in &parts {
        let staged = format!("{root}/{name}.img");
        let staged = stage(&img.url, &staged);
        verify(img, &staged);
        let dev = format!("/dev/block/by-name/{name}{tgt}");
        let n = write_part(&staged, &dev);
        println!("aginx-update: {name}: {n} bytes → {dev} (sha256 ok)");
        if staged != img.url {
            let _ = std::fs::remove_file(&staged);
        }
    }
    // M22: stage the new rootfs into the userdata swap area (after all
    // partition writes — the marker must never reference an update whose
    // kernel half failed to land). Rootfs updates require the matching
    // vendor_boot: only that trampoline knows how to perform the swap.
    // State tar goes first: whenever the swap marker exists, the captured
    // state must already be complete.
    if let Some(img) = &m.rootfs {
        if m.vendor_boot.is_none() {
            die("manifest has rootfs but no vendor_boot — the swap trampoline must ship with it");
        }
        stage_state_tar();
        if img.pre_staged {
            commit_rootfs_swap(img);
        } else {
            let staged = format!("{root}/rootfs.img");
            let staged = stage(&img.url, &staged);
            verify(img, &staged);
            stage_rootfs_swap(&staged, &img.sha256);
            if staged != img.url {
                let _ = std::fs::remove_file(&staged);
            }
        }
    }
    let st = Command::new(BOOT_OK_BIN)
        .arg("set-active").arg(tgt.trim_start_matches('_'))
        .status()
        .unwrap_or_else(|e| die(&format!("run aginx-boot-ok: {e}")));
    if !st.success() {
        die("aginx-boot-ok set-active failed — slots untouched, safe to retry");
    }
    println!("aginx-update: slot {tgt} staged — rebooting into update (auto-rollback after 7 unmarked boots)");
    if no_reboot {
        println!("aginx-update: --no-reboot given, not rebooting");
        return;
    }
    Command::new(REBOOT_BIN).arg("reboot").status()
        .unwrap_or_else(|e| die(&format!("aginx-reboot: {e}")));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("status") => {
            println!("slot {} version {}", active_slot(), current_version());
            let _ = Command::new(BOOT_OK_BIN).arg("status").status();
        }
        Some("apply") => {
            let src = args.get(1).unwrap_or_else(|| die("usage: aginx-update apply <manifest> [--no-reboot]"));
            cmd_apply(src, args.iter().any(|a| a == "--no-reboot"));
        }
        Some("sha256") => {
            // self-check against busybox sha256sum; also used to build manifests
            let f = args.get(1).unwrap_or_else(|| die("usage: aginx-update sha256 <file>"));
            println!("{}", sha256_hex_file(f));
        }
        Some("write-part") => {
            // escape hatch / test primitive: verified write, no slot logic
            let img = args.get(1).unwrap_or_else(|| die("usage: aginx-update write-part <img> <part>"));
            let part = args.get(2).unwrap_or_else(|| die("usage: aginx-update write-part <img> <part>"));
            let n = write_part(img, part);
            println!("aginx-update: {n} bytes → {part}");
        }
        _ => {
            eprintln!("usage: aginx-update <status|apply|write-part|sha256> …");
            std::process::exit(2);
        }
    }
}

/// SHA-256 (FIPS 180-4), streaming. In-crate on purpose: the updater must
/// not grow a dependency tree of its own.
struct Sha256 {
    h: [u32; 8],
    len: u64,
    buf: [u8; 64],
    n: usize,
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Sha256 {
    fn new() -> Sha256 {
        Sha256 { h: [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19], len: 0, buf: [0; 64], n: 0 }
    }

    fn block(&mut self, b: &[u8]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let mut v = self.h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
            let t1 = v[7].wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v[7] = v[6]; v[6] = v[5]; v[5] = v[4];
            v[4] = v[3].wrapping_add(t1);
            v[3] = v[2]; v[2] = v[1]; v[1] = v[0];
            v[0] = t1.wrapping_add(t2);
        }
        for i in 0..8 {
            self.h[i] = self.h[i].wrapping_add(v[i]);
        }
    }

    fn update(&mut self, mut d: &[u8]) {
        self.len = self.len.wrapping_add(d.len() as u64);
        while !d.is_empty() {
            let take = (64 - self.n).min(d.len());
            self.buf[self.n..self.n + take].copy_from_slice(&d[..take]);
            self.n += take;
            d = &d[take..];
            if self.n == 64 {
                let b = self.buf;
                self.block(&b);
                self.n = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 32] {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.n != 56 {
            self.update(&[0]);
        }
        // update() would count the length bytes into len — irrelevant now
        let mut b = self.buf;
        b[56..64].copy_from_slice(&bits.to_be_bytes());
        self.block(&b);
        let mut out = [0u8; 32];
        for (i, h) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&h.to_be_bytes());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        hex(&h.finish())
    }

    #[test]
    fn sha256_fips_vectors() {
        assert_eq!(
            sha(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sha256_streaming_chunks_match_one_shot() {
        // update()'s 64-byte buffer boundary logic: odd chunk sizes must
        // hash identically to a single pass.
        let long: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let one = sha(&long);
        let mut h = Sha256::new();
        for c in long.chunks(7) {
            h.update(c);
        }
        assert_eq!(hex(&h.finish()), one);
    }

    #[test]
    fn swap_header_byte_layout() {
        let hex64 = "ab".repeat(32);
        let h = swap_header(0x1122334455667788, &hex64, 2040373248);
        assert_eq!(h.len(), 4096);
        assert_eq!(&h[..8], b"AGXROOT1");
        assert_eq!(&h[8..12], &1u32.to_le_bytes()); // version
        assert_eq!(&h[12..16], &1u32.to_le_bytes()); // flags: pending
        assert_eq!(&h[16..24], &0x1122334455667788u64.to_le_bytes());
        assert_eq!(&h[24..88], hex64.as_bytes());
        assert_eq!(&h[88..96], &2040373248u64.to_le_bytes());
        assert!(h[96..].iter().all(|&b| b == 0));
    }

    #[test]
    fn swap_protocol_offsets_stable() {
        // M22 wire protocol — MUST match crates/aginxos-init (frozen) and
        // /etc/init.d/state-restore. Changing any of these bricks the
        // update path against every image already in the field.
        assert_eq!(SWAP_OFF, 8u64 << 30);
        assert_eq!(SWAP_HDR, 4096);
        assert_eq!(BAK_OFF, 32u64 << 30);
        assert_eq!(STATE_OFF, 64u64 << 30);
        assert_eq!(STATE_MAX, 512u64 << 20);
        assert_eq!(SWAP_MAGIC, b"AGXROOT1");
        assert_eq!(STATE_MAGIC, b"AGXSTATE");
    }

    #[test]
    fn spawn_paths_are_d13_device_names() {
        // The frozen first-gen binary died here: agdl/agboot-ok/reboot2
        // do not exist on the N4 image. These are the live D13 names.
        assert_eq!(DOWNLOAD_BIN, "/usr/bin/aginx-download");
        assert_eq!(BOOT_OK_BIN, "/usr/bin/aginx-boot-ok");
        assert_eq!(REBOOT_BIN, "/usr/bin/aginx-reboot");
    }
}
