//! aginx-fg — qpnp-qg user-data pack + minimal FG surface.
//!
//! Kernel contract (`include/uapi/linux/qg.h`, stock redfin = Lineage redbull): // D14-exempt
//! ```text
//! struct qg_param { u32 data; bool valid; };           /* 8B */
//! struct qg_user_data { qg_param param[QG_MAX]; };     /* QG_MAX=18 → 144B */
//! struct fifo_data { u32 v, i, count, interval; };     /* 16B */
//! struct qg_kernel_data {
//!   u32 seq_no, fifo_time, fifo_length;
//!   fifo_data fifo[16];
//!   qg_param param[18];
//! };                                                   /* 412B */
//! ```
//!
//! `qg_device_write` rejects count < 144 and copies exactly 144.
//! `qg_device_read` requires count ≥ 412 and blocks until `data_ready`
//! (or returns 0 under O_NONBLOCK).
//!
//! SOC unit: soco_raw = percent × 100 (9900 == 99.00%).
//!
//! WARNING: `process_udata_work` copies QG_OCV_UV / QG_RBAT_MOHM `.data`
//! into SDAM whenever SYS/SOC is valid — even if those params have
//! `valid=0`. Never leave them as accidental zeros if SDAM holds a real OCV;
//! pass explicit ocv/rbat or leave them unset only when current SDAM is
//! already zero / unknown-safe.
//!
//! Canonical unlock blob (SYS=CC=9900, rest zero) md5:
//! `6755de4de653a48a90e3d357fb9dce78` (verified 2026-10-12 on device).

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use md5::{Digest, Md5};

/// Array length — not a payload slot.
pub const QG_MAX: usize = 18;
pub const PARAM_SIZE: usize = 8;
pub const UDATA_SIZE: usize = QG_MAX * PARAM_SIZE; // 144
pub const MAX_FIFO_LENGTH: usize = 16;
pub const FIFO_ENTRY_SIZE: usize = 16;
/// `sizeof(struct qg_kernel_data)` on aarch64/Linux (host sizeof check: 412).
pub const KDATA_SIZE: usize = 4 + 4 + 4 + (MAX_FIFO_LENGTH * FIFO_ENTRY_SIZE) + UDATA_SIZE;

pub const QG_OCV_UV: usize = 1;
pub const QG_RBAT_MOHM: usize = 2;
pub const QG_CC_SOC: usize = 9;
pub const QG_SYS_SOC: usize = 16;

pub const QG_ENUM: [&str; QG_MAX] = [
    "QG_SOC",
    "QG_OCV_UV",
    "QG_RBAT_MOHM",
    "QG_PON_OCV_UV",
    "QG_GOOD_OCV_UV",
    "QG_ESR",
    "QG_CHARGE_COUNTER",
    "QG_FIFO_TIME_DELTA",
    "QG_BATT_SOC",
    "QG_CC_SOC",
    "QG_ESR_CHARGE_DELTA",
    "QG_ESR_DISCHARGE_DELTA",
    "QG_ESR_CHARGE_SF",
    "QG_ESR_DISCHARGE_SF",
    "QG_FULL_SOC",
    "QG_CLEAR_LEARNT_DATA",
    "QG_SYS_SOC",
    "QG_V_IBAT",
];

/// Verified 2026-10-12: SYS+CC=9900, all else zero.
pub const CANONICAL_9900_MD5: &str = "6755de4de653a48a90e3d357fb9dce78";

pub const DEFAULT_QG_DEV: &str = "/dev/qg";
pub const BMS_CAPACITY: &str = "/sys/class/power_supply/bms/capacity";
pub const BMS_SYS_SOC: &str = "/sys/class/power_supply/bms/sys_soc";
pub const BMS_CC_SOC: &str = "/sys/class/power_supply/bms/cc_soc";
pub const BMS_FG_RESET: &str = "/sys/class/power_supply/bms/fg_reset";
pub const BMS_CHARGE_FULL: &str = "/sys/class/power_supply/bms/charge_full";
pub const BMS_CHARGE_FULL_DESIGN: &str = "/sys/class/power_supply/bms/charge_full_design";
pub const BMS_VOLTAGE_OCV: &str = "/sys/class/power_supply/bms/voltage_ocv";
pub const BMS_VOLTAGE_NOW: &str = "/sys/class/power_supply/bms/voltage_now";
pub const BMS_RESISTANCE_NOW: &str = "/sys/class/power_supply/bms/resistance_now";
pub const BMS_RESISTANCE: &str = "/sys/class/power_supply/bms/resistance";
pub const BATT_STATUS: &str = "/sys/class/power_supply/battery/status";
pub const BATT_CHARGE_TYPE: &str = "/sys/class/power_supply/battery/charge_type";

/// Zero-recovery voltage floor: live VBAT must be at least this (µV).
pub const RECOVER_MIN_UV: u32 = 4_350_000;
/// SOC stamped by zero-recovery — the canonical 10-12 unlock value.
pub const RECOVER_SOCO: u32 = 9900;
/// `qg_reset` polls QG_OK up to 20×200ms; wait this long before the udata write.
pub const RECOVER_RESET_WAIT_SECS: u64 = 5;

/// soco_raw at 100.00%.
pub const SOCO_FULL: u32 = 10_000;
/// Soft floor — never stamp 0 into udata (10-10 weld).
pub const SOCO_FLOOR: u32 = 1;
/// Kernel `process_udata_work`: input present + (CC − FULL) > this → good_ocv.
pub const MAX_CC_SOC_DELTA: i32 = 300;
/// Pixel 5 design capacity (µAh); overridden by charge_full(_design) on device.
pub const DEFAULT_FULL_UAH: u32 = 4_187_000;
/// Measured ESR / rbat fallback (mΩ) when sysfs resistance_* is 0.
pub const DEFAULT_RBAT_MOHM: u32 = 84;

#[derive(Debug, Clone, Default)]
pub struct UdataBuilder {
    pub sys_soc: Option<u32>,
    pub cc_soc: Option<u32>,
    pub ocv_uv: Option<u32>,
    pub rbat_mohm: Option<u32>,
}

impl UdataBuilder {
    pub fn unlock_raw(soco_raw: u32) -> Self {
        Self {
            sys_soc: Some(soco_raw),
            cc_soc: Some(soco_raw),
            ocv_uv: None,
            rbat_mohm: None,
        }
    }

    pub fn build(&self) -> [u8; UDATA_SIZE] {
        let mut blob = [0u8; UDATA_SIZE];
        if let Some(v) = self.cc_soc {
            set_param(&mut blob, QG_CC_SOC, v, true);
        }
        if let Some(v) = self.sys_soc {
            set_param(&mut blob, QG_SYS_SOC, v, true);
        }
        if let Some(v) = self.ocv_uv {
            set_param(&mut blob, QG_OCV_UV, v, true);
        }
        if let Some(v) = self.rbat_mohm {
            set_param(&mut blob, QG_RBAT_MOHM, v, true);
        }
        blob
    }
}

/// Pack one `qg_param` at `idx` (little-endian u32 + bool + 3 pad).
pub fn set_param(blob: &mut [u8; UDATA_SIZE], idx: usize, data: u32, valid: bool) {
    assert!(idx < QG_MAX, "param index {idx} out of range");
    let off = idx * PARAM_SIZE;
    blob[off..off + 4].copy_from_slice(&data.to_le_bytes());
    blob[off + 4] = u8::from(valid);
    blob[off + 5] = 0;
    blob[off + 6] = 0;
    blob[off + 7] = 0;
}

pub fn get_param(blob: &[u8; UDATA_SIZE], idx: usize) -> (u32, bool) {
    assert!(idx < QG_MAX);
    let off = idx * PARAM_SIZE;
    let data = u32::from_le_bytes(blob[off..off + 4].try_into().unwrap());
    let valid = blob[off + 4] != 0;
    (data, valid)
}

pub fn md5_hex(data: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

pub fn describe(blob: &[u8; UDATA_SIZE]) -> String {
    let mut out = format!(
        "size={} (QG_MAX={} × {}B)\nmd5={}\n",
        blob.len(),
        QG_MAX,
        PARAM_SIZE,
        md5_hex(blob)
    );
    out.push_str("enum:\n");
    for (i, name) in QG_ENUM.iter().enumerate() {
        let (data, valid) = get_param(blob, i);
        if valid {
            out.push_str(&format!("  {i:2} {name:<24} data={data} valid=1 <<\n"));
        } else {
            out.push_str(&format!("  {i:2} {name:<24} (zero)\n"));
        }
    }
    out.push_str(&format!(
        "offsets: QG_CC_SOC@{}  QG_SYS_SOC@{}\n",
        QG_CC_SOC * PARAM_SIZE,
        QG_SYS_SOC * PARAM_SIZE
    ));
    out
}

/// One FIFO sample from kdata (voltage / current / count / interval).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FifoSample {
    pub v: u32,
    pub i: u32,
    pub count: u32,
    pub interval: u32,
}

/// Parsed `qg_kernel_data` (412B).
#[derive(Debug, Clone)]
pub struct KernelData {
    pub seq_no: u32,
    pub fifo_time: u32,
    pub fifo_length: u32,
    pub fifo: [FifoSample; MAX_FIFO_LENGTH],
    /// Raw param slots (data, valid) — same layout as udata.
    pub param: [(u32, bool); QG_MAX],
}

impl KernelData {
    pub fn parse(buf: &[u8; KDATA_SIZE]) -> Self {
        let seq_no = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        let fifo_time = u32::from_le_bytes(buf[4..8].try_into().unwrap());
        let fifo_length = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        let mut fifo = [FifoSample::default(); MAX_FIFO_LENGTH];
        for (n, slot) in fifo.iter_mut().enumerate() {
            let off = 12 + n * FIFO_ENTRY_SIZE;
            *slot = FifoSample {
                v: u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()),
                i: u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap()),
                count: u32::from_le_bytes(buf[off + 8..off + 12].try_into().unwrap()),
                interval: u32::from_le_bytes(buf[off + 12..off + 16].try_into().unwrap()),
            };
        }
        let param_off = 12 + MAX_FIFO_LENGTH * FIFO_ENTRY_SIZE;
        let mut param = [(0u32, false); QG_MAX];
        for (n, slot) in param.iter_mut().enumerate() {
            let off = param_off + n * PARAM_SIZE;
            let data = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
            let valid = buf[off + 4] != 0;
            *slot = (data, valid);
        }
        Self {
            seq_no,
            fifo_time,
            fifo_length,
            fifo,
            param,
        }
    }
}

#[derive(Debug)]
pub enum FgError {
    Io(io::Error),
    SizeGate { got: usize, expect: usize },
    Md5Gate { got: String, expect: String },
    BmsUnsafe { capacity: i32 },
    BmsUnreadable,
    SocForbidden { soco_raw: u32 },
    RecoverRefused {
        capacity: i32,
        voltage_uv: u32,
        status: String,
        charge_type: String,
    },
    Msg(String),
}

impl std::fmt::Display for FgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FgError::Io(e) => write!(f, "io: {e}"),
            FgError::SizeGate { got, expect } => {
                write!(f, "size gate FAIL: got {got} expect {expect}")
            }
            FgError::Md5Gate { got, expect } => {
                write!(f, "md5 gate FAIL: got {got} expect {expect}")
            }
            FgError::BmsUnsafe { capacity } => {
                write!(f, "refusing write: bms capacity={capacity} (< 1)")
            }
            FgError::BmsUnreadable => write!(f, "refusing write: bms capacity unreadable"),
            FgError::SocForbidden { soco_raw } => {
                write!(f, "refusing write: soco_raw={soco_raw} (never stamp 0)")
            }
            FgError::RecoverRefused {
                capacity,
                voltage_uv,
                status,
                charge_type,
            } => write!(
                f,
                "refusing zero-recovery: bms={capacity} vbat={voltage_uv}uV status={status} \
                 charge_type={charge_type} (need bms<1, vbat>={RECOVER_MIN_UV}uV, \
                 status Full or charge_type Taper)"
            ),
            FgError::Msg(s) => write!(f, "{s}"),
        }
    }
}

impl From<io::Error> for FgError {
    fn from(e: io::Error) -> Self {
        FgError::Io(e)
    }
}

impl std::error::Error for FgError {}

/// Read bms capacity percent (0–100), if present.
pub fn read_bms_capacity(path: impl AsRef<Path>) -> io::Result<i32> {
    let s = std::fs::read_to_string(path)?;
    s.trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Read a sysfs u32 (trim), if present and parseable.
pub fn read_sysfs_u32(path: impl AsRef<Path>) -> io::Result<u32> {
    let s = std::fs::read_to_string(path)?;
    s.trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// FIFO `i` raw bits as signed µA (two's complement cast).
///
/// **Sign vs Linux psy:** on redfin qpnp-qg, kdata FIFO `i` is empirically // D14-exempt
/// **opposite** `battery/current_now` — discharge shows **positive** FIFO
/// (e.g. +100402…+365295) while psy is negative (−140k…−400k µA). Use
/// [`fifo_i_ua_coulomb`] for ΣI×dt so discharge drains soco_raw.
#[inline]
pub fn fifo_i_ua(raw: u32) -> i32 {
    raw as i32
}

/// Effective current for coulomb: negate FIFO `i` to match Linux psy
/// (discharge negative → soco drops). See [`fifo_i_ua`].
#[inline]
pub fn fifo_i_ua_coulomb(raw: u32) -> i32 {
    -fifo_i_ua(raw)
}

/// Design / learned full capacity in µAh.
pub fn read_full_uah() -> u32 {
    for p in [BMS_CHARGE_FULL, BMS_CHARGE_FULL_DESIGN] {
        if let Ok(v) = read_sysfs_u32(p) {
            if v > 0 {
                return v;
            }
        }
    }
    DEFAULT_FULL_UAH
}

/// Plausible pack ESR / RBAT in milliohms (Pixel-class cells ~30–200).
pub fn rbat_mohm_plausible(v: u32) -> bool {
    (1..=500).contains(&v)
}

/// OCV/RBAT care for SYS/SOC writes: `process_udata_work` copies `.data`
/// into SDAM whenever SYS is valid — even if `valid=0`. Prefer real sysfs
/// values; fall back to voltage_now + DEFAULT_RBAT_MOHM so we never weld 0.
///
/// Note: without stock FG, `bms/resistance` often returns junk (e.g. 16e6);
/// only accept values in [`rbat_mohm_plausible`]. QG_RBAT_MOHM is **mΩ**.
pub fn read_ocv_rbat_care(
    override_ocv: Option<u32>,
    override_rbat: Option<u32>,
) -> (Option<u32>, Option<u32>) {
    // Prefer voltage_now each stamp — stock FG absent → voltage_ocv (SDAM)
    // freezes at last good OCV (e.g. 4449207) while the pack is already
    // unloading at ~4.40V. Fall back to voltage_ocv only if now unreadable.
    let ocv = override_ocv.or_else(|| {
        read_sysfs_u32(BMS_VOLTAGE_NOW)
            .ok()
            .filter(|&v| v > 0)
            .or_else(|| read_sysfs_u32(BMS_VOLTAGE_OCV).ok().filter(|&v| v > 0))
    });
    let rbat = override_rbat
        .filter(|&v| rbat_mohm_plausible(v))
        .or_else(|| {
            read_sysfs_u32(BMS_RESISTANCE_NOW)
                .ok()
                .filter(|&v| rbat_mohm_plausible(v))
                .or_else(|| {
                    read_sysfs_u32(BMS_RESISTANCE)
                        .ok()
                        .filter(|&v| rbat_mohm_plausible(v))
                })
                .or(Some(DEFAULT_RBAT_MOHM))
        });
    (ocv, rbat)
}

/// Read `battery/status` trimmed, if present.
pub fn read_battery_status() -> Option<String> {
    std::fs::read_to_string(BATT_STATUS)
        .ok()
        .map(|s| s.trim().to_string())
}

/// True only when google_battery status is exactly Full.
///
/// Do **not** infer Full from capacity==100 — after unplug, rl can sit at
/// 100 while status is already Discharging; nailing 10000 then fights
/// coulomb drain.
pub fn battery_is_full() -> bool {
    read_battery_status()
        .map(|s| s.eq_ignore_ascii_case("Full"))
        .unwrap_or(false)
}

/// Read `battery/charge_type` trimmed, if present.
pub fn read_charge_type() -> Option<String> {
    std::fs::read_to_string(BATT_CHARGE_TYPE)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Pure gate for pulling a welded-0 qg back to [`RECOVER_SOCO`].
///
/// The ordinary bms<1 gate can never open on its own after a deep discharge:
/// msoc only moves when userspace writes udata, and writes are refused while
/// bms<1. Open the side door only on live evidence that the cell is near
/// full: VBAT ≥ [`RECOVER_MIN_UV`] **and** (status Full **or** charge_type
/// Taper — constant-voltage phase).
pub fn zero_recovery_allowed(
    bms_capacity: i32,
    voltage_uv: u32,
    status: &str,
    charge_type: &str,
) -> bool {
    bms_capacity < 1
        && voltage_uv >= RECOVER_MIN_UV
        && (status.eq_ignore_ascii_case("Full") || charge_type.eq_ignore_ascii_case("Taper"))
}

/// Recover a welded-0 qg (10-10 / 10-15 deep-discharge deadlock).
///
/// Checks [`zero_recovery_allowed`] on live sysfs, kicks `fg_reset` first
/// (S7 OCV resample + `force_soc`), waits [`RECOVER_RESET_WAIT_SECS`], then
/// writes the md5-gated canonical [`RECOVER_SOCO`] blob so the forced
/// `process_udata_work` consumes it. Returns the VBAT evidence used.
pub fn recover_from_zero(dev: impl AsRef<Path>) -> Result<u32, FgError> {
    let capacity = read_bms_capacity(BMS_CAPACITY).map_err(|_| FgError::BmsUnreadable)?;
    let voltage_uv = read_sysfs_u32(BMS_VOLTAGE_NOW)?;
    let status = read_battery_status().unwrap_or_default();
    let charge_type = read_charge_type().unwrap_or_default();
    if !zero_recovery_allowed(capacity, voltage_uv, &status, &charge_type) {
        return Err(FgError::RecoverRefused {
            capacity,
            voltage_uv,
            status,
            charge_type,
        });
    }
    let blob = UdataBuilder::unlock_raw(RECOVER_SOCO).build();
    let got = md5_hex(&blob);
    if got != CANONICAL_9900_MD5 {
        return Err(FgError::Md5Gate {
            got,
            expect: CANONICAL_9900_MD5.to_string(),
        });
    }
    if Path::new(BMS_FG_RESET).exists() {
        let mut rf = OpenOptions::new().write(true).open(BMS_FG_RESET)?;
        rf.write_all(b"1\n")?;
        std::thread::sleep(std::time::Duration::from_secs(RECOVER_RESET_WAIT_SECS));
    }
    let mut f = OpenOptions::new().write(true).open(dev.as_ref())?;
    write_udata_fd(&mut f, &blob)?;
    Ok(voltage_uv)
}

/// True when status is Discharging (unplugged / unloading).
pub fn battery_is_discharging() -> bool {
    read_battery_status()
        .map(|s| s.eq_ignore_ascii_case("Discharging"))
        .unwrap_or(false)
}

/// Write 144B udata to `/dev/qg` (or override) with size + optional md5 + bms gates.
///
/// Safety: refuses `soco_raw == 0` when the builder sets SYS/CC, refuses bms < 1
/// when the capacity sysfs node exists, and never skips the size gate.
pub fn write_udata(
    dev: impl AsRef<Path>,
    blob: &[u8; UDATA_SIZE],
    expect_md5: Option<&str>,
    fg_reset: bool,
) -> Result<(), FgError> {
    if blob.len() != UDATA_SIZE {
        return Err(FgError::SizeGate {
            got: blob.len(),
            expect: UDATA_SIZE,
        });
    }
    let got = md5_hex(blob);
    if let Some(expect) = expect_md5 {
        if got != expect {
            return Err(FgError::Md5Gate {
                got,
                expect: expect.to_string(),
            });
        }
    }

    // Never stamp a zero SOC into SYS/CC (would re-weld the 10-10 failure mode).
    let (sys, sys_v) = get_param(blob, QG_SYS_SOC);
    let (cc, cc_v) = get_param(blob, QG_CC_SOC);
    if (sys_v && sys == 0) || (cc_v && cc == 0) {
        return Err(FgError::SocForbidden {
            soco_raw: if sys_v { sys } else { cc },
        });
    }

    if Path::new(BMS_CAPACITY).exists() {
        match read_bms_capacity(BMS_CAPACITY) {
            Ok(c) if c < 1 => return Err(FgError::BmsUnsafe { capacity: c }),
            Ok(_) => {}
            Err(_) => return Err(FgError::BmsUnreadable),
        }
    }

    let mut f = OpenOptions::new().write(true).open(dev.as_ref())?;
    write_udata_fd(&mut f, blob)?;

    if fg_reset && Path::new(BMS_FG_RESET).exists() {
        let mut rf = OpenOptions::new().write(true).open(BMS_FG_RESET)?;
        rf.write_all(b"1\n")?;
    }
    Ok(())
}

/// Write 144B through an already-open `/dev/qg` fd (daemon path).
/// Applies the same SOC=0 / size gates as [`write_udata`] but skips re-open
/// and the optional bms/md5/fg_reset checks (caller already gated).
pub fn write_udata_fd(f: &mut File, blob: &[u8; UDATA_SIZE]) -> Result<(), FgError> {
    let (sys, sys_v) = get_param(blob, QG_SYS_SOC);
    let (cc, cc_v) = get_param(blob, QG_CC_SOC);
    if (sys_v && sys == 0) || (cc_v && cc == 0) {
        return Err(FgError::SocForbidden {
            soco_raw: if sys_v { sys } else { cc },
        });
    }
    let n = f.write(blob)?;
    if n != UDATA_SIZE {
        return Err(FgError::SizeGate {
            got: n,
            expect: UDATA_SIZE,
        });
    }
    f.flush()?;
    Ok(())
}

/// Non-blocking read of one kdata frame. Returns `Ok(None)` if no data ready.
pub fn try_read_kdata(dev: &mut File) -> Result<Option<KernelData>, FgError> {
    // Caller should open with O_NONBLOCK for try semantics; if blocking,
    // this waits for FIFO. We always read exactly KDATA_SIZE.
    let mut buf = [0u8; KDATA_SIZE];
    match dev.read(&mut buf) {
        Ok(0) => Ok(None),
        Ok(n) if n == KDATA_SIZE => Ok(Some(KernelData::parse(&buf))),
        Ok(n) => Err(FgError::SizeGate {
            got: n,
            expect: KDATA_SIZE,
        }),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(FgError::Io(e)),
    }
}

/// Open `/dev/qg` read-write. Single-opener misc device — opening is the mutex.
pub fn open_qg(dev: impl AsRef<Path>, nonblock: bool) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if nonblock {
            opts.custom_flags(libc_nonblock());
        }
    }
    let _ = nonblock; // non-unix: ignore
    opts.open(dev)
}

#[cfg(unix)]
fn libc_nonblock() -> i32 {
    // Avoid a libc crate dep: O_NONBLOCK is 0x800 on aarch64/arm Linux, 0x4 on macOS.
    // Daemon runs on device (Linux); host tests use tmp files.
    #[cfg(target_os = "linux")]
    {
        0x800
    }
    #[cfg(not(target_os = "linux"))]
    {
        0x4 // macOS O_NONBLOCK — only for host dry paths
    }
}

/// Coulomb-integration state: Σ(I×dt) from kdata FIFO → soco_raw.
///
/// Kernel / device contract (`qg_process_fifo` + redfin empirics 2026-10-12): // D14-exempt
/// - `fifo.i` = signed µA in `u32` bits; **FIFO sign is opposite Linux psy**
///   on this SoC (discharge → positive FIFO, negative `current_now`)
/// - coulomb uses [`fifo_i_ua_coulomb`] (= `-fifo_i_ua`) so discharge drains
/// - `fifo.interval` = sample interval in **ms** (`get_sample_interval` → reg×10)
/// - one entry covers `count * interval` ms (`get_fifo_done_time`)
/// - µAh = µA × ms / 3_600_000
/// - `fifo_length == 0` frames are no-ops (caller should skip)
///
/// Soft floor ≥ [`SOCO_FLOOR`]; Full nail → [`SOCO_FULL`]. Never emits 0.
/// `MAX_CC_SOC_DELTA` (300) + USB present → kernel good_ocv if CC ≫ FULL;
/// we clamp to FULL so that path stays quiet.
#[derive(Debug, Clone, Default)]
pub struct CoulombState {
    /// Tracked soco_raw (percent × 100).
    pub soco_raw: u32,
    /// Residual µAh not yet folded into soco_raw (signed).
    pub delta_uah: i64,
    /// Design/full capacity in µAh (0 = integrate is a no-op).
    pub full_uah: u32,
    /// Last seen kdata seq_no (gap detect).
    pub last_seq: u32,
    /// Samples applied since start (telemetry).
    pub samples: u64,
}

impl CoulombState {
    pub fn new(seed_soco_raw: u32, full_uah: u32) -> Self {
        Self {
            soco_raw: seed_soco_raw.clamp(SOCO_FLOOR, SOCO_FULL),
            delta_uah: 0,
            full_uah,
            last_seq: 0,
            samples: 0,
        }
    }

    /// Nail to 100.00% (call when battery STATUS=Full).
    pub fn nail_full(&mut self) {
        self.soco_raw = SOCO_FULL;
        self.delta_uah = 0;
    }

    /// Integrate one kdata frame. Returns updated soco_raw (floor ≥1, ≤FULL).
    pub fn integrate(&mut self, k: &KernelData) -> u32 {
        if k.seq_no != 0 {
            self.last_seq = k.seq_no;
        }
        let n = (k.fifo_length as usize).min(MAX_FIFO_LENGTH);
        if n == 0 || self.full_uah == 0 {
            return self.soco_raw.clamp(SOCO_FLOOR, SOCO_FULL);
        }
        for s in k.fifo.iter().take(n) {
            // Invert FIFO sign → Linux psy convention (discharge negative).
            let i_ua = fifo_i_ua_coulomb(s.i) as i64;
            let dt_ms = (s.count as u64).saturating_mul(s.interval as u64);
            if dt_ms == 0 {
                continue;
            }
            // µAh = µA * ms / 3_600_000
            let duah = i_ua.saturating_mul(dt_ms as i64) / 3_600_000;
            self.delta_uah = self.delta_uah.saturating_add(duah);
            self.samples = self.samples.saturating_add(1);
        }
        self.fold_delta();
        self.soco_raw
    }

    /// Fold residual µAh into soco_raw, keeping a sub-LSB remainder.
    fn fold_delta(&mut self) {
        if self.full_uah == 0 {
            return;
        }
        let full = self.full_uah as i64;
        // Δsoco_raw = delta_uah * 10000 / full_uah
        let dsoco = self.delta_uah.saturating_mul(SOCO_FULL as i64) / full;
        if dsoco == 0 {
            self.soco_raw = self.soco_raw.clamp(SOCO_FLOOR, SOCO_FULL);
            return;
        }
        let next = (self.soco_raw as i64).saturating_add(dsoco);
        self.soco_raw = next.clamp(SOCO_FLOOR as i64, SOCO_FULL as i64) as u32;
        // keep remainder: delta_uah -= dsoco * full / 10000
        self.delta_uah = self
            .delta_uah
            .saturating_sub(dsoco.saturating_mul(full) / SOCO_FULL as i64);
    }
}

pub fn hold_udata(soco_raw: u32, ocv_uv: Option<u32>, rbat_mohm: Option<u32>) -> [u8; UDATA_SIZE] {
    UdataBuilder {
        sys_soc: Some(soco_raw),
        cc_soc: Some(soco_raw),
        ocv_uv,
        rbat_mohm,
    }
    .build()
}

/// qpnp-qg `process_udata_work` (redfin kernel): when **no input present**, // D14-exempt
/// a CC_SOC write **above** the live kernel `cc_soc` is rejected
/// (`cc_soc N is not monotonic`). SYS_SOC still applies. While charging,
/// upward CC is allowed (unless CC ≫ FULL by [`MAX_CC_SOC_DELTA`]).
///
/// So on discharge we clamp the stamped CC to `min(soco, live_cc)`.
pub fn hold_udata_monotonic(
    soco_raw: u32,
    ocv_uv: Option<u32>,
    rbat_mohm: Option<u32>,
    discharging: bool,
    live_cc_soc: Option<u32>,
) -> [u8; UDATA_SIZE] {
    let soco = soco_raw.clamp(SOCO_FLOOR, SOCO_FULL);
    let cc = if discharging {
        match live_cc_soc {
            Some(live) if live >= SOCO_FLOOR => soco.min(live),
            _ => soco,
        }
    } else {
        soco
    };
    UdataBuilder {
        sys_soc: Some(soco),
        cc_soc: Some(cc),
        ocv_uv,
        rbat_mohm,
    }
    .build()
}

/// Seed coulomb from live qg state, not bare `bms/capacity`.
///
/// Prefer `sys_soc` (soco_raw) when healthy; else `capacity×100`. If
/// discharging and live `cc_soc` is below that seed, drop to `cc_soc` so the
/// first stamp cannot trip the kernel non-monotonic gate (e.g. after a
/// concurrent `run --once` lowered CC while the daemon still held a higher
/// in-memory estimate).
pub fn seed_soco_raw() -> Result<u32, FgError> {
    let bms_pct = read_bms_capacity(BMS_CAPACITY).map_err(FgError::Io)?;
    if bms_pct < 1 {
        return Err(FgError::SocForbidden {
            soco_raw: bms_pct as u32 * 100,
        });
    }
    let from_bms = (bms_pct as u32).saturating_mul(100);
    let from_sys = read_sysfs_u32(BMS_SYS_SOC)
        .ok()
        .filter(|&v| (SOCO_FLOOR..=SOCO_FULL).contains(&v));
    let mut seed = from_sys.unwrap_or(from_bms).clamp(SOCO_FLOOR, SOCO_FULL);
    if battery_is_discharging() {
        if let Ok(live_cc) = read_sysfs_u32(BMS_CC_SOC) {
            if (SOCO_FLOOR..=SOCO_FULL).contains(&live_cc) && live_cc < seed {
                seed = live_cc;
            }
        }
    }
    Ok(seed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_match_kernel() {
        assert_eq!(UDATA_SIZE, 144);
        assert_eq!(KDATA_SIZE, 412);
        assert_eq!(QG_ENUM.len(), QG_MAX);
        assert_eq!(QG_CC_SOC * PARAM_SIZE, 72);
        assert_eq!(QG_SYS_SOC * PARAM_SIZE, 128);
    }

    #[test]
    fn zero_recovery_gate() {
        // 10-15 live scene: bms welded 0, VBAT 4.456V, Charging/Taper → allowed.
        assert!(zero_recovery_allowed(0, 4_456_214, "Charging", "Taper"));
        // Full status alone is enough evidence (charge_type may read None/Fast).
        assert!(zero_recovery_allowed(0, 4_400_000, "Full", "N/A"));
        // Boundary: exactly the floor passes, one µV below refuses.
        assert!(zero_recovery_allowed(0, RECOVER_MIN_UV, "Charging", "Taper"));
        assert!(!zero_recovery_allowed(0, RECOVER_MIN_UV - 1, "Charging", "Taper"));
        // Healthy bms → not a recovery case (normal unlock path owns it).
        assert!(!zero_recovery_allowed(1, 4_456_000, "Charging", "Taper"));
        // Charging but still constant-current (Fast) → refuse: high terminal V under load.
        assert!(!zero_recovery_allowed(0, 4_400_000, "Charging", "Fast"));
        // Discharging deep cell (10-15 last stamp 2.766V) → refuse.
        assert!(!zero_recovery_allowed(0, 2_765_791, "Discharging", "N/A"));
        // Unreadable status/type default to empty → refuse.
        assert!(!zero_recovery_allowed(0, 4_456_000, "", ""));
        assert_eq!(RECOVER_SOCO, 9900);
        assert_eq!(
            md5_hex(&UdataBuilder::unlock_raw(RECOVER_SOCO).build()),
            CANONICAL_9900_MD5
        );
    }

    #[test]
    fn canonical_9900_md5() {
        let blob = UdataBuilder::unlock_raw(9900).build();
        assert_eq!(md5_hex(&blob), CANONICAL_9900_MD5);
        let (sys, sv) = get_param(&blob, QG_SYS_SOC);
        let (cc, cv) = get_param(&blob, QG_CC_SOC);
        assert!(sv && cv);
        assert_eq!(sys, 9900);
        assert_eq!(cc, 9900);
        // OCV/RBAT must stay valid=0 with data=0 for the canonical recipe
        let (ocv, ov) = get_param(&blob, QG_OCV_UV);
        let (rb, rv) = get_param(&blob, QG_RBAT_MOHM);
        assert!(!ov && !rv && ocv == 0 && rb == 0);
    }

    #[test]
    fn ocv_rbat_slots() {
        let blob = UdataBuilder {
            sys_soc: Some(9900),
            cc_soc: Some(9900),
            ocv_uv: Some(4_415_900),
            rbat_mohm: Some(84),
        }
        .build();
        let (ocv, ov) = get_param(&blob, QG_OCV_UV);
        let (rb, rv) = get_param(&blob, QG_RBAT_MOHM);
        assert!(ov && rv);
        assert_eq!(ocv, 4_415_900);
        assert_eq!(rb, 84);
        assert_ne!(md5_hex(&blob), CANONICAL_9900_MD5);
    }

    #[test]
    fn kdata_roundtrip_zeros() {
        let buf = [0u8; KDATA_SIZE];
        let k = KernelData::parse(&buf);
        assert_eq!(k.seq_no, 0);
        assert_eq!(k.fifo_length, 0);
        assert_eq!(k.param[QG_SYS_SOC], (0, false));
    }

    #[test]
    fn kdata_parse_header_and_param() {
        let mut buf = [0u8; KDATA_SIZE];
        buf[0..4].copy_from_slice(&7u32.to_le_bytes());
        buf[4..8].copy_from_slice(&100u32.to_le_bytes());
        buf[8..12].copy_from_slice(&2u32.to_le_bytes());
        // fifo[0].v = 4400000
        let f0 = 12;
        buf[f0..f0 + 4].copy_from_slice(&4_400_000u32.to_le_bytes());
        // param SYS_SOC at param_off + 16*8
        let param_off = 12 + MAX_FIFO_LENGTH * FIFO_ENTRY_SIZE;
        let sys_off = param_off + QG_SYS_SOC * PARAM_SIZE;
        buf[sys_off..sys_off + 4].copy_from_slice(&9900u32.to_le_bytes());
        buf[sys_off + 4] = 1;
        let k = KernelData::parse(&buf);
        assert_eq!(k.seq_no, 7);
        assert_eq!(k.fifo_time, 100);
        assert_eq!(k.fifo_length, 2);
        assert_eq!(k.fifo[0].v, 4_400_000);
        assert_eq!(k.param[QG_SYS_SOC], (9900, true));
    }

    #[test]
    fn write_udata_md5_gate() {
        let dir = std::env::temp_dir().join(format!("aginx-fg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dev = dir.join("qg");
        File::create(&dev).unwrap();
        let blob = UdataBuilder::unlock_raw(9900).build();
        let err = write_udata(&dev, &blob, Some("deadbeef"), false).unwrap_err();
        match err {
            FgError::Md5Gate { .. } => {}
            other => panic!("expected Md5Gate, got {other}"),
        }
        write_udata(&dev, &blob, Some(CANONICAL_9900_MD5), false).unwrap();
        let got = std::fs::read(&dev).unwrap();
        assert_eq!(got, blob);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rbat_plausible_gate() {
        assert!(rbat_mohm_plausible(84));
        assert!(rbat_mohm_plausible(1));
        assert!(rbat_mohm_plausible(500));
        assert!(!rbat_mohm_plausible(0));
        assert!(!rbat_mohm_plausible(84000));
        assert!(!rbat_mohm_plausible(16_384_000));
    }

    #[test]
    fn fifo_i_ua_sign() {
        // 4294962414u32 == -4882i32 (earlier Full-window sample)
        assert_eq!(fifo_i_ua(4294962414), -4882);
        assert_eq!(fifo_i_ua_coulomb(4294962414), 4882); // invert
        assert_eq!(fifo_i_ua(5798), 5798);
        assert_eq!(fifo_i_ua_coulomb(5798), -5798);
        // Discharge empirics: positive FIFO → negative coulomb
        assert_eq!(fifo_i_ua_coulomb(100_402), -100_402);
        assert_eq!(fifo_i_ua_coulomb(365_295), -365_295);
    }

    #[test]
    fn coulomb_integrate_discharge_drops_soc() {
        // FIFO stores discharge as **positive** on redfin; coulomb negates. // D14-exempt
        // +1A FIFO → −1A effective → 1_000_000 µAh drain over 1h
        // full=4_187_000 → Δsoco ≈ -10000*1e6/4.187e6 ≈ -2388
        let mut st = CoulombState::new(9900, 4_187_000);
        let mut k = KernelData::parse(&[0u8; KDATA_SIZE]);
        k.fifo_length = 1;
        k.seq_no = 1;
        k.fifo[0] = FifoSample {
            v: 4_000_000,
            i: 1_000_000u32, // +1A in FIFO == discharge
            count: 1,
            interval: 3_600_000, // 1 hour in ms
        };
        let soc = st.integrate(&k);
        assert!(soc < 9900, "soc={soc} should drop after 1Ah discharge");
        assert!(soc >= SOCO_FLOOR);
        assert_eq!(st.samples, 1);
        assert_eq!(fifo_i_ua_coulomb(1_000_000), -1_000_000);
    }

    #[test]
    fn coulomb_integrate_charge_raises_soc() {
        // Charge appears as negative FIFO on this SoC → coulomb flips to +.
        let mut st = CoulombState::new(5000, 4_187_000);
        let mut k = KernelData::parse(&[0u8; KDATA_SIZE]);
        k.fifo_length = 1;
        k.seq_no = 2;
        k.fifo[0] = FifoSample {
            v: 4_200_000,
            i: (-1_000_000i32) as u32, // −1A FIFO == charge
            count: 1,
            interval: 3_600_000,
        };
        let soc = st.integrate(&k);
        assert!(soc > 5000, "soc={soc} should rise after 1Ah charge");
        assert!(soc <= SOCO_FULL);
    }

    /// Observed discharge sample (redfin 2026-10-12, STATUS=Discharging): // D14-exempt
    /// fifo i_ua=+365295 raw, count=128, interval_ms=100 → dt=12800ms.
    /// psy current_now was negative at the same time.
    #[test]
    fn coulomb_empirical_discharge_sample_drains() {
        let mut st = CoulombState::new(10_000, 4_187_000);
        let mut k = KernelData::parse(&[0u8; KDATA_SIZE]);
        k.fifo_length = 1;
        k.seq_no = 9;
        k.fifo[0] = FifoSample {
            v: 4_402_494,
            i: 365_295u32, // positive FIFO while discharging
            count: 128,
            interval: 100,
        };
        assert_eq!(fifo_i_ua(365_295), 365_295);
        assert_eq!(fifo_i_ua_coulomb(365_295), -365_295);
        let soc = st.integrate(&k);
        // duah ≈ 365295*12800/3600000 ≈ 1299 µAh → Δsoco ≈ -3
        assert!(soc < 10_000, "soc={soc} must drop on empirical discharge frame");
        assert!(soc >= 9_995, "soc={soc} drop should be small (~3 raw)");
        assert!(st.delta_uah <= 0, "residual should be non-positive after fold");
        assert_eq!(st.samples, 1);
    }

    #[test]
    fn coulomb_never_emits_zero_and_nails_full() {
        let mut st = CoulombState::new(100, 1_000); // tiny pack
        let mut k = KernelData::parse(&[0u8; KDATA_SIZE]);
        k.fifo_length = 1;
        k.fifo[0] = FifoSample {
            v: 3_000_000,
            i: 2_000_000u32, // huge +FIFO == discharge after invert
            count: 1,
            interval: 3_600_000,
        };
        let soc = st.integrate(&k);
        assert!(soc >= SOCO_FLOOR, "soft floor");
        st.nail_full();
        assert_eq!(st.soco_raw, SOCO_FULL);
        assert_eq!(st.delta_uah, 0);
    }

    #[test]
    fn coulomb_empty_fifo_holds() {
        let mut st = CoulombState::new(9900, 4_187_000);
        let k = KernelData::parse(&[0u8; KDATA_SIZE]);
        assert_eq!(st.integrate(&k), 9900);
        assert_eq!(st.samples, 0);
    }

    #[test]
    fn hold_monotonic_clamps_cc_on_discharge() {
        // Live CC=9200, coulomb thinks 9230 → SYS=9230 CC=9200 (not 9230).
        let blob = hold_udata_monotonic(9230, None, Some(84), true, Some(9200));
        let (sys, sv) = get_param(&blob, QG_SYS_SOC);
        let (cc, cv) = get_param(&blob, QG_CC_SOC);
        assert!(sv && cv);
        assert_eq!(sys, 9230);
        assert_eq!(cc, 9200);
        // Charging: upward CC allowed.
        let blob2 = hold_udata_monotonic(9300, None, Some(84), false, Some(9200));
        let (cc2, _) = get_param(&blob2, QG_CC_SOC);
        assert_eq!(cc2, 9300);
        // Discharge with soco below live: both follow soco.
        let blob3 = hold_udata_monotonic(9100, None, Some(84), true, Some(9200));
        let (sys3, _) = get_param(&blob3, QG_SYS_SOC);
        let (cc3, _) = get_param(&blob3, QG_CC_SOC);
        assert_eq!(sys3, 9100);
        assert_eq!(cc3, 9100);
    }

    #[test]
    fn write_refuses_zero_soc() {
        let dir = std::env::temp_dir().join(format!("aginx-fg-zero-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dev = dir.join("qg");
        File::create(&dev).unwrap();
        let blob = UdataBuilder::unlock_raw(0).build();
        let err = write_udata(&dev, &blob, None, false).unwrap_err();
        match err {
            FgError::SocForbidden { soco_raw: 0 } => {}
            other => panic!("expected SocForbidden, got {other}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
