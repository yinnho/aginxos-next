// Wi-Fi setup wizard (M10 UI half; N4③ 搬入新仓改姓 aginx-net-wizard).
//
// Runs as a TUI inside aterm's pty (launcher auto-starts it when
// /etc/wifi.conf is missing, and it keeps a "WIFI SETUP" button for
// re-config). Flow: aginx-net-scan -> numbered AP list -> password prompt ->
// write /etc/wifi.conf (0600) -> run net-bringup in the foreground so the
// user sees join/dhcp/internet live (M20b: never a whole-device reboot;
// net-watch owns steady-state healing from here).
//
// ASCII only (v1 has no CJK input): SSIDs the scan renders with '?'
// can't be selected — they are shown but marked unjoinable.
use std::io::{BufRead, Write};
use std::process::Command;

struct Ap {
    ssid: String,
    dbm: f64,
    joinable: bool,
}

// nlscan separates columns with runs of spaces, so plain split(' ') yields
// empty fields — peel off the first four whitespace-separated tokens
// (mac, ch=N, -N.NN, dBm) and treat the remainder (spaces included) as
// the SSID.
fn fields(l: &str) -> Option<(&str, &str, &str, &str)> {
    let mut rest = l.trim_start();
    let mut tok = [""; 4];
    for slot in &mut tok {
        let idx = rest.find(char::is_whitespace)?;
        *slot = &rest[..idx];
        rest = rest[idx..].trim_start();
    }
    Some((tok[0], tok[1], tok[2], rest))
}

fn scan() -> Vec<Ap> {
    let out = Command::new("/usr/bin/aginx-net-scan")
        .arg("wlan0")
        .output()
        .expect("run nlscan");
    let mut aps: Vec<Ap> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let (mac, ch, dbm_s, ssid) = fields(l)?;
            let ssid = ssid.trim_end();
            if mac.len() != 17 || !ch.starts_with("ch=") {
                return None;
            }
            if ssid == "<hidden>" || ssid.is_empty() {
                return None;
            }
            let dbm: f64 = dbm_s.parse().ok()?;
            // nlscan prints '?' for non-ASCII SSID bytes; such a name can't
            // be typed back into wifi-join with the ASCII-only keyboard.
            let joinable = !ssid.contains('?');
            Some(Ap {
                ssid: ssid.to_string(),
                dbm,
                joinable,
            })
        })
        .collect();
    // dedupe by SSID keeping the strongest BSS, then strongest first
    let mut best: Vec<Ap> = Vec::new();
    for ap in aps {
        match best.iter_mut().find(|b| b.ssid == ap.ssid) {
            Some(b) if ap.dbm > b.dbm => *b = ap,
            Some(_) => {}
            None => best.push(ap),
        }
    }
    best.sort_by(|a, b| b.dbm.partial_cmp(&a.dbm).unwrap_or(std::cmp::Ordering::Equal));
    best
}

fn bars(dbm: f64) -> &'static str {
    match dbm as i32 {
        x if x >= -50 => "####",
        x if x >= -60 => "### ",
        x if x >= -70 => "#   ",
        _ => ".   ",
    }
}

fn prompt(text: &str) -> String {
    print!("{text}");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    // EOF (stdin closed) must quit, not loop back into a rescan forever.
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => std::process::exit(0),
        Ok(_) => {}
    }
    line.trim().to_string()
}

fn write_conf(ssid: &str, psk: &str) {
    let conf = format!("ssid={ssid}\npsk={psk}\n");
    std::fs::write("/etc/wifi.conf", conf).expect("write /etc/wifi.conf");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions("/etc/wifi.conf", std::fs::Permissions::from_mode(0o600))
        .expect("chmod /etc/wifi.conf");
}

// net-bringup redirects its own stdout to /var/net.log, so the wizard can't
// just inherit stdio — run it in the background and mirror the boot.state
// verdict lines (wifi/dhcp/internet/time/done) as they land.
fn connect() -> bool {
    let mut pos = std::fs::read_to_string("/run/boot.state").map(|s| s.len()).unwrap_or(0);
    let mut child = match Command::new("/bin/sh").arg("/etc/init.d/net-bringup").spawn() {
        Ok(c) => c,
        Err(e) => {
            println!("cannot start net-bringup: {e}");
            return false;
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(420);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Ok(state) = std::fs::read_to_string("/run/boot.state") {
            if state.len() > pos {
                for line in state[pos..].lines() {
                    println!("  {line}");
                    if line.starts_with("done ok") {
                        let _ = child.wait();
                        return true;
                    }
                    if line.starts_with("done fail") {
                        let _ = child.wait();
                        return false;
                    }
                }
                pos = state.len();
            }
        }
        if matches!(child.try_wait(), Ok(Some(_))) {
            println!("  net-bringup exited without a verdict");
            return false;
        }
        if std::time::Instant::now() > deadline {
            println!("  timed out");
            let _ = child.kill();
            return false;
        }
    }
}

fn main() {
    println!("=========================================");
    println!("  AginxOS Wi-Fi Setup");
    println!("=========================================");
    loop {
        println!();
        println!("scanning wlan0 ...");
        let aps = scan();
        if aps.is_empty() {
            println!("no networks found.");
        }
        for (i, ap) in aps.iter().enumerate() {
            let mark = if ap.joinable { "" } else { "  [non-ascii name]" };
            println!("{:>2}) {:<24} {:>5} dBm  [{}]{mark}", i + 1, ap.ssid, ap.dbm, bars(ap.dbm));
        }
        let pick = prompt("number + Enter (r=rescan, q=quit): ");
        match pick.as_str() {
            "q" | "Q" => return,
            "r" | "R" | "" => continue,
            n => match n.parse::<usize>().map(|v| v.checked_sub(1)) {
                Ok(Some(i)) if i < aps.len() => {
                    let ap = &aps[i];
                    if !ap.joinable {
                        println!("that SSID has non-ascii characters (v1 keyboard is ASCII-only).");
                        continue;
                    }
                    let psk = prompt(&format!("password for '{}': ", ap.ssid));
                    if psk.is_empty() {
                        println!("open networks are not supported yet — need a WPA2 password.");
                        continue;
                    }
                    write_conf(&ap.ssid, &psk);
                    println!();
                    println!("--- connecting (join -> dhcp -> internet check) ---");
                    if connect() {
                        println!();
                        println!("network is up.");
                        // No reboot (M20b) and no unit nudges (N4 切净:
                        // relay/carrier 不再随镜像——net-watch owns
                        // steady-state healing from here).
                        return;
                    }
                    println!();
                    println!("connection failed — wrong password or AP unreachable.");
                    let _ = std::fs::remove_file("/etc/wifi.conf");
                }
                _ => println!("no such number."),
            },
        }
    }
}
