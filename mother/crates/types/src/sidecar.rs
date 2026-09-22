//! The secret sidecar client leg (M36, DECISIONS §9 on the OS side).
//!
//! Hand-rolled on purpose — ~50 lines of std UnixStream + serde_json. The
//! agsecretd wire protocol (ndjson: one request line in, one agio envelope
//! line out) IS the interface; the carrier is a separate repo from the
//! daemon crate and must not grow a code dependency on it.
//!
//! Every failure mode reads as `None`: socket absent, daemon wedged past the
//! read timeout, denied by policy, no env mapping, malformed reply. A miss
//! is never a hard error — the resolution order (env > sidecar > absent)
//! degrades to exactly the pre-sidecar behavior when the sidecar isn't
//! running, which is the case on dev hosts and pre-M36 devices.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

/// Boot socket of agsecretd on the phone.
const DEFAULT_SOCKET: &str = "/run/aginx/secret.sock";

/// Bound on one round trip. The carrier calls this during daemon boot
/// (Brain::new) and per `ag api` invocation — a wedged daemon must not hang
/// either; 2s is far above a healthy local unix-socket round trip.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// The sidecar socket: `AGSECRET_SOCKET` override (adb dev loop, tests)
/// over the boot default. Read through [`crate::env::get_env`] so the
/// in-process override map works the same as every other env knob.
pub fn socket() -> PathBuf {
    crate::env::get_env("AGSECRET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET))
}

/// Env-var-style lookup against the sidecar (`{"op":"env","name":…}`).
/// `None` covers both "sidecar absent" and "no secret for this caller" —
/// callers treat it as the gap-filler between real env and absent.
pub fn env_at(sock: &std::path::Path, name: &str) -> Option<String> {
    let mut stream = UnixStream::connect(sock).ok()?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok()?;
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok()?;

    let line = serde_json::to_string(&serde_json::json!({"op": "env", "name": name})).ok()?;
    stream.write_all(line.as_bytes()).ok()?;
    stream.write_all(b"\n").ok()?;
    stream.flush().ok()?;

    let mut buf = String::new();
    // A reply bigger than 64 KiB is not an envelope we want — treat as absent.
    let mut capped = BufReader::new(&mut stream).take(64 * 1024 + 1);
    let n = capped.read_line(&mut buf).ok()?;
    if n == 0 || n > 64 * 1024 {
        return None;
    }
    let resp: Value = serde_json::from_str(buf.trim_end()).ok()?;
    if resp.get("ok").and_then(Value::as_bool) != Some(true) {
        return None; // not_found / denied / bad_request — all just "no"
    }
    resp.get("data")?.get("value")?.as_str().map(str::to_string)
}

/// `env_at` against the configured socket — the shape every consumer uses.
pub fn env_lookup(name: &str) -> Option<String> {
    env_at(&socket(), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AGSECRET_SOCKET lives in the process-global override map — the two
    /// tests that touch it serialize on this lock (same idea as the OS
    /// repo's testkit::env_lock).
    static SOCK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Unique scratch socket per test — no shared state, no env lock needed.
    fn tmp_sock(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("carrier-sidecar-{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("s.sock")
    }

    /// One-shot stub daemon: accept loop replying with `reply` to every
    /// connection (one line in, one line out — the real wire shape).
    fn stub_daemon(sock: &std::path::Path, reply: &'static str) {
        let listener = std::os::unix::net::UnixListener::bind(sock).unwrap();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { continue };
                let mut line = String::new();
                if BufReader::new(&mut stream).read_line(&mut line).is_err() {
                    continue;
                }
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.write_all(b"\n");
                let _ = stream.flush();
            }
        });
    }

    #[test]
    fn happy_path_returns_value() {
        let s = tmp_sock("happy");
        stub_daemon(
            &s,
            r#"{"ok":true,"data":{"scope":"brain.primary","value":"sk-sidecar-1"}}"#,
        );
        assert_eq!(env_at(&s, "AGINXBRAIN_API_KEY").as_deref(), Some("sk-sidecar-1"));
    }

    #[test]
    fn denied_and_not_found_read_as_absent() {
        let s = tmp_sock("denied");
        stub_daemon(
            &s,
            r#"{"ok":false,"error":{"type":"auth","code":"denied","message":"no"}}"#,
        );
        assert_eq!(env_at(&s, "CHARTER_SK"), None);

        let s2 = tmp_sock("notfound");
        stub_daemon(
            &s2,
            r#"{"ok":false,"error":{"type":"not_found","code":"not_found","message":"no"}}"#,
        );
        assert_eq!(env_at(&s2, "CHARTER_SK"), None);
    }

    #[test]
    fn absent_socket_and_garbage_read_as_absent() {
        let s = tmp_sock("nolistener"); // bound by no one
        assert_eq!(env_at(&s, "X"), None);

        let s2 = tmp_sock("garbage");
        stub_daemon(&s2, "this is not json");
        assert_eq!(env_at(&s2, "X"), None);
    }

    #[test]
    fn socket_override_via_env_map() {
        let _g = SOCK_LOCK.lock().unwrap();
        let d = std::env::temp_dir().join("carrier-sidecar-override");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let s = d.join("s.sock");
        // positive leg only — the override map has no remove, and the
        // default leg is covered by socket() returning DEFAULT_SOCKET
        // whenever no override is set.
        crate::env::set_env_override("AGSECRET_SOCKET", s.to_str().unwrap());
        assert_eq!(socket(), s);
    }

    #[test]
    fn get_secret_order_env_then_sidecar() {
        let _g = SOCK_LOCK.lock().unwrap();
        use crate::env::{get_secret, set_env_override};
        let s = tmp_sock("order");
        stub_daemon(
            &s,
            r#"{"ok":true,"data":{"scope":"test.scope","value":"from-sidecar"}}"#,
        );
        crate::env::set_env_override("AGSECRET_SOCKET", s.to_str().unwrap());

        // 1. env leg set → wins over the sidecar value
        set_env_override("CARRIER_TEST_ORDER_KEY_41", "from-env");
        assert_eq!(get_secret("CARRIER_TEST_ORDER_KEY_41").as_deref(), Some("from-env"));

        // 2. env leg absent → sidecar fills the gap
        assert_eq!(get_secret("CARRIER_TEST_ORDER_KEY_42").as_deref(), Some("from-sidecar"));

        // 3. both absent → None
        // (CARRIER_TEST_ORDER_KEY_43 has no env and no policy mapping in
        // the stub — but the stub replies ok:true for ANY name, so use a
        // name whose value the stub can't know: assert via a fresh stub
        // that denies instead.)
        let s2 = tmp_sock("order-deny");
        stub_daemon(
            &s2,
            r#"{"ok":false,"error":{"type":"auth","code":"denied","message":"no"}}"#,
        );
        crate::env::set_env_override("AGSECRET_SOCKET", s2.to_str().unwrap());
        assert_eq!(get_secret("CARRIER_TEST_ORDER_KEY_43"), None);
    }
}
