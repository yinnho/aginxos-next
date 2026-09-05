// aginx-svc — control client for the supervisor (M16, docs/SYSTEM.md
// §12.2). One command per connection over /run/svc/ctl.sock; `logs` tails
// the unit's log file locally instead of round-tripping it.
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use aginx_svc::{log_path, CTL_SOCK};

const USAGE: &str = "usage: aginx-svc list | status <name> | start|stop|restart <name> | reload | logs <name>";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let name = args.get(1).map(String::as_str).unwrap_or("");

    match cmd {
        "list" | "status" | "start" | "stop" | "restart" | "reload" => {}
        "logs" => {
            if name.is_empty() {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
            std::process::exit(tail(&log_path(name)));
        }
        _ => {
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    }

    let req = match cmd {
        "list" | "reload" => cmd.to_string(),
        _ => {
            if name.is_empty() {
                eprintln!("{USAGE}");
                std::process::exit(2);
            }
            format!("{cmd} {name}")
        }
    };

    let mut s = match UnixStream::connect(CTL_SOCK) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("aginx-svc: connect {CTL_SOCK}: {e} (is aginx-svcd running?)");
            std::process::exit(1);
        }
    };
    if let Err(e) = s.write_all(format!("{req}\n").as_bytes()) {
        eprintln!("aginx-svc: write: {e}");
        std::process::exit(1);
    }
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut resp = String::new();
    if let Err(e) = s.read_to_string(&mut resp) {
        eprintln!("aginx-svc: read: {e}");
        std::process::exit(1);
    }
    let ok = resp.trim_end().ends_with("OK");
    let body = resp.trim_end().trim_end_matches("\nOK").trim_end();
    if !body.is_empty() {
        println!("{body}");
    }
    if !ok {
        // ERR line already printed via body
        std::process::exit(1);
    }
}

/// Tail the last 40 lines — busybox `tail` works too, but keeping it here
/// means one less PATH dependency in scripts.
fn tail(path: &str) -> i32 {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("aginx-svc: read {path}: {e}");
            return 1;
        }
    };
    let text = String::from_utf8_lossy(&data);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(40);
    for l in &lines[start..] {
        println!("{l}");
    }
    0
}
