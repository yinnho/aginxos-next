//! OpenAI-compatible HTTP driver for LLM API calls.
//!
//! All LLM traffic goes through aginxbrain (OpenAI-compatible proxy),
//! so this driver only implements the OpenAI Chat Completions format.
//! aginxbrain handles provider-specific routing based on the model/tag.

use crate::llm_driver::{CompletionRequest, CompletionResponse, LlmDriver, LlmError, StreamEvent};
use crate::think_filter::{FilterAction, StreamingThinkFilter};
use crate::USER_AGENT;
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};
use carrier_types::message::{ContentBlock, MessageContent, Role, StopReason, TokenUsage};
use carrier_types::tool::ToolCall;
use zeroize::Zeroizing;

// ---------------------------------------------------------------------------
// OpenAI driver struct
// ---------------------------------------------------------------------------

/// Total HTTP timeout for a NON-streaming LLM request (seconds). Streaming
/// requests override this per-request with `LLM_STREAM_TOTAL_TIMEOUT_SECS`
/// (see send_openai_with_retry) — a total cap there killed healthy long
/// generations mid-stream. Acts as a hard backstop even if the outer
/// `tokio::time::timeout` somehow doesn't fire (observed when the server
/// accepts the connection then stalls).
pub(crate) const LLM_HTTP_TIMEOUT_SECS: u64 = 180;

/// Timeout for reading a response body after headers are received (seconds).
/// Prevents hangs when the server sends headers slowly but then stalls on body.
const LLM_BODY_READ_TIMEOUT_SECS: u64 = 120;

/// Total-request timeout for STREAMING requests (seconds). Overrides the
/// client-level `LLM_HTTP_TIMEOUT_SECS` per request: a healthy SSE stream that
/// keeps producing tokens has no total budget problem, and the layered idle
/// timeouts (byte / content / first-token, below) are the real stall
/// detection. The client-level 180s cap killed legitimate long generations
/// mid-stream (clone-creation turns writing whole definition files exceed
/// 180s of pure output) — retries regenerated from scratch and the turn died.
/// 1800s is only a zombie backstop; a genuine stream of max_tokens=8192
/// finishes far below it.
pub(crate) const LLM_STREAM_TOTAL_TIMEOUT_SECS: u64 = 1800;

/// Response-header budget for streaming requests. Real SSE servers send
/// headers immediately and then stream tokens, so a tight first-byte budget
/// used to be safe — but brains that IGNORE the `stream` parameter (the
/// AginxOS brain, per aginxos/docs/CARRIER.md §3.2) compute the ENTIRE
/// response before sending any byte, and codex measured requests needing
/// the 600s tier. Must stay well under LLM_STREAM_TOTAL_TIMEOUT_SECS.
pub(crate) const STREAM_HEADER_TIMEOUT_SECS: u64 = 600;

// Compile-time invariant: the header budget must never exceed the
// whole-request streaming budget.
const _: () = assert!(STREAM_HEADER_TIMEOUT_SECS <= LLM_STREAM_TOTAL_TIMEOUT_SECS);

pub struct UnifiedHttpDriver {
    api_key: Zeroizing<String>,
    base_url: String,
    client: reqwest::Client,
}

impl UnifiedHttpDriver {
    pub fn new(api_key: String, base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .pool_max_idle_per_host(0)
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(LLM_HTTP_TIMEOUT_SECS))
            .build()
            .unwrap_or_default();
        Self {
            api_key: Zeroizing::new(api_key),
            base_url,
            client,
        }
    }
}

// ---------------------------------------------------------------------------
// OpenAI format request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct OaiMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<OaiMessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OaiToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    /// Always include reasoning_content — aginxbrain handles provider differences.
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OaiMessageContent {
    Text(String),
    Parts(Vec<OaiContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum OaiContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: OaiImageUrl },
    #[serde(rename = "input_audio")]
    InputAudio { input_audio: OaiInputAudio },
}

#[derive(Debug, Serialize)]
struct OaiImageUrl {
    url: String,
}

#[derive(Debug, Serialize)]
struct OaiInputAudio {
    data: String,
    format: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct OaiToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OaiFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OaiFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Serialize)]
struct OaiTool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OaiToolDef,
}

#[derive(Debug, Serialize)]
struct OaiToolDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct OaiResponse {
    choices: Vec<OaiChoice>,
    usage: Option<OaiUsage>,
}

#[derive(Debug, Deserialize)]
struct OaiChoice {
    message: OaiResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OaiResponseMessage {
    content: Option<serde_json::Value>,
    tool_calls: Option<Vec<OaiToolCall>>,
    reasoning_content: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct OaiUsage {
    prompt_tokens: u64,
    completion_tokens: u64,
}

fn extract_reasoning_text(val: &serde_json::Value) -> String {
    val.as_str().unwrap_or("").to_string()
}

fn mime_to_audio_format(mime: &str) -> &str {
    match mime {
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/ogg" => "ogg",
        "audio/flac" => "flac",
        "audio/mp4" | "audio/m4a" => "mp4",
        "audio/webm" => "webm",
        _ => "mp3",
    }
}

// ---------------------------------------------------------------------------
// Message building
// ---------------------------------------------------------------------------

impl UnifiedHttpDriver {
    fn build_oai_messages(&self, request: &CompletionRequest) -> Vec<OaiMessage> {
        let mut oai_messages: Vec<OaiMessage> = Vec::new();

        if let Some(ref system) = request.system {
            if !system.is_empty() {
                oai_messages.push(OaiMessage {
                    role: "system".to_string(),
                    content: Some(OaiMessageContent::Text(system.clone())),
                    tool_calls: None,
                    tool_call_id: None,
                    reasoning_content: None,
                });
            }
        }

        for msg in &request.messages {
            match (&msg.role, &msg.content) {
                (Role::System, MessageContent::Text(text)) => {
                    if request.system.is_none() {
                        oai_messages.push(OaiMessage {
                            role: "system".to_string(),
                            content: Some(OaiMessageContent::Text(text.clone())),
                            tool_calls: None,
                            tool_call_id: None,
                            reasoning_content: None,
                        });
                    }
                }
                (Role::User, MessageContent::Text(text)) => {
                    oai_messages.push(OaiMessage {
                        role: "user".to_string(),
                        content: Some(OaiMessageContent::Text(text.clone())),
                        tool_calls: None,
                        tool_call_id: None,
                        reasoning_content: None,
                    });
                }
                (Role::Assistant, MessageContent::Text(text)) => {
                    oai_messages.push(OaiMessage {
                        role: "assistant".to_string(),
                        content: Some(OaiMessageContent::Text(text.clone())),
                        tool_calls: None,
                        tool_call_id: None,
                        reasoning_content: None,
                    });
                }
                (Role::User, MessageContent::Blocks(blocks)) => {
                    let mut parts: Vec<OaiContentPart> = Vec::new();
                    let mut has_tool_results = false;
                    for block in blocks {
                        match block {
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                ..
                            } => {
                                has_tool_results = true;
                                oai_messages.push(OaiMessage {
                                    role: "tool".to_string(),
                                    content: Some(OaiMessageContent::Text(if content.is_empty() {
                                        "(empty)".to_string()
                                    } else {
                                        content.clone()
                                    })),
                                    tool_calls: None,
                                    tool_call_id: Some(tool_use_id.clone()),
                                    reasoning_content: None,
                                });
                            }
                            ContentBlock::Text { text, .. } => {
                                parts.push(OaiContentPart::Text { text: text.clone() });
                            }
                            ContentBlock::Image {
                                data,
                                media_type,
                                url,
                            } => {
                                // Prefer a real HTTP(S) URL so providers fetch the image
                                // themselves (avoids huge base64 payloads / token bloat).
                                let image_url = match url.as_ref() {
                                    Some(u)
                                        if u.starts_with("https://")
                                            || u.starts_with("http://") =>
                                    {
                                        u.clone()
                                    }
                                    _ if !data.is_empty() => {
                                        format!("data:{media_type};base64,{data}")
                                    }
                                    Some(u) if !u.is_empty() => u.clone(),
                                    _ => continue,
                                };
                                parts.push(OaiContentPart::ImageUrl {
                                    image_url: OaiImageUrl { url: image_url },
                                });
                            }
                            ContentBlock::Audio {
                                data, media_type, ..
                            } => {
                                parts.push(OaiContentPart::InputAudio {
                                    input_audio: OaiInputAudio {
                                        data: data.clone(),
                                        format: mime_to_audio_format(media_type).to_string(),
                                    },
                                });
                            }
                            ContentBlock::Thinking { .. } => {}
                            _ => {}
                        }
                    }
                    if !parts.is_empty() && !has_tool_results {
                        oai_messages.push(OaiMessage {
                            role: "user".to_string(),
                            content: Some(OaiMessageContent::Parts(parts)),
                            tool_calls: None,
                            tool_call_id: None,
                            reasoning_content: None,
                        });
                    }
                }
                (Role::Assistant, MessageContent::Blocks(blocks)) => {
                    let mut text_parts = Vec::new();
                    let mut tc_list = Vec::new();
                    let mut reasoning_text = String::new();
                    for block in blocks {
                        match block {
                            ContentBlock::Text { text, .. } => text_parts.push(text.clone()),
                            ContentBlock::ToolUse {
                                id, name, input, ..
                            } => {
                                tc_list.push(OaiToolCall {
                                    id: id.clone(),
                                    call_type: "function".to_string(),
                                    function: OaiFunction {
                                        name: name.clone(),
                                        arguments: serde_json::to_string(input).unwrap_or_default(),
                                    },
                                });
                            }
                            ContentBlock::Thinking { thinking } => {
                                reasoning_text = thinking.clone();
                            }
                            _ => {}
                        }
                    }
                    let has_tool_calls = !tc_list.is_empty();
                    oai_messages.push(OaiMessage {
                        role: "assistant".to_string(),
                        content: if text_parts.is_empty() {
                            if has_tool_calls {
                                Some(OaiMessageContent::Text(String::new()))
                            } else {
                                None
                            }
                        } else {
                            Some(OaiMessageContent::Text(text_parts.join("")))
                        },
                        tool_calls: if tc_list.is_empty() {
                            None
                        } else {
                            Some(tc_list)
                        },
                        tool_call_id: None,
                        // Always include reasoning_content — aginxbrain handles provider differences
                        reasoning_content: if reasoning_text.is_empty() {
                            None
                        } else {
                            Some(reasoning_text)
                        },
                    });
                }
                _ => {}
            }
        }

        oai_messages
    }

    fn build_oai_request(&self, request: &CompletionRequest) -> OaiRequest {
        let mut messages = self.build_oai_messages(request);

        // Sanitize tool_call arguments AND names: strict providers reject
        // non-JSON arguments like "null", empty strings, or malformed JSON, and
        // reject function_call entries with an empty name ("Invalid 'name' for
        // function_call"). An empty name can sneak in from a gateway streaming
        // edge case (see AginxBrain spec §5.4) and persist in session history;
        // dropping the call here (and its paired tool_result below) repairs both
        // new and historically-poisoned sessions.
        let mut removed_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for msg in &mut messages {
            if let Some(calls) = &mut msg.tool_calls {
                calls.retain(|tc| {
                    let name_valid = !tc.function.name.is_empty();
                    let args = tc.function.arguments.trim();
                    let args_valid = !args.is_empty() && args != "null" && serde_json::from_str::<serde_json::Value>(args).is_ok();
                    let valid = name_valid && args_valid;
                    if !valid {
                        warn!(tool = %tc.function.name, raw_args = %tc.function.arguments, "Removing tool_call with invalid name or arguments from request");
                        removed_ids.insert(tc.id.clone());
                    }
                    valid
                });
                if calls.is_empty() {
                    msg.tool_calls = None;
                }
            }
        }
        // Remove tool_result messages whose call was removed
        if !removed_ids.is_empty() {
            messages.retain(|msg| {
                if msg.role == "tool" {
                    if let Some(ref id) = msg.tool_call_id {
                        return !removed_ids.contains(id);
                    }
                }
                true
            });
        }

        let max_tokens = if request.max_tokens > 0 {
            Some(request.max_tokens)
        } else {
            None
        };
        let temperature = if request.temperature > 0.0 {
            Some(request.temperature)
        } else {
            None
        };

        let tools: Vec<OaiTool> = request.tools.iter().map(|t| {
            let schema = carrier_types::tool::normalize_schema_for_provider(&t.input_schema, "openai");
            if !schema.is_object() {
                warn!(tool = %t.name, "Tool schema is not an object after normalization, type={}", schema);
            }
            OaiTool {
                tool_type: "function".to_string(),
                function: OaiToolDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: schema,
                },
            }
        }).collect();

        let tool_choice = if tools.is_empty() {
            None
        } else {
            Some(serde_json::json!("auto"))
        };

        OaiRequest {
            model: request.model.clone(),
            messages,
            max_tokens,
            temperature,
            tools,
            tool_choice,
            stream: false,
            stream_options: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Non-streaming completion
// ---------------------------------------------------------------------------

impl UnifiedHttpDriver {
    async fn complete_openai(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, LlmError> {
        let mut oai_request = self.build_oai_request(&request);
        // Image generation needs a longer header timeout (up to 180s for first byte)
        let is_image = request.extra.get("n").is_some() && request.extra.get("size").is_some();
        let header_timeout = if is_image { 180 } else { 60 };
        let resp = self
            .send_openai_with_retry(&mut oai_request, header_timeout)
            .await?;

        let body = tokio::time::timeout(
            std::time::Duration::from_secs(LLM_BODY_READ_TIMEOUT_SECS),
            resp.text(),
        )
        .await
        .map_err(|_| {
            LlmError::Http(format!(
                "Response body read timed out after {LLM_BODY_READ_TIMEOUT_SECS}s"
            ))
        })?
        .map_err(|e| LlmError::Http(e.to_string()))?;

        // aginxbrain wraps some responses in {"code":"Success","output":{...}}
        // Try standard OpenAI format first; if missing `choices`, unwrap from `output`
        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| LlmError::Parse(e.to_string()))?;
        let oai_json = if parsed.get("choices").is_some() {
            parsed
        } else if let Some(output) = parsed.get("output") {
            output.clone()
        } else {
            parsed
        };
        let oai_response: OaiResponse =
            serde_json::from_value(oai_json).map_err(|e| LlmError::Parse(e.to_string()))?;

        let choice = oai_response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| LlmError::Parse("No choices in response".to_string()))?;

        let mut content = Vec::new();
        let mut tool_calls = Vec::new();
        let mut media = None;

        if let Some(ref reasoning) = choice.message.reasoning_content {
            let text = extract_reasoning_text(reasoning);
            if !text.is_empty() {
                debug!(len = text.len(), "Captured reasoning_content from response");
                content.push(ContentBlock::Thinking { thinking: text });
            }
        }

        // content can be: String, Array of content parts (OpenAI), or Array with image URLs (aginxbrain)
        if let Some(content_val) = &choice.message.content {
            match content_val {
                serde_json::Value::String(text) => {
                    if !text.is_empty() {
                        let (cleaned, thinking) = extract_think_tags(text);
                        if let Some(think_text) = thinking {
                            if choice.message.reasoning_content.is_none() {
                                content.push(ContentBlock::Thinking {
                                    thinking: think_text,
                                });
                            }
                        }
                        if !cleaned.is_empty() {
                            content.push(ContentBlock::Text {
                                text: cleaned,
                                provider_metadata: None,
                            });
                        }
                    }
                }
                serde_json::Value::Array(parts) => {
                    let mut text_parts = Vec::new();
                    let mut image_urls = Vec::new();
                    for part in parts {
                        if let Some(s) = part.as_str() {
                            text_parts.push(s.to_string());
                        } else if let Some(url) = part.get("image").and_then(|v| v.as_str()) {
                            image_urls.push(url.to_string());
                        } else if let Some(url) = part
                            .get("image_url")
                            .and_then(|v| v.get("url"))
                            .and_then(|v| v.as_str())
                        {
                            image_urls.push(url.to_string());
                        } else if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                            text_parts.push(t.to_string());
                        }
                    }
                    if !text_parts.is_empty() {
                        let text = text_parts.join("");
                        let (cleaned, thinking) = extract_think_tags(&text);
                        if let Some(think_text) = thinking {
                            if choice.message.reasoning_content.is_none() {
                                content.push(ContentBlock::Thinking {
                                    thinking: think_text,
                                });
                            }
                        }
                        if !cleaned.is_empty() {
                            content.push(ContentBlock::Text {
                                text: cleaned,
                                provider_metadata: None,
                            });
                        }
                    }
                    if !image_urls.is_empty() {
                        let items: Vec<carrier_types::media::GeneratedImage> = image_urls
                            .into_iter()
                            .map(|url| carrier_types::media::GeneratedImage {
                                data_base64: String::new(),
                                url: Some(url),
                            })
                            .collect();
                        media = Some(carrier_types::media::MediaOutput::Images { items });
                    }
                }
                _ => {}
            }
        }

        let has_text = content
            .iter()
            .any(|b| matches!(b, ContentBlock::Text { .. }));
        let has_thinking = content
            .iter()
            .any(|b| matches!(b, ContentBlock::Thinking { .. }));
        if has_thinking && !has_text && choice.message.tool_calls.is_none() && media.is_none() {
            let thinking_text = content
                .iter()
                .find_map(|b| match b {
                    ContentBlock::Thinking { thinking } => Some(thinking.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            let summary = extract_thinking_summary(thinking_text);
            debug!(
                summary_len = summary.len(),
                "Synthesizing text from thinking-only response"
            );
            content.push(ContentBlock::Text {
                text: summary,
                provider_metadata: None,
            });
        }

        if let Some(calls) = choice.message.tool_calls {
            for call in calls {
                // Defensive: skip tool_calls with an empty name (see streaming
                // path for rationale) — they can't execute and poison history.
                if call.function.name.is_empty() {
                    warn!(
                        id = %call.id,
                        args_len = call.function.arguments.len(),
                        "Dropping non-streamed tool_call with empty name; skipping to keep history valid"
                    );
                    continue;
                }
                let input: serde_json::Value =
                    serde_json::from_str(&call.function.arguments).unwrap_or_default();
                content.push(ContentBlock::ToolUse {
                    id: call.id.clone(),
                    name: call.function.name.clone(),
                    input: input.clone(),
                    provider_metadata: None,
                });
                tool_calls.push(ToolCall {
                    id: call.id,
                    name: call.function.name,
                    input,
                });
            }
        }

        let stop_reason = match choice.finish_reason.as_deref() {
            Some("stop") => StopReason::EndTurn,
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

        let mut usage = oai_response
            .usage
            .map(|u| TokenUsage {
                input_tokens: u.prompt_tokens,
                output_tokens: u.completion_tokens,
            })
            .unwrap_or_default();

        if !content.is_empty() && usage.input_tokens == 0 && usage.output_tokens == 0 {
            debug!("Response has content but no usage stats — setting synthetic output_tokens=1");
            usage.output_tokens = 1;
        }

        Ok(CompletionResponse {
            content,
            stop_reason,
            tool_calls,
            usage,
            media,
        })
    }

    /// OpenAI-specific retry with request body mutation.
    async fn send_openai_with_retry(
        &self,
        oai_request: &mut OaiRequest,
        header_timeout_secs: u64,
    ) -> Result<reqwest::Response, LlmError> {
        let max_retries: u8 = 3;
        for attempt in 0..=max_retries {
            let url = self.base_url.clone();
            debug!(url = %url, attempt, "Sending OpenAI API request");

            let builder = self
                .client
                .post(&url)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {}", self.api_key.as_str()))
                .json(&*oai_request)
                // Streaming: lift the client-level 180s total timeout. The
                // stream's own idle/first-token timeouts below are the stall
                // detection; a total cap only kills healthy long generations
                // (see LLM_STREAM_TOTAL_TIMEOUT_SECS). Non-streaming requests
                // (complete/image paths) keep the 180s client default.
                .timeout(std::time::Duration::from_secs(if oai_request.stream {
                    LLM_STREAM_TOTAL_TIMEOUT_SECS
                } else {
                    LLM_HTTP_TIMEOUT_SECS
                }));

            // Response-header timeout: defend against servers that accept the
            // connection but never return headers (stalled upstream). We use
            // select! + an independent sleep timer rather than relying on the
            // reqwest client-level timeout, which empirically fails to fire in
            // this stalled-connection state (same class of bug as the streaming
            // idle timeout). 60s is generous for first-byte; genuine responses
            // arrive far faster.
            let header_timeout = std::time::Duration::from_secs(header_timeout_secs);
            let resp = match tokio::select! {
                r = builder.send() => r,
                _ = tokio::time::sleep(header_timeout) => {
                    return Err(LlmError::Http(format!(
                        "Response header timeout: no HTTP response in {}s. \
                         The upstream accepted the connection but appears stalled.",
                        header_timeout.as_secs()
                    )));
                }
            } {
                Ok(r) => r,
                Err(e) => {
                    let err_str = e.to_string();
                    if attempt < max_retries
                        && (err_str.contains("error decoding")
                            || err_str.contains("error sending")
                            || err_str.contains("connection"))
                    {
                        let retry_ms = (attempt as u64 + 1) * 2000;
                        warn!(%err_str, attempt, retry_ms, "HTTP transport error, retrying");
                        tokio::time::sleep(std::time::Duration::from_millis(retry_ms)).await;
                        continue;
                    }
                    return Err(LlmError::Http(err_str));
                }
            };
            let status = resp.status().as_u16();

            if resp.status().is_success() {
                return Ok(resp);
            }

            let body =
                match tokio::time::timeout(std::time::Duration::from_secs(15), resp.text()).await {
                    Ok(Ok(text)) => text,
                    Ok(Err(e)) => {
                        warn!("Error reading error response body: {e}");
                        String::new()
                    }
                    Err(_) => "[body read timed out]".to_string(),
                };

            // Log 400 errors with tool details for debugging provider schema issues
            if status == 400 && body.contains("arguments") && attempt == 0 {
                let problem_tools: Vec<&str> = oai_request
                    .tools
                    .iter()
                    .filter(|t| !t.function.parameters.is_object())
                    .map(|t| t.function.name.as_str())
                    .collect();
                let bad_msg_args: Vec<String> = oai_request
                    .messages
                    .iter()
                    .filter_map(|m| m.tool_calls.as_ref())
                    .flat_map(|calls| calls.iter())
                    .filter(|c| {
                        let s = c.function.arguments.trim();
                        s.is_empty()
                            || s == "null"
                            || serde_json::from_str::<serde_json::Value>(s).is_err()
                    })
                    .map(|c| {
                        format!(
                            "{}: {}...",
                            c.function.name,
                            &c.function.arguments[..c.function.arguments.len().min(80)]
                        )
                    })
                    .collect();
                warn!(
                    status,
                    ?problem_tools,
                    ?bad_msg_args,
                    "Provider rejected tool arguments schema"
                );
            }

            // 429 rate limit
            if status == 429 {
                if attempt < max_retries {
                    let retry_ms = (attempt as u64 + 1) * 2000;
                    warn!(status, retry_ms, "Rate limited, retrying");
                    tokio::time::sleep(std::time::Duration::from_millis(retry_ms)).await;
                    continue;
                }
                return Err(LlmError::RateLimited {
                    retry_after_ms: 5000,
                });
            }

            // Strip temperature for models that don't support it
            if status == 400
                && oai_request.temperature.is_some()
                && attempt < max_retries
                && (body.contains("temperature")
                    && (body.contains("unsupported_parameter") || body.contains("deprecated")))
            {
                warn!(model = %oai_request.model, "Stripping temperature for this model");
                oai_request.temperature = None;
                continue;
            }

            // Auto-cap max_tokens
            if status == 400 && body.contains("max_tokens") && attempt < max_retries {
                let current = oai_request.max_tokens.unwrap_or(4096);
                let cap = extract_max_tokens_limit(&body).unwrap_or(current / 2);
                warn!(
                    old = current,
                    new = cap,
                    "Auto-capping max_tokens to model limit"
                );
                oai_request.max_tokens = Some(cap);
                continue;
            }

            // Retry without tools
            let body_lower = body.to_lowercase();
            if !oai_request.tools.is_empty()
                && attempt < max_retries
                && (status == 500
                    || body_lower.contains("internal error")
                    || (status == 400
                        && (body_lower.contains("does not support tools")
                            || body_lower.contains("tool")
                                && body_lower.contains("not supported"))))
            {
                warn!(model = %oai_request.model, status, "Model may not support tools, retrying without tools");
                oai_request.tools.clear();
                oai_request.tool_choice = None;
                continue;
            }

            return Err(LlmError::Api {
                status,
                message: crate::str_utils::safe_truncate_str(&body, 500).to_string(),
            });
        }

        Err(LlmError::Api {
            status: 0,
            message: "Max retries exceeded".to_string(),
        })
    }
}

// ===========================================================================
// Shared helper functions
// ===========================================================================

/// Maximum idle time (no bytes received) before aborting a streaming response.
const STREAM_IDLE_TIMEOUT_SECS: u64 = 120;

/// Content-level idle timeout for streaming: max seconds without any
/// genuine token output (text/reasoning/tool_call delta), even if the
/// connection keeps sending SSE keepalive frames. Defends against proxies
/// that hold the connection open with comments while the upstream LLM is
/// stalled — byte-level idle timeout cannot detect this.
const STREAM_CONTENT_IDLE_SECS: u64 = 120;

/// First-token timeout: max seconds to wait for the FIRST genuine token of a
/// stream before declaring the upstream stalled and retrying. Tighter than the
/// inter-chunk idle because a genuinely-responsive model emits its first token
/// quickly even when it will think for a while; only a truly stalled upstream
/// (accepted the connection, produces nothing) blows this. Replaces the old
/// non-streaming "60s header" stall detection with streaming's "20s first
/// token". See AginxBrain spec §6 and docs/STREAMING-UNIFICATION.md.
const STREAM_FIRST_TOKEN_SECS: u64 = 20;

/// Read the next chunk, racing against BOTH:
/// - byte-level idle (no bytes at all for STREAM_IDLE_TIMEOUT_SECS)
/// - content-level idle (no meaningful token output for `content_idle`,
///   even if keepalive bytes keep the connection nominally alive)
///
/// `content_idle` is the active content-idle budget: callers pass the tighter
/// STREAM_FIRST_TOKEN_SECS before the first genuine token arrives, then the
/// looser STREAM_CONTENT_IDLE_SECS once the stream is producing tokens.
///
/// The content idle is a `tokio::select!` branch with its own sleep timer,
/// so it fires independently of whether `stream.next()` ever resolves —
/// this is the key difference from wrapping the whole call in
/// `tokio::time::timeout`, which can fail to fire when the reqwest future
/// cannot be dropped mid-await.
async fn next_chunk_with_timeouts(
    stream: &mut (impl futures::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Unpin),
    last_content_at: std::time::Instant,
    content_idle: std::time::Duration,
) -> Result<Option<bytes::Bytes>, LlmError> {
    // How long since the last genuine token output.
    let content_elapsed = last_content_at.elapsed();
    // Hard check: if content idle already exceeded, bail immediately
    // WITHOUT entering select!. This is critical for high-frequency
    // keepalive: if the proxy emits empty frames faster than the sleep
    // timer can fire, select! would always pick the ready chunk branch
    // and the sleep branch would never win. Checking here (before select!)
    // guarantees the timeout fires regardless of keepalive cadence.
    if content_elapsed >= content_idle {
        return Err(LlmError::Http(format!(
            "Streaming content idle timeout: no token output in {}s \
             (only keepalive frames). The upstream LLM appears stalled.",
            content_idle.as_secs()
        )));
    }
    // How long until content idle fires (since last genuine token output).
    let content_remaining = content_idle.saturating_sub(content_elapsed);
    // Byte idle is the outer cap; content idle is usually tighter.
    let byte_cap = std::time::Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS);
    let idle_cap = std::cmp::min(content_remaining, byte_cap);

    tokio::select! {
        chunk_result = stream.next() => match chunk_result {
            Some(Ok(c)) => Ok(Some(c)),
            Some(Err(e)) => Err(LlmError::Http(e.to_string())),
            None => Ok(None),
        },
        _ = tokio::time::sleep(idle_cap) => {
            // Distinguish which timer fired for clearer errors.
            if content_remaining <= byte_cap {
                Err(LlmError::Http(format!(
                    "Streaming content idle timeout: no token output in {}s \
                     (only keepalive frames). The upstream LLM appears stalled.",
                    content_idle.as_secs()
                )))
            } else {
                Err(LlmError::Http(format!(
                    "Streaming idle timeout: no data received in {STREAM_IDLE_TIMEOUT_SECS}s"
                )))
            }
        }
    }
}

/// Extract think tags from content text, returning (cleaned_text, thinking_content).
fn extract_think_tags(text: &str) -> (String, Option<String>) {
    let mut thinking_parts = Vec::new();
    let mut cleaned = String::with_capacity(text.len());
    let mut remaining = text;
    let open_tag = "<think>";
    let close_tag = "</think>";

    while let Some(start) = remaining.find(open_tag) {
        cleaned.push_str(&remaining[..start]);
        let after_open = start + open_tag.len();

        if let Some(end) = remaining[after_open..].find(close_tag) {
            let think_text = remaining[after_open..after_open + end].trim();
            if !think_text.is_empty() {
                thinking_parts.push(think_text.to_string());
            }
            remaining = &remaining[after_open + end + close_tag.len()..];
        } else {
            let thought = remaining[after_open..].trim();
            if !thought.is_empty() {
                thinking_parts.push(thought.to_string());
            }
            break;
        }
    }

    cleaned.push_str(remaining);

    let thinking = if thinking_parts.is_empty() {
        None
    } else {
        Some(thinking_parts.join("\n\n"))
    };

    (cleaned.trim().to_string(), thinking)
}

/// Pieces of a single complete (non-streaming) completion body — the shape
/// brains that ignore the `stream` parameter return (AginxOS brain,
/// aginxos/docs/CARRIER.md §3.2).
struct NonStreamCompletion {
    text: String,
    reasoning: Option<String>,
    tool_calls: Vec<(String, String, String)>,
    finish_reason: Option<String>,
    usage_input: u64,
    usage_output: u64,
}

/// Parse one complete OpenAI-format JSON body. Mirrors `complete_openai`:
/// unwraps the aginxbrain `{"code":"Success","output":{...}}` envelope,
/// accepts `content` as String or array-of-parts, reads tool_calls and usage.
/// Returns None when the body isn't a completion (no `choices` — garbage,
/// error page), letting the caller decide how to surface that.
fn parse_non_stream_completion(body: &str) -> Option<NonStreamCompletion> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let oai_json = if parsed.get("choices").is_some() {
        parsed
    } else if let Some(output) = parsed.get("output") {
        output.clone()
    } else {
        parsed
    };
    let choices = oai_json.get("choices")?.as_array()?;
    let choice = choices.first()?;
    let message = choice.get("message")?;

    let mut text = String::new();
    match message.get("content") {
        Some(serde_json::Value::String(s)) => text.push_str(s),
        Some(serde_json::Value::Array(parts)) => {
            for part in parts {
                if let Some(s) = part.as_str() {
                    text.push_str(s);
                } else if let Some(t) = part.get("text").and_then(|v| v.as_str()) {
                    text.push_str(t);
                }
            }
        }
        _ => {}
    }

    let reasoning = message
        .get("reasoning_content")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());

    let mut tool_calls = Vec::new();
    if let Some(calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
        for call in calls {
            let func = call.get("function");
            tool_calls.push((
                call.get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                func.and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                func.and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
    }

    let finish_reason = choice
        .get("finish_reason")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let usage = oai_json.get("usage");
    let usage_input = usage
        .and_then(|u| u.get("prompt_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let usage_output = usage
        .and_then(|u| u.get("completion_tokens"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Some(NonStreamCompletion {
        text,
        reasoning,
        tool_calls,
        finish_reason,
        usage_input,
        usage_output,
    })
}

/// Extract a brief summary from thinking-only content.
fn extract_thinking_summary(thinking: &str) -> String {
    let paragraphs: Vec<&str> = thinking
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
        .collect();
    if let Some(last) = paragraphs.last() {
        let trimmed = last.trim();
        if trimmed.len() > 200 {
            let end = trimmed
                .char_indices()
                .take_while(|(i, _)| *i < 200)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);
            format!("{}...", &trimmed[..end])
        } else {
            trimmed.to_string()
        }
    } else {
        "Thinking complete.".to_string()
    }
}

/// Extract max_tokens limit from error body.
fn extract_max_tokens_limit(body: &str) -> Option<u32> {
    let idx = body.find("max_tokens")?;
    let after = &body[idx + "max_tokens".len()..];
    let start = after.find(|c: char| c.is_ascii_digit())?;
    let digits: String = after[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

// ===========================================================================
// LlmDriver implementation
// ===========================================================================

#[async_trait]
impl LlmDriver for UnifiedHttpDriver {
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        self.complete_openai(request).await
    }

    async fn stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<CompletionResponse, LlmError> {
        let mut oai_request = self.build_oai_request(&request);
        oai_request.stream = true;
        oai_request.stream_options = Some(serde_json::json!({"include_usage": true}));

        let resp = self.send_openai_with_retry(&mut oai_request, STREAM_HEADER_TIMEOUT_SECS).await?;

        let mut buffer = String::new();
        let mut text_content = String::new();
        let mut reasoning_content = String::new();
        let mut think_filter = StreamingThinkFilter::new();
        let mut tool_accum: Vec<(String, String, String)> = Vec::new();
        let mut finish_reason: Option<String> = None;
        let mut usage = TokenUsage::default();

        // Content-level idle tracking: catches proxies that keep the
        // connection alive with SSE comments/empty frames while the
        // upstream LLM is stalled (byte-level idle timeout won't fire
        // because keepalive bytes keep arriving). Reset only when we
        // receive genuine token output (text/reasoning/tool_call delta
        // — NOT empty `{"choices":[{"delta":{}}]}` frames).
        let mut last_content_at = std::time::Instant::now();
        // Whether we have seen the first genuine token yet. Until then we use
        // a tighter first-token budget (STREAM_FIRST_TOKEN_SECS) so a stalled
        // upstream is detected quickly and retried, instead of waiting the full
        // inter-chunk idle. Flipped to true on the first real token.
        let mut got_first_token = false;
        // Whether any SSE `data:` frame was seen. A brain that ignores the
        // `stream` parameter answers with a single complete JSON body — no
        // `data:` lines — and the SSE parser below would silently skip every
        // line of it (turning the whole turn into an empty EndTurn). The
        // non-SSE fallback after the loop keys off this flag.
        let mut saw_sse_frame = false;
        // Lines the SSE parser skipped, kept ONLY while no SSE frame has been
        // seen — that body may be the brain's single complete JSON response.
        let mut non_sse_body = String::new();

        let mut byte_stream = resp.bytes_stream();
        while let Some(chunk) = next_chunk_with_timeouts(
            &mut byte_stream,
            last_content_at,
            if got_first_token {
                std::time::Duration::from_secs(STREAM_CONTENT_IDLE_SECS)
            } else {
                std::time::Duration::from_secs(STREAM_FIRST_TOKEN_SECS)
            },
        )
        .await?
        {
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find('\n') {
                let line = buffer[..pos].trim_end().to_string();
                buffer = buffer[pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                let data = match line.strip_prefix("data:") {
                    Some(d) => d.trim_start(),
                    None => {
                        // Not SSE framing. Until a real frame proves otherwise,
                        // this could be a brain that ignored `stream` and sent
                        // one complete JSON body (CARRIER.md §3.2) — stash the
                        // line for the non-SSE fallback after the loop.
                        if !saw_sse_frame {
                            non_sse_body.push_str(&line);
                            non_sse_body.push('\n');
                        }
                        continue;
                    }
                };
                if data == "[DONE]" {
                    continue;
                }

                let json: serde_json::Value = match serde_json::from_str(data) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if let Some(u) = json.get("usage") {
                    if let Some(pt) = u["prompt_tokens"].as_u64() {
                        usage.input_tokens = pt;
                    }
                    if let Some(ct) = u["completion_tokens"].as_u64() {
                        usage.output_tokens = ct;
                    }
                }

                let choices = match json["choices"].as_array() {
                    Some(c) => c,
                    None => continue,
                };
                saw_sse_frame = true;
                non_sse_body.clear();

                // Determine if this frame carries genuine token output
                // (non-empty content/reasoning/tool_call). Only genuine
                // tokens reset the content-idle timer — empty frames like
                // `{"choices":[{"delta":{}}]}` must NOT, or a stalled proxy
                // emitting empty frames would defeat the idle timeout.
                let mut got_real_content = false;

                for choice in choices {
                    let delta = &choice["delta"];

                    if let Some(text) = delta["content"].as_str() {
                        if !text.is_empty() {
                            got_real_content = true;
                            text_content.push_str(text);
                            for action in think_filter.process(text) {
                                match action {
                                    FilterAction::EmitText(t) => {
                                        let _ = tx.send(StreamEvent::TextDelta { text: t }).await;
                                    }
                                    FilterAction::EmitThinking(t) => {
                                        let _ =
                                            tx.send(StreamEvent::ThinkingDelta { text: t }).await;
                                    }
                                }
                            }
                        }
                    }

                    if let Some(reasoning) = delta["reasoning_content"].as_str() {
                        if !reasoning.is_empty() {
                            got_real_content = true;
                            reasoning_content.push_str(reasoning);
                            let _ = tx
                                .send(StreamEvent::ThinkingDelta {
                                    text: reasoning.to_string(),
                                })
                                .await;
                        }
                    }

                    if let Some(calls) = delta["tool_calls"].as_array() {
                        if !calls.is_empty() {
                            got_real_content = true;
                        }
                        for call in calls {
                            let idx = call["index"].as_u64().unwrap_or(0) as usize;
                            if idx > 100 {
                                warn!(idx = idx, "tool_calls index exceeds 100, skipping");
                                continue;
                            }
                            while tool_accum.len() <= idx {
                                tool_accum.push((String::new(), String::new(), String::new()));
                            }
                            if let Some(id) = call["id"].as_str() {
                                tool_accum[idx].0 = id.to_string();
                            }
                            if let Some(func) = call.get("function") {
                                if let Some(name) = func["name"].as_str() {
                                    tool_accum[idx].1 = name.to_string();
                                    let _ = tx
                                        .send(StreamEvent::ToolUseStart {
                                            id: tool_accum[idx].0.clone(),
                                            name: name.to_string(),
                                        })
                                        .await;
                                }
                                if let Some(args) = func["arguments"].as_str() {
                                    tool_accum[idx].2.push_str(args);
                                    let _ = tx
                                        .send(StreamEvent::ToolInputDelta {
                                            text: args.to_string(),
                                        })
                                        .await;
                                }
                            }
                        }
                    }

                    if let Some(fr) = choice["finish_reason"].as_str() {
                        if !fr.is_empty() {
                            finish_reason = Some(fr.to_string());
                        }
                    }
                }

                // Reset content-idle timer only when genuine token output
                // arrived this frame. finish_reason alone (end of stream)
                // also counts as progress so we don't time out on the
                // final frame.
                if got_real_content || finish_reason.is_some() {
                    last_content_at = std::time::Instant::now();
                    // First genuine token switches us from the tight first-token
                    // budget to the looser inter-chunk idle budget.
                    if got_real_content {
                        got_first_token = true;
                    }
                }
            }
        }

        // Non-SSE fallback: the upstream ignored `stream` and answered with
        // one complete JSON body (AginxOS brain behavior — CARRIER.md §3.2).
        // Parse it and populate the same accumulators the SSE path fills, so
        // downstream consumers see one big text/tool "chunk" and the final
        // CompletionResponse is built from real content instead of silence.
        if !saw_sse_frame {
            // A body without a trailing newline leaves its last line in
            // `buffer` (the line loop only consumes complete lines).
            if !buffer.is_empty() {
                non_sse_body.push_str(&buffer);
            }
            match parse_non_stream_completion(&non_sse_body) {
                Some(ns) => {
                    if let Some(reasoning) = ns.reasoning {
                        if !reasoning.is_empty() {
                            reasoning_content = reasoning.clone();
                            let _ = tx
                                .send(StreamEvent::ThinkingDelta { text: reasoning })
                                .await;
                        }
                    }
                    if !ns.text.is_empty() {
                        text_content = ns.text.clone();
                        for action in think_filter.process(&ns.text) {
                            match action {
                                FilterAction::EmitText(t) => {
                                    let _ = tx.send(StreamEvent::TextDelta { text: t }).await;
                                }
                                FilterAction::EmitThinking(t) => {
                                    let _ =
                                        tx.send(StreamEvent::ThinkingDelta { text: t }).await;
                                }
                            }
                        }
                    }
                    for (id, name, args) in &ns.tool_calls {
                        tool_accum.push((id.clone(), name.clone(), args.clone()));
                        let _ = tx
                            .send(StreamEvent::ToolUseStart {
                                id: id.clone(),
                                name: name.clone(),
                            })
                            .await;
                        let _ = tx
                            .send(StreamEvent::ToolInputDelta { text: args.clone() })
                            .await;
                    }
                    if let Some(fr) = ns.finish_reason {
                        finish_reason = Some(fr);
                    }
                    usage.input_tokens = ns.usage_input;
                    usage.output_tokens = ns.usage_output;
                }
                None => {
                    if !non_sse_body.trim().is_empty() {
                        warn!(
                            len = non_sse_body.len(),
                            "Non-SSE response body did not parse as a completion (no SSE frames seen)"
                        );
                    }
                }
            }
        }

        // Flush think filter
        for action in think_filter.flush() {
            match action {
                FilterAction::EmitText(t) => {
                    let _ = tx.send(StreamEvent::TextDelta { text: t }).await;
                }
                FilterAction::EmitThinking(t) => {
                    let _ = tx.send(StreamEvent::ThinkingDelta { text: t }).await;
                }
            }
        }

        // Build content
        let mut content = Vec::new();
        let mut tool_calls = Vec::new();

        if !reasoning_content.is_empty() {
            content.push(ContentBlock::Thinking {
                thinking: reasoning_content,
            });
        }

        if !text_content.is_empty() {
            let (clean_text, thinking) = extract_think_tags(&text_content);
            if let Some(th) = thinking {
                content.push(ContentBlock::Thinking { thinking: th });
            }
            if !clean_text.is_empty() {
                content.push(ContentBlock::Text {
                    text: clean_text,
                    provider_metadata: None,
                });
            }
        }

        for (id, name, args_json) in &tool_accum {
            // Defensive: a tool_call must have a name. The gateway occasionally
            // drops the function.name delta in streamed tool_calls (an edge case
            // of the SSE tool-call conversion — see AginxBrain spec §5.4),
            // leaving an empty name. Such a call cannot be executed and, worse,
            // poisons the conversation history: the next request echoes it back
            // and the API rejects it with "Invalid 'name' for function_call".
            // Skip it and warn so we can tell if this becomes frequent.
            if name.is_empty() {
                warn!(
                    id = %id,
                    args_len = args_json.len(),
                    "Dropping streamed tool_call with empty name (gateway dropped the name delta); skipping to keep history valid"
                );
                continue;
            }
            let input: serde_json::Value = serde_json::from_str(args_json)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            content.push(ContentBlock::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
                provider_metadata: None,
            });
            tool_calls.push(ToolCall {
                id: id.clone(),
                name: name.clone(),
                input,
            });
            let _ = tx
                .send(StreamEvent::ToolUseEnd {
                    id: id.clone(),
                    name: name.clone(),
                    input: serde_json::from_str(args_json).unwrap_or_default(),
                })
                .await;
        }

        if content.is_empty() && tool_calls.is_empty() {
            content.push(ContentBlock::Text {
                text: text_content.clone(),
                provider_metadata: None,
            });
        }

        let stop_reason = match finish_reason.as_deref() {
            Some("stop") => StopReason::EndTurn,
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

        if usage.output_tokens == 0 && (!content.is_empty() || !tool_accum.is_empty()) {
            usage.output_tokens = 1;
        }

        let response = CompletionResponse {
            content,
            stop_reason,
            tool_calls,
            usage,
            media: None,
        };
        let _ = tx
            .send(StreamEvent::ContentComplete {
                stop_reason: response.stop_reason,
                usage: response.usage,
            })
            .await;
        Ok(response)
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_think_tags() {
        let (cleaned, thinking) = extract_think_tags("hello world");
        assert_eq!(cleaned, "hello world");
        assert!(thinking.is_none());
    }

    #[test]
    fn test_extract_thinking_summary() {
        let summary = extract_thinking_summary("Line one\n\nLine two\n\nLine three");
        assert_eq!(summary, "Line three");
    }

    #[test]
    fn test_mime_to_audio_format() {
        assert_eq!(mime_to_audio_format("audio/mpeg"), "mp3");
        assert_eq!(mime_to_audio_format("audio/wav"), "wav");
        assert_eq!(mime_to_audio_format("audio/unknown"), "mp3");
    }

    // --- non-SSE single-body fallback (CARRIER.md §3.2) ---

    #[test]
    fn test_parse_non_stream_completion_standard() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"hello","reasoning_content":"thinking...","tool_calls":[{"id":"c1","type":"function","function":{"name":"web_search","arguments":"{\"q\":\"x\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
        let ns = parse_non_stream_completion(body).expect("standard body should parse");
        assert_eq!(ns.text, "hello");
        assert_eq!(ns.reasoning.as_deref(), Some("thinking..."));
        assert_eq!(
            ns.tool_calls,
            vec![("c1".to_string(), "web_search".to_string(), "{\"q\":\"x\"}".to_string())]
        );
        assert_eq!(ns.finish_reason.as_deref(), Some("tool_calls"));
        assert_eq!((ns.usage_input, ns.usage_output), (10, 5));
    }

    #[test]
    fn test_parse_non_stream_completion_output_envelope() {
        // aginxbrain wraps some responses in {"code":"Success","output":{...}}
        let body = r#"{"code":"Success","output":{"choices":[{"message":{"content":"wrapped"},"finish_reason":"stop"}]}}"#;
        let ns = parse_non_stream_completion(body).expect("enveloped body should parse");
        assert_eq!(ns.text, "wrapped");
        assert_eq!(ns.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn test_parse_non_stream_completion_content_parts() {
        let body = r#"{"choices":[{"message":{"content":[{"type":"text","text":"part one "},{"type":"text","text":"part two"}]},"finish_reason":"stop"}]}"#;
        let ns = parse_non_stream_completion(body).expect("content-parts body should parse");
        assert_eq!(ns.text, "part one part two");
    }

    #[test]
    fn test_parse_non_stream_completion_no_trailing_newline_or_with() {
        // Bodies arrive both with and without a trailing newline depending on
        // the server — serde_json tolerates trailing whitespace either way.
        for body in [
            r#"{"choices":[{"message":{"content":"a"},"finish_reason":"stop"}]}"#,
            "{\"choices\":[{\"message\":{\"content\":\"a\"},\"finish_reason\":\"stop\"}]}\n",
        ] {
            let ns = parse_non_stream_completion(body).expect("body should parse");
            assert_eq!(ns.text, "a");
        }
    }

    #[test]
    fn test_parse_non_stream_completion_garbage_returns_none() {
        assert!(parse_non_stream_completion("").is_none());
        assert!(parse_non_stream_completion("not json at all").is_none());
        // JSON but no choices (error page shape) → None, caller warns
        assert!(parse_non_stream_completion(r#"{"error":"bad key"}"#).is_none());
    }
}
