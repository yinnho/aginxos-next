// brain — OpenAI 格式 brain 客户端（carrier llm_driver + llm_driver_impl 的
// 设计搬入，两处手术之一落地在这层）。
//
// 单层架构照搬 carrier：所有 LLM 流量过 aginxbrain（OpenAI 兼容代理），
// model 字段 = 模态路由标签（chat/reasoning/…），回退链归 brain 服务端，
// 本端不做 endpoint 轮换。搬入的东西：
// - 请求 sanitation（空名/坏 JSON arguments 的 tool_call 连带其 tool
//   结果一起剔除——历史毒化自愈，严格 provider 不会 400）
// - aginxbrain 的 {"code":"Success","output":{…}} 包装解包
// - <think> 标签剥离（部分上游把思考混在 content 里）
// - thinking-only 响应合成文本（取思考首段，否则用户什么都看不到）
// - 429/过载/传输错的指数退避重试（MAX_RETRIES=3）
// 未搬：多 endpoint fallback/circuit breaker/并发闸——那是 daemon 尺度
// 的机器，spawn-per-request 的 v0 用不上。

use crate::message::{Message, Role, StopReason};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

/// brain 一次调用要满配的形状（对话 + 工具面）。
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<crate::tools::ToolDef>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub system: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct RespToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Debug, Clone)]
pub struct CompletionResponse {
    pub text: String,
    pub thinking: String,
    pub tool_calls: Vec<RespToolCall>,
    pub stop_reason: StopReason,
    pub usage: TokenUsage,
}

impl CompletionResponse {
    pub fn end_turn(text: impl Into<String>) -> CompletionResponse {
        CompletionResponse {
            text: text.into(),
            thinking: String::new(),
            tool_calls: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: TokenUsage::default(),
        }
    }

    pub fn tool_use(calls: Vec<RespToolCall>) -> CompletionResponse {
        CompletionResponse {
            text: String::new(),
            thinking: String::new(),
            tool_calls: calls,
            stop_reason: StopReason::ToolUse,
            usage: TokenUsage::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BrainError {
    #[error("http: {0}")]
    Http(String),
    #[error("api {status}: {message}")]
    Api { status: u16, message: String },
    #[error("rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("parse: {0}")]
    Parse(String),
    #[error("auth failed: {0}")]
    Auth(String),
}

impl BrainError {
    /// 换个姿势重试有意义吗（认证/配置类错误重试也一样死）。
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            BrainError::RateLimited { .. } | BrainError::Http(_) | BrainError::Api { .. } | BrainError::Parse(_)
        )
    }
}

/// brain 驱动面：单方法，测试用脚本化假实现顶替。
#[async_trait]
pub trait BrainDriver: Send + Sync {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, BrainError>;
}

// ---------------------------------------------------------------------------
// 环境配置
// ---------------------------------------------------------------------------

/// AGINXBRAIN_API_KEY 只从环境来，运行时专用：不入库、不回显、不落日志。
#[derive(Debug, Clone)]
pub struct BrainConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}

impl BrainConfig {
    pub fn from_env() -> BrainConfig {
        BrainConfig {
            base_url: std::env::var("AGINX_BRAIN_URL")
                .unwrap_or_else(|_| "https://brain.aginx.net/v1/chat/completions".to_string()),
            api_key: std::env::var("AGINXBRAIN_API_KEY").ok().filter(|k| !k.is_empty()),
            model: std::env::var("AGINX_BRAIN_MODEL").unwrap_or_else(|_| "chat".to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP 驱动
// ---------------------------------------------------------------------------

pub const MAX_RETRIES: u32 = 3;
pub const BASE_RETRY_DELAY_MS: u64 = 1000;
const HTTP_TIMEOUT_SECS: u64 = 300;
const BODY_READ_TIMEOUT_SECS: u64 = 120;

pub struct HttpBrain {
    cfg: BrainConfig,
    client: reqwest::Client,
}

impl HttpBrain {
    pub fn new(cfg: BrainConfig) -> HttpBrain {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        HttpBrain { cfg, client }
    }
}

#[async_trait]
impl BrainDriver for HttpBrain {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, BrainError> {
        // 指数退避重试（carrier call_with_retry 的单驱动裁剪版）。
        for attempt in 0..=MAX_RETRIES {
            match self.once(&request).await {
                Ok(r) => return Ok(r),
                Err(e) if !e.is_retryable() || attempt == MAX_RETRIES => return Err(e),
                Err(BrainError::RateLimited { retry_after_ms }) => {
                    let delay = retry_after_ms.max(BASE_RETRY_DELAY_MS * 2u64.pow(attempt));
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(BASE_RETRY_DELAY_MS * 2u64.pow(attempt))).await;
                }
            }
        }
        unreachable!("retry loop returns inside")
    }
}

impl HttpBrain {
    async fn once(&self, request: &CompletionRequest) -> Result<CompletionResponse, BrainError> {
        // model 归本端配置：run_turn 传空串（模型选择是部署期决定，
        // 不是每轮该问的）。空则用 AGINX_BRAIN_MODEL。
        let mut request = request.clone();
        if request.model.is_empty() {
            request.model = self.cfg.model.clone();
        }
        let body = build_oai_request(&request);
        let mut builder = self
            .client
            .post(&self.cfg.base_url)
            .header("content-type", "application/json")
            .json(&body);
        if let Some(key) = &self.cfg.api_key {
            builder = builder.header("authorization", format!("Bearer {key}"));
        }
        let resp = builder.send().await.map_err(|e| BrainError::Http(e.to_string()))?;
        let status = resp.status().as_u16();
        if status == 401 || status == 403 {
            let msg = resp.text().await.unwrap_or_default();
            return Err(BrainError::Auth(format!("brain rejected credentials ({status}): {}", truncate(&msg, 300))));
        }
        if status == 429 {
            let retry_after_ms = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|secs| secs * 1000)
                .unwrap_or(0);
            return Err(BrainError::RateLimited { retry_after_ms });
        }
        if !resp.status().is_success() {
            let msg = resp.text().await.unwrap_or_default();
            return Err(BrainError::Api { status, message: truncate(&msg, 300).to_string() });
        }
        let text = tokio::time::timeout(Duration::from_secs(BODY_READ_TIMEOUT_SECS), resp.text())
            .await
            .map_err(|_| BrainError::Http(format!("response body read timed out after {BODY_READ_TIMEOUT_SECS}s")))?
            .map_err(|e| BrainError::Http(e.to_string()))?;
        parse_completion(&text)
    }
}

fn truncate(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

// ---------------------------------------------------------------------------
// 请求构建（Message 树 → OpenAI wire JSON）
// ---------------------------------------------------------------------------

pub(crate) fn build_oai_request(request: &CompletionRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = &request.system {
        if !system.is_empty() {
            messages.push(json!({"role": "system", "content": system}));
        }
    }
    for m in &request.messages {
        match m.role {
            Role::System => messages.push(json!({"role": "system", "content": m.content})),
            Role::User => messages.push(json!({"role": "user", "content": m.content})),
            Role::Assistant if !m.tool_calls.is_empty() => {
                let calls: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .map(|c| {
                        json!({"id": c.id, "type": "function",
                               "function": {"name": c.name, "arguments": c.arguments}})
                    })
                    .collect();
                let mut v = json!({"role": "assistant", "tool_calls": calls});
                if !m.content.is_empty() {
                    v["content"] = json!(m.content);
                }
                messages.push(v);
            }
            Role::Assistant => messages.push(json!({"role": "assistant", "content": m.content})),
            Role::Tool => {
                let content =
                    if m.content.is_empty() { "(empty)".to_string() } else { m.content.clone() };
                messages.push(json!({
                    "role": "tool", "content": content,
                    "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                }));
            }
        }
    }

    // Sanitation（carrier 同款）：空名 / 非 JSON / "null" arguments 的调用
    // 连带其配对 tool 结果一起剔除——严格 provider 见到会 400，且这类毒
    // 记录可能在会话历史里躺了很久。
    let mut removed_ids: Vec<String> = Vec::new();
    for m in &mut messages {
        let Some(calls) = m.get_mut("tool_calls").and_then(|v| v.as_array_mut()) else { continue };
        calls.retain(|c| {
            let f = &c["function"];
            let name_ok = f["name"].as_str().is_some_and(|n| !n.is_empty());
            let args = f["arguments"].as_str().unwrap_or("").trim();
            let args_ok = !args.is_empty()
                && args != "null"
                && serde_json::from_str::<Value>(args).is_ok();
            let ok = name_ok && args_ok;
            if !ok {
                if let Some(id) = c["id"].as_str() {
                    removed_ids.push(id.to_string());
                }
            }
            ok
        });
        if calls.is_empty() {
            m.as_object_mut().unwrap().remove("tool_calls");
        }
    }
    if !removed_ids.is_empty() {
        messages.retain(|m| {
            if m["role"] == "tool" {
                if let Some(id) = m["tool_call_id"].as_str() {
                    return !removed_ids.iter().any(|r| r == id);
                }
            }
            true
        });
    }

    let tools: Vec<Value> = request
        .tools
        .iter()
        .map(|t| {
            json!({"type": "function", "function": {
                "name": t.name, "description": t.description, "parameters": t.parameters,
            }})
        })
        .collect();

    let mut v = json!({"model": request.model, "messages": messages, "stream": false});
    if !tools.is_empty() {
        v["tools"] = json!(tools);
        v["tool_choice"] = json!("auto");
    }
    if request.max_tokens > 0 {
        v["max_tokens"] = json!(request.max_tokens);
    }
    if request.temperature > 0.0 {
        v["temperature"] = json!(request.temperature);
    }
    v
}

// ---------------------------------------------------------------------------
// 响应解析（OpenAI wire JSON → CompletionResponse）
// ---------------------------------------------------------------------------

pub(crate) fn parse_completion(body: &str) -> Result<CompletionResponse, BrainError> {
    let parsed: Value =
        serde_json::from_str(body).map_err(|e| BrainError::Parse(format!("bad json: {e}")))?;
    // aginxbrain 有时把响应包成 {"code":"Success","output":{…}} —— 标准
    // 形状优先，缺 choices 就解包 output。
    let oai = if parsed.get("choices").is_some() {
        parsed
    } else if let Some(output) = parsed.get("output") {
        output.clone()
    } else {
        parsed
    };

    let choice = oai
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| BrainError::Parse("no choices in response".into()))?;
    let msg = &choice["message"];

    // content 可以是字符串或分部数组；只取文本分部（v0 无媒体）。
    let raw_text = match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| match p {
                Value::String(s) => Some(s.clone()),
                other => other.get("text").and_then(|t| t.as_str()).map(String::from),
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    };
    let (text, think_from_tags) = extract_think_tags(&raw_text);
    let mut thinking = msg
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if thinking.is_empty() {
        thinking = think_from_tags.unwrap_or_default();
    }

    let mut tool_calls = Vec::new();
    if let Some(calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
        for c in calls {
            let name = c["function"]["name"].as_str().unwrap_or("").to_string();
            if name.is_empty() {
                continue; // 空名调用不可执行还毒历史，直接跳过
            }
            let input: Value = c["function"]["arguments"]
                .as_str()
                .and_then(|a| serde_json::from_str(a).ok())
                .unwrap_or_else(|| json!({}));
            tool_calls.push(RespToolCall {
                id: c["id"].as_str().unwrap_or("").to_string(),
                name,
                input,
            });
        }
    }

    // thinking-only：有思考无正文无调用时，取思考首段合成可见文本。
    let text = if text.is_empty() && tool_calls.is_empty() && !thinking.is_empty() {
        truncate(thinking.trim(), 200).to_string()
    } else {
        text
    };

    let stop_reason = match choice["finish_reason"].as_str() {
        Some("tool_calls") => StopReason::ToolUse,
        Some("length") => StopReason::MaxTokens,
        _ => {
            if !tool_calls.is_empty() {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }
        }
    };

    let mut usage = TokenUsage {
        input_tokens: oai["usage"]["prompt_tokens"].as_u64().unwrap_or(0),
        output_tokens: oai["usage"]["completion_tokens"].as_u64().unwrap_or(0),
    };
    if !text.is_empty() && tool_calls.is_empty() && usage.input_tokens == 0 && usage.output_tokens
        == 0
    {
        usage.output_tokens = 1;
    }

    Ok(CompletionResponse { text, thinking, tool_calls, stop_reason, usage })
}

/// 剥 `<think>…</think>` 块（部分上游把思考混进 content）。
/// 返回 (清掉后的正文, 首个 think 块内容或 None)。
pub(crate) fn extract_think_tags(text: &str) -> (String, Option<String>) {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let mut out = String::with_capacity(text.len());
    let mut first: Option<String> = None;
    let mut rest = text;
    while let Some(p) = rest.find(OPEN) {
        out.push_str(&rest[..p]);
        let after = &rest[p + OPEN.len()..];
        match after.find(CLOSE) {
            Some(c) => {
                let block = &after[..c];
                if first.is_none() {
                    first = Some(block.to_string());
                }
                rest = &after[c + CLOSE.len()..];
            }
            None => {
                // 未闭合：当普通文本保留，不再扫
                out.push_str(&rest[p..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    (out, first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::MsgToolCall;
    use crate::tools::ToolDef;

    fn req() -> CompletionRequest {
        CompletionRequest {
            model: "chat".into(),
            messages: vec![
                Message::user("你好"),
                Message::assistant_tools(
                    "",
                    vec![MsgToolCall {
                        id: "c1".into(),
                        name: "dev-hello".into(),
                        arguments: r#"["世界"]"#.into(),
                    }],
                ),
                Message::tool_result("c1", "hello 世界"),
                Message::assistant_text("done"),
            ],
            tools: vec![ToolDef::new("dev-hello", "smoke face", None)],
            max_tokens: 512,
            temperature: 0.7,
            system: Some("你是化身".into()),
        }
    }

    #[test]
    fn request_wire_shape() {
        let v = build_oai_request(&req());
        assert_eq!(v["model"], json!("chat"));
        assert_eq!(v["stream"], json!(false));
        assert_eq!(v["tool_choice"], json!("auto"));
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], json!("system")); // system 永远第一
        assert_eq!(msgs[1]["role"], json!("user"));
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], json!("dev-hello"));
        assert_eq!(msgs[3]["role"], json!("tool"));
        assert_eq!(msgs[3]["tool_call_id"], json!("c1"));
        assert_eq!(msgs[4]["role"], json!("assistant"));
        assert_eq!(v["tools"][0]["function"]["name"], json!("dev-hello"));
    }

    #[test]
    fn request_sanitizes_poisoned_tool_calls() {
        let mut r = req();
        r.messages[1].tool_calls.push(MsgToolCall {
            id: "c9".into(),
            name: "".into(), // 空名：剔除
            arguments: "null".into(),
        });
        r.messages.insert(3, Message::tool_result("c9", "orphan"));
        let v = build_oai_request(&r);
        let msgs = v["messages"].as_array().unwrap();
        // 坏调用与其配对结果都被剔除，只剩 c1 的调用与结果
        let assistant = msgs.iter().find(|m| m["role"] == json!("assistant") && m.get("tool_calls").is_some()).unwrap();
        assert_eq!(assistant["tool_calls"].as_array().unwrap().len(), 1);
        assert_eq!(assistant["tool_calls"][0]["id"], json!("c1"));
        assert!(msgs.iter().all(|m| m["tool_call_id"] != json!("c9")));
    }

    #[test]
    fn parse_standard_completion() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"你好，收到","reasoning_content":"想想"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let r = parse_completion(body).unwrap();
        assert_eq!(r.text, "你好，收到");
        assert_eq!(r.thinking, "想想");
        assert_eq!(r.stop_reason, StopReason::EndTurn);
        assert_eq!(r.usage.input_tokens, 10);
        assert_eq!(r.usage.output_tokens, 5);
    }

    #[test]
    fn parse_wrapped_envelope() {
        // aginxbrain 包装形状
        let body = r#"{"code":"Success","output":{"choices":[{"message":{"content":"wrapped"},"finish_reason":"stop"}]}}"#;
        let r = parse_completion(body).unwrap();
        assert_eq!(r.text, "wrapped");
    }

    #[test]
    fn parse_tool_calls_and_finish_reason() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"c1","type":"function","function":{"name":"web-search","arguments":"{\"args\":[\"北京\"]}"}}]},"finish_reason":"tool_calls"}]}"#;
        let r = parse_completion(body).unwrap();
        assert_eq!(r.stop_reason, StopReason::ToolUse);
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "web-search");
        assert_eq!(r.tool_calls[0].input, json!({"args": ["北京"]}));
    }

    #[test]
    fn parse_think_tags_stripped() {
        let body = r#"{"choices":[{"message":{"content":"<think>推理过程</think>答案是 42"},"finish_reason":"stop"}]}"#;
        let r = parse_completion(body).unwrap();
        assert_eq!(r.text, "答案是 42");
        assert_eq!(r.thinking, "推理过程");
    }

    #[test]
    fn parse_thinking_only_synthesizes_text() {
        let body = r#"{"choices":[{"message":{"content":null,"reasoning_content":"这是一段很长的思考过程需要被截断成可见文本"},"finish_reason":"stop"}]}"#;
        let r = parse_completion(body).unwrap();
        assert!(!r.text.is_empty());
        assert!(r.text.starts_with("这是一段"));
    }

    #[test]
    fn parse_synthetic_usage_when_missing() {
        let body = r#"{"choices":[{"message":{"content":"hi"},"finish_reason":"stop"}]}"#;
        let r = parse_completion(body).unwrap();
        assert_eq!(r.usage.output_tokens, 1);
    }

    #[test]
    fn extract_think_tags_unclosed_kept() {
        let (out, t) = extract_think_tags("a<think>未闭合");
        assert_eq!(out, "a<think>未闭合");
        assert!(t.is_none());
    }
}
