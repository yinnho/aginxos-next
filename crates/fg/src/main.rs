//! aginx-fg — CLI for qpnp-qg udata unlock + minimal FG daemon.
//!
//! Subcommands:
//!   pack            build 144B blob → file + print md5/describe
//!   check-canonical assert default 9900 blob md5
//!   unlock          write udata to /dev/qg (size+md5+bms gates; never SOC=0)
//!   run             FG daemon: FIFO coulomb → SYS/CC udata (OCV/RBAT care)
//!   read-kdata      one-shot nonblocking kdata dump (if FIFO ready)
//!
//! Product unlock path is this binary — not the research Python script.

use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use aginx_fg::{
    battery_is_discharging, battery_is_full, describe, fifo_i_ua, fifo_i_ua_coulomb, hold_udata,
    md5_hex, open_qg, read_battery_status, read_bms_capacity, read_full_uah, read_ocv_rbat_care,
    try_read_kdata, write_udata, write_udata_fd, CoulombState, KernelData, UdataBuilder,
    BMS_CAPACITY, BMS_CC_SOC, BMS_SYS_SOC, CANONICAL_9900_MD5, DEFAULT_QG_DEV, KDATA_SIZE,
    QG_CC_SOC, QG_SYS_SOC, SOCO_FLOOR, SOCO_FULL, UDATA_SIZE,
};

fn usage() -> ExitCode {
    eprintln!(
        "usage: aginx-fg <command> [args]
  pack [--soc N] [--sys N] [--cc N] [--ocv-uv N] [--rbat-mohm N] [--out PATH]
  check-canonical
  unlock [--soc N] [--sys N] [--cc N] [--ocv-uv N] [--rbat-mohm N]
         [--dev PATH] [--expect-md5 HEX] [--fg-reset] [--bin PATH]
  run [--dev PATH] [--interval-secs N] [--once]
  read-kdata [--dev PATH]

SOC is soco_raw = percent×100 (9900 = 99%). unlock refuses SOC=0 and bms<1.
Canonical 9900 md5: {CANONICAL_9900_MD5}"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        return usage();
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "pack" => cmd_pack(&args),
        "check-canonical" => cmd_check_canonical(&args),
        "unlock" => cmd_unlock(&args),
        "run" => cmd_run(&args),
        "read-kdata" => cmd_read_kdata(&args),
        _ => {
            eprintln!("aginx-fg: unknown command {cmd}");
            usage()
        }
    }
}

#[derive(Default)]
struct BuildArgs {
    soc: Option<u32>,
    sys: Option<u32>,
    cc: Option<u32>,
    ocv_uv: Option<u32>,
    rbat_mohm: Option<u32>,
    out: Option<PathBuf>,
    dev: Option<PathBuf>,
    expect_md5: Option<String>,
    fg_reset: bool,
    bin: Option<PathBuf>,
    interval_secs: u64,
    once: bool,
}

fn parse_build(args: &[String]) -> Result<BuildArgs, String> {
    let mut ba = BuildArgs {
        interval_secs: 30,
        ..BuildArgs::default()
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--soc" => {
                ba.soc = Some(parse_u32(next(args, &mut i, "--soc")?)?);
            }
            "--sys" | "--sys-soc" => {
                ba.sys = Some(parse_u32(next(args, &mut i, "--sys")?)?);
            }
            "--cc" | "--cc-soc" => {
                ba.cc = Some(parse_u32(next(args, &mut i, "--cc")?)?);
            }
            "--ocv-uv" => {
                ba.ocv_uv = Some(parse_u32(next(args, &mut i, "--ocv-uv")?)?);
            }
            "--rbat-mohm" => {
                ba.rbat_mohm = Some(parse_u32(next(args, &mut i, "--rbat-mohm")?)?);
            }
            "--out" => {
                ba.out = Some(PathBuf::from(next(args, &mut i, "--out")?));
            }
            "--dev" => {
                ba.dev = Some(PathBuf::from(next(args, &mut i, "--dev")?));
            }
            "--expect-md5" => {
                ba.expect_md5 = Some(next(args, &mut i, "--expect-md5")?.to_string());
            }
            "--bin" => {
                ba.bin = Some(PathBuf::from(next(args, &mut i, "--bin")?));
            }
            "--fg-reset" => ba.fg_reset = true,
            "--once" => ba.once = true,
            "--interval-secs" => {
                ba.interval_secs = parse_u32(next(args, &mut i, "--interval-secs")?)? as u64;
            }
            other => return Err(format!("unknown flag {other}")),
        }
        i += 1;
    }
    Ok(ba)
}

fn next<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
    *i += 1;
    args.get(*i)
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn parse_u32(s: &str) -> Result<u32, String> {
    s.parse()
        .map_err(|_| format!("not a u32: {s}"))
}

fn builder_from(ba: &BuildArgs, default_soc: u32) -> UdataBuilder {
    let base = ba.soc.unwrap_or(default_soc);
    UdataBuilder {
        sys_soc: Some(ba.sys.unwrap_or(base)),
        cc_soc: Some(ba.cc.unwrap_or(base)),
        ocv_uv: ba.ocv_uv,
        rbat_mohm: ba.rbat_mohm,
    }
}

fn cmd_pack(args: &[String]) -> ExitCode {
    let ba = match parse_build(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("aginx-fg pack: {e}");
            return usage();
        }
    };
    let blob = builder_from(&ba, 9900).build();
    print!("{}", describe(&blob));
    if let Some(path) = &ba.out {
        match File::create(path).and_then(|mut f| f.write_all(&blob)) {
            Ok(()) => eprintln!("wrote {} ({} bytes)", path.display(), UDATA_SIZE),
            Err(e) => {
                eprintln!("aginx-fg pack: write {}: {e}", path.display());
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

fn cmd_check_canonical(args: &[String]) -> ExitCode {
    if !args.is_empty() {
        eprintln!("aginx-fg check-canonical: no args expected");
        return usage();
    }
    let blob = UdataBuilder::unlock_raw(9900).build();
    let got = md5_hex(&blob);
    if got != CANONICAL_9900_MD5 {
        eprintln!("CANONICAL md5 mismatch: {got} != {CANONICAL_9900_MD5}");
        return ExitCode::from(2);
    }
    println!("canonical md5 OK ({CANONICAL_9900_MD5})");
    print!("{}", describe(&blob));
    ExitCode::SUCCESS
}

fn cmd_unlock(args: &[String]) -> ExitCode {
    let ba = match parse_build(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("aginx-fg unlock: {e}");
            return usage();
        }
    };
    let blob = if let Some(bin) = &ba.bin {
        match std::fs::read(bin) {
            Ok(b) if b.len() == UDATA_SIZE => {
                let mut arr = [0u8; UDATA_SIZE];
                arr.copy_from_slice(&b);
                arr
            }
            Ok(b) => {
                eprintln!(
                    "aginx-fg unlock: bin size {} != {UDATA_SIZE}",
                    b.len()
                );
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("aginx-fg unlock: read {}: {e}", bin.display());
                return ExitCode::FAILURE;
            }
        }
    } else {
        // Prefer matching live BMS when --soc omitted (healthy re-stamp).
        let default_soc = match read_bms_capacity(BMS_CAPACITY) {
            Ok(c) if c >= 1 => (c as u32) * 100,
            Ok(c) => {
                eprintln!("aginx-fg unlock: bms capacity={c}; pass --soc explicitly if intentional");
                return ExitCode::from(3);
            }
            Err(_) => ba.soc.unwrap_or(9900),
        };
        builder_from(&ba, default_soc).build()
    };

    print!("{}", describe(&blob));
    let dev = ba
        .dev
        .unwrap_or_else(|| PathBuf::from(DEFAULT_QG_DEV));
    let expect = ba.expect_md5.as_deref().or_else(|| {
        let (sys, _) = aginx_fg::get_param(&blob, QG_SYS_SOC);
        let (cc, _) = aginx_fg::get_param(&blob, QG_CC_SOC);
        if sys == 9900 && cc == 9900 && ba.ocv_uv.is_none() && ba.rbat_mohm.is_none() && ba.bin.is_none()
        {
            Some(CANONICAL_9900_MD5)
        } else {
            None
        }
    });

    if let Ok(c) = read_bms_capacity(BMS_CAPACITY) {
        eprintln!("pre BMS={c}");
    }
    match write_udata(&dev, &blob, expect, ba.fg_reset) {
        Ok(()) => {
            eprintln!(
                "wrote {} bytes to {} md5={}",
                UDATA_SIZE,
                dev.display(),
                md5_hex(&blob)
            );
            if ba.fg_reset {
                eprintln!("fg_reset=1");
            }
            // best-effort readback
            let _ = print_bms_snapshot("post");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("aginx-fg unlock: {e}");
            ExitCode::FAILURE
        }
    }
}

fn print_bms_snapshot(tag: &str) -> std::io::Result<()> {
    let bms = std::fs::read_to_string(BMS_CAPACITY).unwrap_or_else(|_| "?".into());
    let sys = std::fs::read_to_string(BMS_SYS_SOC).unwrap_or_else(|_| "?".into());
    let cc = std::fs::read_to_string(BMS_CC_SOC).unwrap_or_else(|_| "?".into());
    eprintln!(
        "{tag} BMS={} SYS={} CC={}",
        bms.trim(),
        sys.trim(),
        cc.trim()
    );
    Ok(())
}

fn cmd_run(args: &[String]) -> ExitCode {
    let ba = match parse_build(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("aginx-fg run: {e}");
            return usage();
        }
    };
    let dev_path = ba
        .dev
        .unwrap_or_else(|| PathBuf::from(DEFAULT_QG_DEV));

    // Seed from live BMS (must be healthy). Never start at 0.
    let seed_pct = match read_bms_capacity(BMS_CAPACITY) {
        Ok(c) if c >= 1 => c as u32,
        Ok(c) => {
            eprintln!("aginx-fg run: refusing start, bms={c}");
            return ExitCode::from(3);
        }
        Err(e) => {
            eprintln!("aginx-fg run: bms unreadable: {e}");
            return ExitCode::FAILURE;
        }
    };
    let full_uah = read_full_uah();
    let mut cc = CoulombState::new(seed_pct * 100, full_uah);
    eprintln!(
        "aginx-fg run: coulomb seed soco_raw={} full_uah={full_uah} dev={} interval={}s",
        cc.soco_raw,
        dev_path.display(),
        ba.interval_secs
    );

    let mut qg = match open_qg(&dev_path, true) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("aginx-fg run: open {}: {e}", dev_path.display());
            return ExitCode::FAILURE;
        }
    };

    loop {
        // Drain every ready kdata frame and integrate. Empty frames
        // (fifo_length==0 / seq noise) are ignored — no integrate count.
        let mut frames = 0u32;
        let mut skipped_empty = 0u32;
        loop {
            match try_read_kdata(&mut qg) {
                Ok(Some(k)) => {
                    if k.fifo_length == 0 {
                        skipped_empty += 1;
                        eprintln!(
                            "kdata seq={} fifo_len=0 fifo_time={} — empty, skip integrate",
                            k.seq_no, k.fifo_time
                        );
                        continue;
                    }
                    frames += 1;
                    let before = cc.soco_raw;
                    let after = cc.integrate(&k);
                    eprintln!(
                        "kdata seq={} fifo_len={} fifo_time={} soco {before}->{after} delta_uah={} (samples={})",
                        k.seq_no, k.fifo_length, k.fifo_time, cc.delta_uah, cc.samples
                    );
                    log_fifo_brief(&k);
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("aginx-fg run: kdata: {e}");
                    break;
                }
            }
        }
        if frames == 0 {
            if skipped_empty > 0 {
                eprintln!("kdata: {skipped_empty} empty frame(s), no integrate this tick");
            } else {
                eprintln!("kdata: no frame this tick (FIFO idle)");
            }
        }

        let status = read_battery_status().unwrap_or_else(|| "?".into());
        // Full nail ONLY on true STATUS=Full. Never nail because capacity
        // still reads 100 while already Discharging (post-unplug rl lag).
        if battery_is_full() && !battery_is_discharging() && cc.soco_raw != SOCO_FULL {
            eprintln!("STATUS=Full → nail soco_raw {} -> {SOCO_FULL}", cc.soco_raw);
            cc.nail_full();
        } else if battery_is_discharging() && cc.soco_raw == SOCO_FULL && frames > 0 {
            eprintln!("STATUS=Discharging at soco=FULL — coulomb may drain (no nail)");
        }
        // Soft floor (integrate already clamps; belt-and-suspenders).
        if cc.soco_raw < SOCO_FLOOR {
            cc.soco_raw = SOCO_FLOOR;
        }

        let (ocv, rbat) = read_ocv_rbat_care(ba.ocv_uv, ba.rbat_mohm);
        let blob = hold_udata(cc.soco_raw, ocv, rbat);
        match write_udata_fd(&mut qg, &blob) {
            Ok(()) => eprintln!(
                "coulomb stamp status={status} soco_raw={} ocv={ocv:?} rbat={rbat:?} md5={}",
                cc.soco_raw,
                md5_hex(&blob)
            ),
            Err(e) => {
                eprintln!("aginx-fg run: write: {e}");
                return ExitCode::FAILURE;
            }
        }
        let _ = print_bms_snapshot("post");

        if ba.once {
            return ExitCode::SUCCESS;
        }
        thread::sleep(Duration::from_secs(ba.interval_secs.max(1)));
    }
}


fn log_fifo_brief(k: &KernelData) {
    let n = (k.fifo_length as usize).min(aginx_fg::MAX_FIFO_LENGTH);
    for (i, s) in k.fifo.iter().take(n).enumerate() {
        let i_raw = fifo_i_ua(s.i);
        let i_cc = fifo_i_ua_coulomb(s.i);
        eprintln!(
            "  fifo[{i}] v={} i_fifo={i_raw} i_coulomb={i_cc} (raw={}) count={} interval_ms={} dt_ms={}",
            s.v,
            s.i,
            s.count,
            s.interval,
            (s.count as u64) * (s.interval as u64)
        );
    }
    let (sys, sv) = k.param[QG_SYS_SOC];
    let (cc, cv) = k.param[QG_CC_SOC];
    eprintln!("  kparam SYS={sys}/{sv} CC={cc}/{cv}");
}

fn cmd_read_kdata(args: &[String]) -> ExitCode {
    let ba = match parse_build(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("aginx-fg read-kdata: {e}");
            return usage();
        }
    };
    let dev_path = ba
        .dev
        .unwrap_or_else(|| PathBuf::from(DEFAULT_QG_DEV));
    let mut qg = match open_qg(&dev_path, true) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("aginx-fg read-kdata: open {}: {e}", dev_path.display());
            return ExitCode::FAILURE;
        }
    };
    match try_read_kdata(&mut qg) {
        Ok(None) => {
            eprintln!("no kdata ready (need FIFO IRQ / stock daemon peer); expect size {KDATA_SIZE}");
            ExitCode::SUCCESS
        }
        Ok(Some(k)) => {
            eprintln!(
                "kdata seq={} fifo_time={} fifo_length={}",
                k.seq_no, k.fifo_time, k.fifo_length
            );
            log_fifo_brief(&k);
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("aginx-fg read-kdata: {e}");
            ExitCode::FAILURE
        }
    }
}
