//! The consumer client: the ~40 lines every CLI embeds to talk to the
//! sidecar (M36 agsecret; N5② 吸收改姓). Deliberately dependency-free
//! beyond serde_json — the wire protocol IS the interface, no shared
//! crate between the daemon and its consumers.
//!
//! Consumer resolution order: real env var > sidecar > `.env` file.
//! This crate only provides the sidecar leg; the caller owns the order.
//! Failures are transport-level (socket gone, malformed reply) and
//! should read as "sidecar absent", never as "secret missing" — a
//! server twin without the sidecar falls back to its env file.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// One request → one envelope. Err means transport failure (no daemon,
/// broken line) — an `{"ok":false}` envelope is a SUCCESSFUL round trip
/// and comes back as Ok.
pub fn request(sock: &Path, req: &Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(sock).map_err(|e| format!("connect {sock:?}: {e}"))?;
    let line = serde_json::to_string(req).map_err(|e| e.to_string())?;
    stream
        .write_all(line.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .and_then(|_| stream.flush())
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = String::new();
    let n = BufReader::new(&mut stream)
        .read_line(&mut buf)
        .map_err(|e| format!("read: {e}"))?;
    if n == 0 {
        return Err("daemon closed without a reply".into());
    }
    serde_json::from_str(buf.trim_end()).map_err(|e| format!("bad reply: {e}"))
}

/// Env-var-style lookup (`op: env`) — the leg `api_key_env`/`secret_env`
/// resolution inserts between the real env and the `.env` file. None
/// covers both "sidecar absent" (transport Err) and "no such mapping"
/// (envelope not_found/denied).
pub fn lookup_env(sock: &Path, name: &str) -> Option<String> {
    let resp = request(sock, &serde_json::json!({"op": "env", "name": name})).ok()?;
    if resp.get("ok").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    resp.get("data")?.get("value")?.as_str().map(str::to_string)
}

/// Default socket, or the `AGINX_SECRET_SOCKET` override (adb dev loop
/// and tests — same pattern as AGINX_CMD_PATH/AGINX_PKG_MANIFEST).
pub fn default_socket() -> PathBuf {
    std::env::var_os("AGINX_SECRET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(crate::DEFAULT_SOCKET))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_override_env() {
        // env mutation → serialized per testkit convention
        let _g = testkit::env_lock();
        // AGINX_SECRET_SOCKET wins when set; the default is the boot socket.
        let d = testkit::tmp("aginx-secret-client-sock");
        let s = d.join("s.sock");
        std::env::set_var("AGINX_SECRET_SOCKET", &s);
        assert_eq!(default_socket(), s);
        std::env::remove_var("AGINX_SECRET_SOCKET");
        assert_eq!(default_socket(), PathBuf::from("/run/aginx/secret.sock"));
    }
}
