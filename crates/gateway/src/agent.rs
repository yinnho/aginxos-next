//! agent — 第 1 层收口桥（N5⑤）：外部 JSON-RPC → aginx-server 的 UDS 前台。
//!
//! v1 方法面（其余一律 -32601）：
//! - `initialize` → 定向应答（protocolVersion 恒整数 1，ACP.md §2.1/§7；
//!   authenticated:false——本网关的鉴权是第 0 层 relay_secret 单门，
//!   外部层无 per-client 鉴权态）。
//! - `prompt` → spawn_blocking 走 UDS 一问一答：
//!   `{"op":"send","avatar"?,"text"}` 一行进，agio 信封一行出。
//!   ok → **恰一条** chunk 广播裸行 + 无 id 终帧 `result.stopReason:
//!   endTurn`（agc 只从 chunk 通知读文本——单 chunk 语义 = 完整回复）。
//!   unknown_avatar → -32601（server 的消息自带化身名）；其余失败
//!   （UDS 不通/超时/轮失败）→ -32603。
//!
//! 闸门 110s < agc 的 RPC_TIMEOUT 120s：超时先于客户端给出错误帧，
//! 不留悬死等待。

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use crate::relay::Wire;

pub const SERVER_SOCK: &str = "/run/aginx.sock";

pub struct AgentBridge {
    pub sock: PathBuf,
    pub turn_timeout: Duration,
}

/// prompt 失败的二分：只有 unknown_avatar 是 -32601，其余全 -32603
/// （ACP.md §5 外部错误表）。
#[derive(Debug, PartialEq)]
pub enum PromptError {
    UnknownAvatar(String),
    Other(String),
}

pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": 1,
        "authenticated": false,
        "serverInfo": {"name": "aginx-gateway", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// 成功轮的两条出网帧：先 chunk 后终帧，顺序是契约（客户端拼接即
/// 最终文本，终帧一到即收）。
pub fn turn_frames_ok(text: &str) -> Vec<Wire> {
    vec![
        Wire::Broadcast(json!({"jsonrpc": "2.0", "method": "chunk", "params": {"text": text}})),
        Wire::Broadcast(json!({"jsonrpc": "2.0", "result": {"stopReason": "endTurn"}})),
    ]
}

pub fn error_frame(id: &Value, code: i64, message: &str) -> Wire {
    Wire::Directed(json!({
        "jsonrpc": "2.0", "id": id,
        "error": {"code": code, "message": message},
    }))
}

#[async_trait::async_trait]
impl crate::relay::Bridge for AgentBridge {
    async fn on_data(&self, _client_id: &str, data: Value) -> Vec<Wire> {
        let id = data.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = data.get("method").and_then(Value::as_str) else {
            return vec![error_frame(&id, -32700, "not a json-rpc request")];
        };
        match method {
            "initialize" => vec![Wire::Directed(json!({
                "jsonrpc": "2.0", "id": id, "result": initialize_result(),
            }))],
            "prompt" => self.handle_prompt(&id, &data).await,
            other => vec![error_frame(
                &id,
                -32601,
                &format!("method '{other}' is not implemented in aginx-gateway v1"),
            )],
        }
    }
}

impl AgentBridge {
    async fn handle_prompt(&self, id: &Value, req: &Value) -> Vec<Wire> {
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let text = params.get("message").and_then(Value::as_str).map(str::trim).unwrap_or("");
        if text.is_empty() {
            return vec![error_frame(id, -32602, "prompt needs non-empty message")];
        }
        let avatar = params
            .get("agent")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty());
        match self.uds_turn(avatar, text).await {
            Ok(reply) => turn_frames_ok(&reply),
            Err(PromptError::UnknownAvatar(msg)) => vec![error_frame(id, -32601, &msg)],
            Err(PromptError::Other(msg)) => vec![error_frame(id, -32603, &msg)],
        }
    }

    /// 一轮：UDS 连线里同步跑完（server 的前台一次一轮，真 brain 可以
    /// 跑几分钟——闸门兜底）。std 流自带 read_timeout 作硬界，外层
    /// tokio timeout 先到也行（闸门 < 客户端 120s，错误帧先出门）。
    async fn uds_turn(&self, avatar: Option<&str>, text: &str) -> Result<String, PromptError> {
        let sock = self.sock.clone();
        let avatar = avatar.map(str::to_string);
        let text = text.to_string();
        let gate = self.turn_timeout;
        let task = tokio::task::spawn_blocking(move || uds_roundtrip(&sock, avatar.as_deref(), &text, gate));
        // +1s 宽限：让 std 侧的 read_timeout 先触发，语义更准。
        match tokio::time::timeout(gate + Duration::from_secs(1), task).await {
            Ok(Ok(inner)) => inner,
            Ok(Err(e)) => Err(PromptError::Other(format!("turn task died: {e}"))),
            Err(_) => Err(PromptError::Other(format!("turn gate {gate:?} exceeded"))),
        }
    }
}

fn uds_roundtrip(
    sock: &std::path::Path,
    avatar: Option<&str>,
    text: &str,
    read_timeout: Duration,
) -> Result<String, PromptError> {
    let mut stream = UnixStream::connect(sock)
        .map_err(|e| PromptError::Other(format!("aginx-server unreachable ({e})")))?;
    stream
        .set_read_timeout(Some(read_timeout))
        .map_err(|e| PromptError::Other(format!("set timeout: {e}")))?;

    let mut req = json!({"op": "send", "text": text});
    if let Some(a) = avatar {
        req["avatar"] = json!(a);
    }
    use std::io::{BufRead, BufReader, Write};
    let line = serde_json::to_string(&req).map_err(|e| PromptError::Other(e.to_string()))?;
    stream
        .write_all(line.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .and_then(|_| stream.flush())
        .map_err(|e| PromptError::Other(format!("uds write: {e}")))?;

    let mut buf = Vec::with_capacity(512);
    let mut reader = BufReader::new(&mut stream);
    reader
        .read_until(b'\n', &mut buf)
        .map_err(|e| PromptError::Other(format!("uds read: {e}")))?;
    if buf.len() > crate::relay::MAX_LINE {
        return Err(PromptError::Other("reply exceeds line cap".into()));
    }
    while matches!(buf.last(), Some(b'\n') | Some(b'\r')) {
        buf.pop();
    }
    let env: Value = serde_json::from_slice(&buf)
        .map_err(|e| PromptError::Other(format!("bad envelope: {e}")))?;

    if env.get("ok").and_then(Value::as_bool) == Some(true) {
        let reply = env
            .pointer("/data/text")
            .and_then(Value::as_str)
            .map(str::to_string);
        return match reply {
            // 空串是合法回复（母体/化身可以只回空白吗——server 层已
            // 保证非空；None 才是坏形状）。
            Some(r) if !r.is_empty() => Ok(r),
            Some(_) => Err(PromptError::Other("empty reply from aginx-server".into())),
            None => Err(PromptError::Other("envelope without data.text".into())),
        };
    }
    let code = env.pointer("/error/code").and_then(Value::as_str).unwrap_or("");
    let message = env
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or("aginx-server turn failed")
        .to_string();
    if code == "unknown_avatar" {
        // server 消息自带化身名（"unknown avatar '名字'"）——负例收据
        // 靠它：错误必须提及化身名。
        Err(PromptError::UnknownAvatar(message))
    } else {
        Err(PromptError::Other(format!("{code}: {message}")))
    }
}
