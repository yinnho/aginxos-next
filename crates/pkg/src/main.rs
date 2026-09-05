//! aginx-pkg — AginxOS package installer CLI (M26 agpkg, N4③b 改姓).
//!
//! Thin face over the lib: v0 subcommands with v0 exit codes (usage = 2),
//! human stdout by default, D1 envelope on `--json` for the query
//! commands (list / available). Action commands (install / sync /
//! opt-in / rollback) print progress lines — provision and the adb dev
//! loop key off the exit code, not stdout.

use std::path::Path;
use std::process::exit;

use aginx_pkg::{cmd_available, cmd_list, cmd_opt_in, cmd_rollback, cmd_sync, install_file, usage, Fail, Paths};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let p = Paths::from_env();
    // one key, one chain: same pubkey agupd verifies updates with.
    let pubkey = aginx_sign::AGINX_PUBKEY_B64;

    let cmd = args.first().map(String::as_str).unwrap_or_default();
    let json = args.iter().any(|a| a == "--json");
    let rest: Vec<&str> = args.iter().skip(1).filter(|a| *a != "--json").map(String::as_str).collect();

    match cmd {
        "--help" | "-h" => {
            println!("{}", usage());
            println!("\nmanifest: {} (detached ed25519 sig at .sig required; explicit", p.manifest.display());
            println!("path argument or AGPKG_MANIFEST = dev override, unsigned)");
            exit(0);
        }
        "install" => {
            let (name, src, sha) = match rest.as_slice() {
                [n, s, h] => (*n, *s, *h),
                _ => die_usage(),
            };
            match install_file(&p, name, Path::new(src), sha) {
                Ok(aginx_pkg::Kind::Binary) => println!("aginx-pkg: installed {name} ({sha})"),
                Ok(aginx_pkg::Kind::Bundle { unit, .. }) => {
                    println!("aginx-pkg: installed {name} bundle ({sha}) — skill{}", if unit { " + unit" } else { "" })
                }
                Err(f) => fail(f, json),
            }
        }
        "sync" => {
            let mf = rest.first().map(Path::new);
            exit(cmd_sync(&p, mf, pubkey).unwrap_or_else(|f| fail(f, json)));
        }
        "available" => {
            let mf = rest.first().map(Path::new);
            match cmd_available(&p, mf, pubkey) {
                Ok(out) => print_out(out, json),
                Err(f) => fail(f, json),
            }
        }
        "opt-in" => {
            let name = match rest.as_slice() {
                [n] => *n,
                _ => die_usage(),
            };
            if let Err(f) = cmd_opt_in(&p, name, pubkey) {
                fail(f, json);
            }
        }
        "rollback" => {
            let name = match rest.as_slice() {
                [n] => *n,
                _ => die_usage(),
            };
            if let Err(f) = cmd_rollback(&p, name) {
                fail(f, json);
            }
        }
        "list" => match cmd_list(&p) {
            Ok(out) => print_out(out, json),
            Err(f) => fail(f, json),
        },
        _ => die_usage(),
    }
}

fn die_usage() -> ! {
    eprintln!("{}", usage());
    exit(2);
}

/// Print the query result: human lines, or the D1 envelope on --json.
fn print_out(out: aginx_pkg::CmdOut, json: bool) {
    if json {
        println!("{}", serde_json::to_string(&agio::ok_meta(serde_json::Value::Array(out.data), out.meta)).unwrap());
        return;
    }
    for l in &out.lines {
        println!("{l}");
    }
    // empty result prints nothing (v0 behavior — the aginx-term "+" tile
    // reads these stdout lines).
}

/// Fail path: envelope on --json, aginx-pkg-prefixed stderr + exit 1/2
/// otherwise (usage failures keep the v0 exit 2).
fn fail(f: Fail, json: bool) -> ! {
    if json {
        println!("{}", serde_json::to_string(&f.envelope()).unwrap());
        exit(f.etype.exit_code());
    }
    eprintln!("aginx-pkg: {}", f.message);
    if let Some(h) = &f.hint {
        eprintln!("  hint: {h}");
    }
    exit(f.etype.exit_code());
}
