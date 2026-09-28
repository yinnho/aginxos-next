//! Minimal ALSA PCM ioctl layer, no alsa-lib — Rust port of the
//! snd-cap/snd-play contract (rootfs/src/snd-pcm-uapi.h + snd-cap.c +
//! snd-play.c), which is the proven incantation for redfin's q6 front-ends. // D14-exempt: provenance comment (first-gen receipts), no machine data
//! The struct layout and ioctl numbers ARE the kernel ABI; the const
//! asserts freeze them the way the C header's _Static_asserts do.

use std::fs::OpenOptions;
use std::io;
use std::os::fd::IntoRawFd;
use std::os::unix::fs::OpenOptionsExt;

// ---- uapi structs (sound/asound.h subset, 4.19 layout) ----

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SndMask {
    pub bits: [u32; 8], // 256-bit bitmap
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SndInterval {
    pub min: u32,
    pub max: u32,
    pub openmin_int: u32, // bitfield word; we only ever zero it
}

#[repr(C)]
pub struct SndPcmHwParams {
    pub flags: u32,
    pub masks: [SndMask; 3],          // ids 0..2: ACCESS FORMAT SUBFORMAT
    pub mres: [SndMask; 5],           // kernel-private
    pub intervals: [SndInterval; 12], // ids 8..19
    pub ires: [SndInterval; 9],       // kernel-private
    pub rmask: u32,
    pub cmask: u32,
    pub info: u32,
    pub msbits: u32,
    pub rate_num: u32,
    pub rate_den: u32,
    pub fifo_size: u64,
    pub reserved: [u8; 64],
}

#[repr(C)]
pub struct SndPcmSwParams {
    pub tstamp_mode: i32,
    pub period_step: u32,
    pub sleep_min: u32, // long-obsolete, must be 0
    pub avail_min: u64,
    pub xfer_align: u64, // obsolete, must be 1
    pub start_threshold: u64,
    pub stop_threshold: u64,
    pub silence_threshold: u64,
    pub silence_size: u64,
    pub boundary: u64, // kernel writes this back
    pub proto: u32,    // >=6.12 only; 4.19: reserved
    pub tstamp_type: u32,
    pub reserved: [u8; 56],
}

#[repr(C)]
pub struct SndXferi {
    pub result: i64,
    pub buf: *mut i16,
    pub frames: u64,
}

const _: () = assert!(std::mem::size_of::<SndInterval>() == 12);
const _: () = assert!(std::mem::size_of::<SndPcmHwParams>() == 608);
const _: () = assert!(std::mem::size_of::<SndPcmSwParams>() == 136);
const _: () = assert!(std::mem::size_of::<SndXferi>() == 24);

// ioctl numbers frozen to the canonical uapi encoding (same values the C
// header's _Static_asserts pin on Linux). musl's ioctl takes c_int.
const HW_REFINE: libc::c_int = 0xc260_4110_u32 as libc::c_int;
const HW_PARAMS: libc::c_int = 0xc260_4111_u32 as libc::c_int;
const SW_PARAMS: libc::c_int = 0xc088_4113_u32 as libc::c_int;
const PREPARE: libc::c_int = 0x4140;
const DRAIN: libc::c_int = 0x4144;
const READI: libc::c_int = 0x8018_4151_u32 as libc::c_int;
const WRITEI: libc::c_int = 0x4018_4150;

// hw param ids (uapi): masks 0..2, intervals 8..19
const P_ACCESS: usize = 0;
const P_FORMAT: usize = 1;
const P_CHANNELS: usize = 10;
const P_RATE: usize = 11;
const P_PERIOD_SIZE: usize = 13;
const P_BUFFER_SIZE: usize = 17;

const ACCESS_RW_INTERLEAVED: u64 = 3;
const FORMAT_S16_LE: u64 = 2;

// SAFETY in this module: every raw ioctl pairs a request constant with a
// pointer to the matching struct — the pair is the uapi contract, frozen
// by the size asserts above.
fn ioctl_arg(fd: i32, req: libc::c_int, arg: *mut u8) -> io::Result<()> {
    let r = unsafe { libc::ioctl(fd, req, arg) };
    if r < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn mask_one(m: &mut SndMask, bit: u64) {
    *m = SndMask { bits: [0; 8] };
    m.bits[(bit / 32) as usize] |= 1u32 << (bit % 32);
}

fn iv<'a>(p: &'a mut SndPcmHwParams, id: usize) -> &'a mut SndInterval {
    &mut p.intervals[id - 8]
}

/// One negotiated PCM stream (capture or playback), S16_LE interleaved.
pub struct Pcm {
    pub fd: i32,
    /// frames per READI/WRITEI the kernel chose (q6 FEs pick their own
    /// quantum — snd-cap's 2026-08-31 lesson: read it back, never assume).
    pub period: usize,
    pub rate: u32,
    pub channels: u32,
    capture: bool,
}

impl Pcm {
    /// Open + negotiate. `capture` picks READI vs WRITEI semantics.
    /// Mirrors snd-cap.c/snd-play.c exactly: O_NONBLOCK open (a busy capture
    /// device blocks forever without it), HW_REFINE wide open, pin only
    /// format/rate/ch, take the refined bounds loosely for geometry, then
    /// read the chosen period back.
    pub fn open(dev: &str, rate: u32, channels: u32, capture: bool) -> anyhow::Result<Pcm> {
        let file = OpenOptions::new()
            .read(capture)
            .write(!capture)
            .custom_flags(libc::O_NONBLOCK)
            .open(dev)?;
        let fd = file.into_raw_fd();
        // back to blocking mode for the READI/WRITEI loop (snd_pcm_readi
        // semantics)
        let fl = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if fl >= 0 {
            unsafe { libc::fcntl(fd, libc::F_SETFL, fl & !libc::O_NONBLOCK) };
        }

        let mut hp: SndPcmHwParams = unsafe { std::mem::zeroed() };
        for m in hp.masks.iter_mut() {
            m.bits = [u32::MAX; 8];
        }
        for i in hp.intervals.iter_mut() {
            i.min = 0;
            i.max = u32::MAX;
        }
        hp.rmask = !0;
        ioctl_arg(fd, HW_REFINE, &mut hp as *mut _ as *mut u8)
            .map_err(|e| anyhow::anyhow!("HW_REFINE({dev}): {e}"))?;

        mask_one(&mut hp.masks[P_ACCESS], ACCESS_RW_INTERLEAVED);
        mask_one(&mut hp.masks[P_FORMAT], FORMAT_S16_LE);
        iv(&mut hp, P_CHANNELS).min = channels;
        iv(&mut hp, P_CHANNELS).max = channels;
        iv(&mut hp, P_RATE).min = rate;
        iv(&mut hp, P_RATE).max = rate;
        // cDSP only accepts its own default quantum: pin format/rate/ch
        // only, bound geometry loosely (snd-cap 2026-08-31 ADSP_EFAILED
        // lesson; restrictive hostless FEs cap period<=1024/buf<=4096).
        iv(&mut hp, P_PERIOD_SIZE).min = 16;
        iv(&mut hp, P_PERIOD_SIZE).max = rate / 2;
        iv(&mut hp, P_BUFFER_SIZE).min = if capture { 128 } else { rate };
        iv(&mut hp, P_BUFFER_SIZE).max = rate * 4;
        hp.rmask = (1 << P_ACCESS)
            | (1 << P_FORMAT)
            | (1 << P_CHANNELS)
            | (1 << P_RATE)
            | (1 << P_PERIOD_SIZE)
            | (1 << P_BUFFER_SIZE);
        if let Err(e) = ioctl_arg(fd, HW_PARAMS, &mut hp as *mut _ as *mut u8) {
            dump_caps(fd);
            anyhow::bail!("HW_PARAMS({dev}): {e}");
        }
        // kernel writes the chosen geometry back — read the real period
        // (a fixed 1024 against a 2000-frame q6 FE drains at half speed
        // and overruns into an XRUN a few seconds in).
        let mut period = iv(&mut hp, P_PERIOD_SIZE).min as usize;
        if !(16..=rate as usize).contains(&period) {
            period = 1024;
        }

        let mut sp: SndPcmSwParams = unsafe { std::mem::zeroed() };
        sp.xfer_align = 1; // obsolete but old kernels validate it
        if capture {
            sp.avail_min = 1;
            sp.start_threshold = 1; // start on first frame
            // capture must never self-stop on overrun (stop_threshold of
            // rate*4 made the kernel XRUN + flush the aDSP session):
            // ~INT64_MAX = ALSA "boundary".
            sp.stop_threshold = 1u64 << 62;
        } else {
            sp.avail_min = (rate / 4) as u64;
            sp.start_threshold = (rate / 4) as u64; // first period kicks off
            sp.stop_threshold = (rate * 4) as u64;
        }
        ioctl_arg(fd, SW_PARAMS, &mut sp as *mut _ as *mut u8)
            .map_err(|e| anyhow::anyhow!("SW_PARAMS({dev}): {e}"))?;
        ioctl_arg(fd, PREPARE, std::ptr::null_mut())
            .map_err(|e| anyhow::anyhow!("PREPARE({dev}): {e}"))?;

        println!(
            "[pcm] {dev}: {} Hz / {} ch / S16_LE, period {period} ({:.1} ms){}",
            rate,
            channels,
            period as f64 * 1000.0 / rate as f64,
            if capture { " (capture)" } else { " (playback)" },
        );
        Ok(Pcm { fd, period, rate, channels, capture })
    }

    /// Blocking interleaved read/write of `buf` (frames = buf.len()/ch).
    pub fn xferi(&self, buf: &mut [i16]) -> io::Result<usize> {
        let frames = (buf.len() / self.channels as usize) as u64;
        let mut x = SndXferi {
            result: 0,
            buf: buf.as_mut_ptr(),
            frames,
        };
        loop {
            match ioctl_arg(
                self.fd,
                if self.capture { READI } else { WRITEI },
                &mut x as *mut _ as *mut u8,
            ) {
                Ok(()) => {
                    if x.result < 0 {
                        return Err(io::Error::other(format!("xferi result {}", x.result)));
                    }
                    return Ok(x.result as usize);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// Let the tail drain before drop (alsa drops on close otherwise).
    pub fn drain(&self) {
        let _ = ioctl_arg(self.fd, DRAIN, std::ptr::null_mut());
    }
}

impl Drop for Pcm {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

/// On HW_PARAMS failure, re-refine and print what the device actually
/// supports — a failed pin (no S16_LE, only 48 kHz…) reads as data.
fn dump_caps(fd: i32) {
    let mut p: SndPcmHwParams = unsafe { std::mem::zeroed() };
    for m in p.masks.iter_mut() {
        m.bits = [u32::MAX; 8];
    }
    for i in p.intervals.iter_mut() {
        i.min = 0;
        i.max = u32::MAX;
    }
    p.rmask = !0;
    if ioctl_arg(fd, HW_REFINE, &mut p as *mut _ as *mut u8).is_err() {
        return;
    }
    let fmts = [
        (0u64, "S8"),
        (1, "U8"),
        (2, "S16_LE"),
        (3, "S16_BE"),
        (6, "S24_LE"),
        (10, "S32_LE"),
        (11, "S24_3LE"),
    ];
    let supported: Vec<&str> = fmts
        .iter()
        .filter(|(id, _)| p.masks[P_FORMAT].bits[(id / 32) as usize] & (1u32 << (id % 32)) != 0)
        .map(|(_, n)| *n)
        .collect();
    let rate_min = iv(&mut p, P_RATE).min;
    let rate_max = iv(&mut p, P_RATE).max;
    let ch_min = iv(&mut p, P_CHANNELS).min;
    let ch_max = iv(&mut p, P_CHANNELS).max;
    let per_min = iv(&mut p, P_PERIOD_SIZE).min;
    let per_max = iv(&mut p, P_PERIOD_SIZE).max;
    let buf_min = iv(&mut p, P_BUFFER_SIZE).min;
    let buf_max = iv(&mut p, P_BUFFER_SIZE).max;
    eprintln!(
        "[pcm] device supports: {:?} | rate {}..{} | ch {}..{} | period {}..{} | buf {}..{}",
        supported,
        rate_min,
        rate_max,
        ch_min,
        ch_max,
        per_min,
        per_max,
        buf_min,
        buf_max,
    );
}
