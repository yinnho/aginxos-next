//! aginx-secretd — the secret sidecar daemon (M36 agsecret; N5② 吸收
//! 改姓，store 迁 /var/lib/aginx/secret).
//!
//! Flags exist for tests and the adb dev loop; production runs bare from
//! its aginx-svc unit with the default paths. Exits nonzero only on
//! startup problems (bind/store) — the accept loop serves until killed.

use std::path::PathBuf;

fn main() {
    let mut sock = PathBuf::from(aginx_secret::DEFAULT_SOCKET);
    let mut store = PathBuf::from(aginx_secret::DEFAULT_STORE);
    let mut policy = PathBuf::from(aginx_secret::DEFAULT_POLICY);
    let mut log = PathBuf::from(aginx_secret::DEFAULT_LOG);

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--socket" => sock = PathBuf::from(args.next().unwrap_or_default()),
            "--store" => store = PathBuf::from(args.next().unwrap_or_default()),
            "--policy" => policy = PathBuf::from(args.next().unwrap_or_default()),
            "--log" => log = PathBuf::from(args.next().unwrap_or_default()),
            "--help" | "-h" => {
                println!("usage: aginx-secretd [--socket P] [--store P] [--policy P] [--log P]");
                return;
            }
            other => {
                eprintln!("aginx-secretd: unknown flag {other}");
                std::process::exit(2);
            }
        }
    }

    if let Err(e) = aginx_secret::serve::serve(&sock, &store, &policy, &log) {
        eprintln!("aginx-secretd: {e}");
        std::process::exit(1);
    }
}
