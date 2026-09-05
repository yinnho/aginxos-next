//! secret — relay 钥匙解析（N5⑤）：env > sidecar，消费序与
//! aginx-secret client 的注释纪律一致（真 env 是第一腿，本函数 owns
//! env 腿 + sidecar 腿，`.env` 文件腿是宿主试跑的事，不进网关）。
//!
//! 无钥匙不是致命错：main 的等待循环每 5s 重解析——sidecar 可能
//! 晚于网关起来（requires_weak 只在依赖 starting 时等待，不阻塞），
//! 或运维正在 `aginx-secret set relay.primary`。传输层失败���作
//! "sidecar 暂缺"，与 "scope 无值" 同为 None。

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

use serde_json::Value;

/// 宿主测试通道（设备上不放 env，钥匙全在 sidecar）。
pub const ENV_VAR: &str = "AGINX_RELAY_SECRET";
pub const SCOPE: &str = "relay.primary";
pub const SIDECAR_SOCK: &str = "/run/aginx/secret.sock";

pub fn resolve(sock: &Path) -> Option<String> {
    if let Ok(v) = std::env::var(ENV_VAR) {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    sidecar_get(sock)
}

/// 一进一出的 `get relay.primary`（ndjson 线协议即接口，见
/// crates/secret/src/client.rs——不跨 crate 引代码，wire 即契约）。
fn sidecar_get(sock: &Path) -> Option<String> {
    let mut stream = UnixStream::connect(sock).ok()?;
    let req = serde_json::json!({"op": "get", "scope": SCOPE});
    let line = serde_json::to_string(&req).ok()?;
    stream.write_all(line.as_bytes()).ok()?;
    stream.write_all(b"\n").ok()?;
    stream.flush().ok()?;
    let mut buf = String::new();
    let n = BufReader::new(&mut stream).read_line(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    let resp: Value = serde_json::from_str(buf.trim_end()).ok()?;
    if resp.get("ok").and_then(Value::as_bool) != Some(true) {
        return None; // denied（policy 没放行本网关）与 not_found 同待
    }
    resp.pointer("/data/value")?.as_str().map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    // 假 sidecar：接受一连线，回一行信封，关。脚本由 caller 预置。
    fn fake_sidecar(sock: &Path, reply: String) -> std::thread::JoinHandle<()> {
        let listener = UnixListener::bind(sock).unwrap();
        std::thread::spawn(move || {
            if let Ok((mut conn, _)) = listener.accept() {
                let mut line = String::new();
                use std::io::BufRead;
                if BufReader::new(&mut conn).read_line(&mut line).is_ok() {
                    let _ = conn.write_all(reply.as_bytes());
                }
            }
        })
    }

    #[test]
    fn sidecar_get_reads_the_value_out_of_the_envelope() {
        let dir = std::env::temp_dir().join(format!("aginx-gw-sec-ok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s.sock");
        let h = fake_sidecar(&sock, "{\"ok\":true,\"data\":{\"value\":\"topsecret\"}}\n".into());
        std::env::remove_var(ENV_VAR);
        assert_eq!(sidecar_get(&sock), Some("topsecret".into()));
        h.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn denied_and_absent_both_read_as_none() {
        let dir = std::env::temp_dir().join(format!("aginx-gw-sec-no-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("s.sock");
        let h = fake_sidecar(&sock, "{\"ok\":false,\"error\":{\"code\":\"denied\"}}\n".into());
        assert_eq!(sidecar_get(&sock), None);
        h.join().unwrap();
        // 无守护 = None（不是 Err）
        assert_eq!(sidecar_get(&dir.join("missing.sock")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
