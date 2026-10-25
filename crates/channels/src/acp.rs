//! ACP 窄接口客户端（ndjson JSON-RPC 2.0 over TCP）。
//!
//! 频道对引擎只说这一句：「把文本交给某 agent，拿回文本」——不感知
//! 引擎实现（codex/grok/system 条目随便换，那是 gateway 名册的事）。
//! 对端=本机 aginx-gateway-local（direct 模式 127.0.0.1:8686）。
//!
//! 线序（ACP.md 线权威=生态仓）：initialize(id 1) → prompt(id 2) →
//! chunk 通知（拼接即全文）→ 终帧 result{stopReason,sessionId}（无 id，
//! 不回 prompt ack）。错误帧带 id 与 error.message。
//!
//! 已知取舍：服务器空闲 300s 断连，codex 单长回合若无 chunk 可能静默
//! 超 300s——v0.1 接受（本客户端读超时给足 1200s，断了如实报错回信）。
//!
//! 富媒体最小扩展（v0.1.5，DESIGN §四窄接口保持）：回信除文本外可带
//! 本地媒体路径——(1) assistant content 块 `type=image|video|file` 且
//! 含 `path`/`url` 指向本机文件；(2) 文本内标记
//! `[[aginx-media:image|/abs/path]]`（video/file 同形）。桥侧上传 CDN
//! 后发，标记从正文剥离。无媒体形状时行为与 v0.1.4 相同。

use anyhow::{anyhow, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub const DEFAULT_ADDR: &str = "127.0.0.1:8686";
/// 读超时上限：须长过 gateway 侧 agent 条目 timeout（最长 900s 档）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 1200;

pub struct AcpClient {
    pub addr: String,
    pub timeout: Duration,
    /// 机内服务凭证（刀B）：网关 [[local_clients]] 表的 token——
    /// initialize 时经 _meta.authToken 上呈，principal=声明式身份。
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpMediaKind {
    Image,
    Video,
    File,
}

impl AcpMediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::File => "file",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "image" | "img" | "png" | "jpg" | "jpeg" => Some(Self::Image),
            "video" | "mp4" => Some(Self::Video),
            "file" | "attachment" => Some(Self::File),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AcpMedia {
    pub kind: AcpMediaKind,
    pub path: String,
}

pub struct AcpReply {
    pub text: String,
    pub session_id: Option<String>,
    /// Local media paths declared by the engine (see module docs).
    pub media: Vec<AcpMedia>,
}

/// chunk 方言归一（ACP.md §2.6/§2.8）：规约上 chunk 的 text 是纯文本
/// （接入包声明 output 时网关已翻译）；但 `raw` 方言条目（如本仓
/// carrier 双模桥裸行模）直通引擎的整行 JSON——assistant 行是
/// `{"message":{"content":[{"text":…}]}}`，尾行是 `{"type":"result",…}`
/// （其 result 字段复述全文，拼上会翻倍）。两副面孔都认：
/// 解析得 JSON 信封就取内文，纯文本原样入账；result 行只当回执不拼文。
///
/// 媒体：content 块 `type=image|video|file` 且 `path`/`url` 为本机路径时
/// 记入 `media`；纯文本里的 `[[aginx-media:kind|/path]]` 亦然（稍后剥离）。
fn push_chunk_text(out: &mut String, media: &mut Vec<AcpMedia>, raw: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        out.push_str(raw); // 规约主路：纯文本直通
        return;
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            if let Some(blocks) = v.pointer("/message/content").and_then(|c| c.as_array()) {
                for b in blocks {
                    if let Some(t) = b.get("text").and_then(|t| t.as_str()) {
                        out.push_str(t);
                    }
                    collect_content_media(media, b);
                }
            }
        }
        // raw 方言收尾行：全文已由 assistant 行拼齐，跳过（防翻倍）。
        Some("result") => {}
        // 陌生 JSON 形状：原样保底，宁可难看不可丢话。
        _ => out.push_str(raw),
    }
}

fn collect_content_media(media: &mut Vec<AcpMedia>, block: &serde_json::Value) {
    let ty = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let kind = match ty {
        "image" | "image_url" | "input_image" => Some(AcpMediaKind::Image),
        "video" | "input_video" => Some(AcpMediaKind::Video),
        "file" | "input_file" | "attachment" => Some(AcpMediaKind::File),
        _ => None,
    };
    let Some(kind) = kind else {
        return;
    };
    let path = block
        .get("path")
        .or_else(|| block.get("url"))
        .or_else(|| block.get("file"))
        .and_then(|v| v.as_str())
        .or_else(|| block.pointer("/source/path").and_then(|v| v.as_str()));
    let Some(path) = path else {
        return;
    };
    // Only local filesystem paths — remote http(s) left for a later hop.
    if path.starts_with("http://") || path.starts_with("https://") || path.starts_with("data:") {
        return;
    }
    if std::path::Path::new(path).is_file() {
        media.push(AcpMedia {
            kind,
            path: path.to_string(),
        });
    }
}

/// Strip `[[aginx-media:kind|/abs/path]]` markers from text into media list.
///
/// Walk by UTF-8 char, never by raw byte: `bytes[i] as char` would turn
/// multi-byte Chinese into Latin-1 mojibake (v0.1.5 regression → WeChat 乱码).
pub fn extract_media_markers(text: &str) -> (String, Vec<AcpMedia>) {
    let mut media = Vec::new();
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"[[aginx-media:") {
            if let Some(end) = text[i..].find("]]") {
                let inner = &text[i + "[[aginx-media:".len()..i + end];
                let (kind_s, path) = if let Some((k, rest)) = inner.split_once('|') {
                    (k.trim(), rest.trim())
                } else if let Some((k, rest)) = inner.split_once(':') {
                    (k.trim(), rest.trim())
                } else {
                    ("", "")
                };
                if let Some(kind) = AcpMediaKind::parse(kind_s) {
                    if !path.is_empty() && std::path::Path::new(path).is_file() {
                        media.push(AcpMedia {
                            kind,
                            path: path.to_string(),
                        });
                        i += end + 2;
                        continue;
                    }
                }
            }
        }
        // One Unicode scalar, not one byte — preserves CJK / emoji intact.
        let ch = text[i..].chars().next().expect("i on char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    (out, media)
}

impl AcpClient {
    pub fn new() -> Self {
        let timeout = std::env::var("AGINX_CHANNELS_ACP_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&s: &u64| s >= 30)
            .unwrap_or(DEFAULT_TIMEOUT_SECS);
        Self {
            addr: std::env::var("AGINX_CHANNELS_ACP_ADDR")
                .unwrap_or_else(|_| DEFAULT_ADDR.to_string()),
            token: std::env::var("AGINX_CHANNELS_ACP_TOKEN")
                .ok()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()),
            timeout: Duration::from_secs(timeout),
        }
    }

    /// One prompt per connection（ACP 语义：连接=会话腿，直连模式服务器
    /// 本就一连接一请求序列；简单诚实，不做连接池）。
    pub fn prompt(
        &self,
        agent: &str,
        message: &str,
        resume_session: Option<&str>,
    ) -> Result<AcpReply> {
        let deadline = Instant::now() + self.timeout;
        let connect_timeout = self.timeout.min(Duration::from_secs(10));
        let mut stream = TcpStream::connect(&self.addr)
            .with_context(|| format!("ACP connect {addr}", addr = self.addr))?;
        stream.set_read_timeout(Some(connect_timeout)).ok();
        stream.set_write_timeout(Some(connect_timeout)).ok();

        // initialize（带机内服务凭证时经 _meta.authToken 上呈——网关
        // local_clients 表换 Authorized+声明式 principal，不再是匿名 PseudoBound）
        let mut init_params = serde_json::json!({
            "protocolVersion": "0.1.0",
            "clientInfo": { "name": "aginx-channels", "version": env!("CARGO_PKG_VERSION") },
        });
        if let Some(ref token) = self.token {
            init_params["_meta"] = serde_json::json!({ "authToken": token });
        }
        let init = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": init_params,
        });
        self.send_line(&mut stream, &init)?;
        self.wait_response(&mut stream, 1, deadline, None)?; // 无 chunk 期

        // prompt（无 ack，直接读流）
        let mut params = serde_json::json!({ "agent": agent, "message": message });
        if let Some(sid) = resume_session.filter(|s| !s.is_empty()) {
            params["sessionId"] = serde_json::json!(sid);
        }
        let prompt = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "prompt",
            "params": params,
        });
        self.send_line(&mut stream, &prompt)?;

        // 读到终帧：chunk 拼接，result 停
        let mut text = String::new();
        let mut media: Vec<AcpMedia> = Vec::new();
        let mut session_id = resume_session.map(|s| s.to_string());
        let mut reader = BufReader::new(stream);
        loop {
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "ACP prompt timed out after {secs}s (agent {agent})",
                    secs = self.timeout.as_secs()
                ));
            }
            let remain = deadline.saturating_duration_since(Instant::now());
            reader
                .get_ref()
                .set_read_timeout(Some(remain.max(Duration::from_millis(500))))
                .ok();
            let mut line = String::new();
            let n = reader
                .read_line(&mut line)
                .with_context(|| "ACP stream closed mid-turn")?;
            if n == 0 {
                return Err(anyhow!("ACP stream closed mid-turn (agent {agent})"));
            }
            let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue; // 空行/垃圾行——跳过，下一帧见
            };
            if let Some(err) = frame.get("error") {
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("?");
                let code = err.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
                return Err(anyhow!("ACP error {code}: {msg} (agent {agent})"));
            }
            if frame.get("method").and_then(|m| m.as_str()) == Some("chunk") {
                if let Some(t) = frame.pointer("/params/text").and_then(|t| t.as_str()) {
                    push_chunk_text(&mut text, &mut media, t);
                }
                continue;
            }
            if let Some(result) = frame.get("result") {
                if let Some(sid) = result.get("sessionId").and_then(|s| s.as_str()) {
                    if !sid.is_empty() {
                        session_id = Some(sid.to_string());
                    }
                }
                let (text, marker_media) = extract_media_markers(&text);
                media.extend(marker_media);
                return Ok(AcpReply {
                    text,
                    session_id,
                    media,
                });
            }
            // 其余通知（如日志类）——忽略
        }
    }

    fn send_line(&self, stream: &mut TcpStream, value: &serde_json::Value) -> Result<()> {
        let mut line = serde_json::to_string(value)?;
        line.push('\n');
        stream.write_all(line.as_bytes())?;
        stream.flush()?;
        Ok(())
    }

    /// 等 id 匹配的 response 帧（initialize 的回执）。途中 chunk/通知忽略。
    fn wait_response(
        &self,
        stream: &mut TcpStream,
        want_id: i64,
        deadline: Instant,
        _sink: Option<&mut String>,
    ) -> Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        loop {
            if Instant::now() >= deadline {
                return Err(anyhow!("ACP initialize timed out"));
            }
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))
                .ok();
            let mut line = String::new();
            let n = reader.read_line(&mut line).context("ACP stream closed at init")?;
            if n == 0 {
                return Err(anyhow!("ACP stream closed at init"));
            }
            let Ok(frame) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            if let Some(err) = frame.get("error") {
                let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("?");
                return Err(anyhow!("ACP init error: {msg}"));
            }
            if frame.get("id").and_then(|i| i.as_i64()) == Some(want_id) {
                return Ok(());
            }
            // 通知帧先到（不会，但忽略无害）
        }
    }
}


impl Default for AcpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_media_markers, push_chunk_text, AcpMediaKind};

    /// 规约主路（ACP.md §2.6）：纯文本 chunk 直通拼接。
    #[test]
    fn chunk_plain_text_concatenates() {
        let mut out = String::new();
        let mut media = Vec::new();
        push_chunk_text(&mut out, &mut media, "在");
        push_chunk_text(&mut out, &mut media, "的");
        assert_eq!(out, "在的");
        assert!(media.is_empty());
    }

    /// raw 方言（carrier 双模桥裸行模）：assistant 行取内文 text 块，
    /// result 收尾行跳过（result 字段复述全文，拼上会翻倍）。
    #[test]
    fn chunk_raw_dialect_envelope_extracts_and_skips_result() {
        let mut out = String::new();
        let mut media = Vec::new();
        push_chunk_text(
            &mut out,
            &mut media,
            r#"{"message":{"content":[{"text":"在","type":"text"}]},"type":"assistant"}"#,
        );
        push_chunk_text(
            &mut out,
            &mut media,
            r#"{"message":{"content":[{"text":"的","type":"text"}]},"type":"assistant"}"#,
        );
        push_chunk_text(
            &mut out,
            &mut media,
            r#"{"duration_ms":4308,"is_error":false,"result":"在的","session_id":"s1","type":"result"}"#,
        );
        assert_eq!(out, "在的");
    }

    /// 陌生 JSON 形状：原样保底（宁可难看不可丢话）。
    #[test]
    fn chunk_unknown_json_falls_back_to_raw() {
        let mut out = String::new();
        let mut media = Vec::new();
        push_chunk_text(&mut out, &mut media, r#"{"hello":"world"}"#);
        assert_eq!(out, r#"{"hello":"world"}"#);
    }

    #[test]
    fn media_marker_strips_and_collects() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/acp.rs");
        assert!(std::path::Path::new(path).is_file(), "fixture path {path}");
        let raw = format!("hello [[aginx-media:image|{path}]] world");
        let (text, media) = extract_media_markers(&raw);
        assert_eq!(text, "hello  world");
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].kind, AcpMediaKind::Image);
        assert_eq!(media[0].path, path);
    }

    /// v0.1.5 regression: byte-as-char walk turned UTF-8 CJK into Latin-1
    /// mojibake on every ACP→WeChat reply (even with no media markers).
    #[test]
    fn media_marker_preserves_chinese_without_markers() {
        let raw = "你好，白泽在研究。";
        let (text, media) = extract_media_markers(raw);
        assert_eq!(text, raw);
        assert!(media.is_empty());
    }

    #[test]
    fn media_marker_preserves_chinese_around_marker() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/src/acp.rs");
        let raw = format!("开头 [[aginx-media:file|{path}]] 结尾");
        let (text, media) = extract_media_markers(&raw);
        assert_eq!(text, "开头  结尾");
        assert_eq!(media.len(), 1);
        assert_eq!(media[0].kind, AcpMediaKind::File);
    }
}
