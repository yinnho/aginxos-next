//! Shared helper functions for the agent loop.
//!
//! Contains retry logic, fallback chain, loop detection, turn trimming,
//! and turn summary generation — pure utilities used by the main loop
//! and its branch handlers.

use crate::auth_cooldown::{CooldownVerdict, ProviderCooldown};
use crate::llm_driver::{
    Brain, CompletionRequest, CompletionResponse, LlmDriver, LlmError, StreamEvent,
};
use crate::llm_errors;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::message::{ContentBlock, Message, MessageContent, Role, TurnSummary};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum retries for rate-limited or overloaded API calls.
pub const MAX_RETRIES: u32 = 3;

/// Base delay for exponential backoff (milliseconds).
pub const BASE_RETRY_DELAY_MS: u64 = 1000;

/// Wall-clock timeout for streaming LLM calls (seconds).
/// Even though the driver has a per-chunk idle timeout (120s), keepalive bytes
/// or very slow responses can defeat it. This provides a hard upper bound.
/// A healthy stream producing tokens has no budget problem (the driver's
/// layered idle timeouts are the real stall detection); the old min(180, …)
/// total cap killed legitimate long generations mid-stream — clone-creation
/// turns writing whole definition files routinely stream 200-400s. 1800s is a zombie backstop only; the outer daemon backstop + turn progress
/// detection bound the rest.
pub(in crate::agent_loop) const STREAM_WALL_CLOCK_TIMEOUT_SECS: u64 = 1800;

/// Max tokens for turn summary generation.
pub(in crate::agent_loop) const SUMMARY_MAX_TOKENS: u32 = 150;

/// Summary modality (fast/cheap).
pub(in crate::agent_loop) const SUMMARY_MODALITY: &str = "fast";

/// Reasoning modality — expensive model for planning and complex inference.
pub(in crate::agent_loop) const REASONING_MODALITY: &str = "reasoning";

/// Pick the modality for the current agent loop iteration.
///
/// Single-model strategy (see `docs/AGENT-LOOP-REMEDIATION.md`, Phase 1): use
/// `reasoning` for *every* turn. The previous turn-parity alternation (even →
/// reasoning, odd → chat) caused inconsistent capability and unreliable tool
/// use, because the weaker `chat` model received tool schemas and a system
/// prompt designed for the strong model. One capable model for the whole loop
/// keeps behavior consistent. If `reasoning` is unavailable, fall back to the
/// default modality.
pub(in crate::agent_loop) fn pick_modality(
    brain: Option<&std::sync::Arc<dyn crate::llm_driver::Brain>>,
    _iteration: u32,
    default_modality: &str,
) -> String {
    let Some(brain) = brain else {
        return default_modality.to_string();
    };
    if brain.has_modality(REASONING_MODALITY) {
        tracing::info!(
            selected = REASONING_MODALITY,
            default = default_modality,
            "Single-model modality: reasoning (turn-parity alternation removed)"
        );
        return REASONING_MODALITY.to_string();
    }
    tracing::info!(
        selected = default_modality,
        "Single-model modality: reasoning unavailable, falling back to default"
    );
    default_modality.to_string()
}

/// Timeout for individual tool executions (seconds).
/// Raised from 60s to 120s for browser automation and long-running builds.
pub const TOOL_TIMEOUT_SECS: u64 = 120;

/// Tools that need a longer timeout (image generation, browser automation, and
/// shell_exec — flow scripts make slow brain API image/video/vision calls that
/// exceed the default 120s; the inner shell.rs timeout also rises for
/// flow-scoped shell_allow calls).
pub const TOOL_TIMEOUT_LONG_SECS: u64 = 300;
pub const TOOL_LONG_TIMEOUT_NAMES: &[&str] = &[
    "image_generate",
    "browser_navigate",
    "browser_execute",
    "shell_exec",
];

/// Maximum message history size before auto-trimming to prevent context overflow.
pub const MAX_HISTORY_MESSAGES: usize = 30;

/// Hard loop detection window: same (tool_name, input_hash) repeated this many
/// times triggers tool removal. Reduced from 6 to 4 to cut wasted iterations.
/// Below 4 risks blocking legitimate retries (e.g. eventual-consistency reads).
///
/// Same-name-different-input (e.g. paginated search) is NOT a hard loop.
pub const LOOP_DETECTION_WINDOW: usize = 4;

/// Soft loop detection window: same tool_name (regardless of input) called this
/// many consecutive times triggers a gentle reminder in the loop status message,
/// without removing the tool.
pub const SOFT_LOOP_WINDOW: usize = 2;

/// Cumulative (whole-turn) repetition thresholds — progressive, educate first
/// (dsh guard/repeat-tool-reminder cadence): at 3 identical calls inject a
/// reminder carrying the args preview, at 5 escalate the wording, at 8 abort
/// the turn. Counts accumulate per (normalized name, input hash) across the
/// WHOLE turn, even when interleaved with other calls so the consecutive
/// `LOOP_DETECTION_WINDOW` never fills — catches ROTATING repetition (e.g.
/// file_read on 4 paths cycled) that the consecutive-only detector misses.
///
/// Calls that are DENIED or error out still count: a model hammering a
/// denied call (allowlist wall, missing tool) is exactly the loop worth
/// breaking (dsh rationale: "a model hammering a denied call is exactly the
/// loop worth breaking").
pub const CUMULATIVE_REMIND_AT: u32 = 3;
/// Second cumulative stage — escalated wording after the first reminder was
/// ignored (see [`CUMULATIVE_REMIND_AT`]).
pub const CUMULATIVE_ESCALATE_AT: u32 = 5;
/// Final cumulative stage — abort the turn. 8 identical calls with two
/// ignored warnings is stuck by any measure; guidance does not fix
/// input-cycling (the 16:29 ai-writer turn had soft-loop reminders and still
/// burned 600s).
pub const CUMULATIVE_BREAK_AT: u32 = 8;

/// Default context window size (tokens) for token-based trimming.
pub(in crate::agent_loop) const DEFAULT_CONTEXT_WINDOW: usize = 128_000;

// ---------------------------------------------------------------------------
// Loop detection
// ---------------------------------------------------------------------------

/// Hash a tool input value for loop detection. Two calls with the same hash
/// are considered identical for loop-detection purposes.
///
/// Canonical by construction: serde_json's default map is a BTreeMap, so
/// object keys serialize in sorted order no matter the order the model
/// emitted them — `{"a":1,"b":2}` and `{"b":2,"a":1}` hash identically,
/// nested objects included. Array order stays significant (it is semantic).
pub fn tool_input_hash(input: &serde_json::Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let serialized = serde_json::to_string(input).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    serialized.hash(&mut hasher);
    hasher.finish()
}

/// Loop-detection key for a tool call: `(normalized name, input hash)`.
///
/// The name goes through the same normalization as dispatch
/// (`normalize_tool_name`), so a free-text recovery artifact like `web_search,`
/// and a clean `web_search` count as the SAME call — without this the
/// cumulative/consecutive counters reset on every dirty name and miss the
/// loop.
pub fn tool_call_key(name: &str, input: &serde_json::Value) -> (String, u64) {
    (
        carrier_types::tool_compat::normalize_tool_name(name).to_string(),
        tool_input_hash(input),
    )
}

/// Short human-readable preview of a tool input for loop-warning messages.
pub fn tool_args_preview(input: &serde_json::Value, max_chars: usize) -> String {
    let s = serde_json::to_string(input).unwrap_or_default();
    if s.chars().count() <= max_chars {
        s
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{truncated}…")
    }
}

/// Detect a tool-use loop: returns the (name, input_hash) of the looping call
/// if the last `window` entries are all the same (name, input_hash), else None.
pub fn detect_tool_loop(recent: &[(String, u64)], window: usize) -> Option<(String, u64)> {
    if recent.len() < window {
        return None;
    }
    let tail = &recent[recent.len() - window..];
    let first = &tail[0];
    if tail.iter().all(|entry| entry == first) {
        Some(first.clone())
    } else {
        None
    }
}

/// Detect a soft loop: same tool name (regardless of input) called `window`
/// consecutive times at the tail of `recent`. Returns the tool name if so.
pub fn detect_soft_loop(recent: &[(String, u64)], window: usize) -> Option<String> {
    if recent.len() < window {
        return None;
    }
    let tail = &recent[recent.len() - window..];
    let first_name = &tail[0].0;
    if tail.iter().all(|(name, _)| name == first_name) {
        Some(first_name.clone())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Session sync (single-track messages)
// ---------------------------------------------------------------------------

/// Sync loop working messages back to session.messages.
///
/// During the loop, only `messages` (the working copy) is updated. This
/// function copies the loop's working messages back to `session.messages`
/// before any session save operation, so `session.messages` always reflects
/// the full conversation state.
///
/// System-role messages are filtered out — they are loop-internal hints and
/// should not be persisted to the session.
pub(in crate::agent_loop) fn sync_loop_messages(
    messages: &[Message],
    session: &mut carrier_memory::session::Session,
    session_base_len: usize,
) {
    let loop_msgs: Vec<Message> = messages
        .iter()
        .filter(|m| m.role != Role::System)
        .cloned()
        .collect();
    session.messages.truncate(session_base_len);
    session.messages.extend(loop_msgs);
}

// ---------------------------------------------------------------------------
// LLM retry / fallback
// ---------------------------------------------------------------------------

/// Call an LLM driver with automatic retry on rate-limit and overload errors.
///
/// Uses the `llm_errors` classifier for smart error handling and the
/// `ProviderCooldown` circuit breaker to prevent request storms.
///
/// If `llm_concurrency_limit` is provided, a semaphore permit is acquired
/// before each LLM call attempt and released immediately after. This ensures
/// the concurrency slot is only held during actual API work, not during
/// tool execution or session management.
pub(in crate::agent_loop) async fn call_with_retry(
    driver: &dyn LlmDriver,
    request: CompletionRequest,
    stream_tx: Option<mpsc::Sender<StreamEvent>>,
    provider: Option<&str>,
    cooldown: Option<&ProviderCooldown>,
    llm_concurrency_limit: Option<&Arc<tokio::sync::Semaphore>>,
) -> CarrierResult<CompletionResponse> {
    let is_stream = stream_tx.is_some();

    // Check circuit breaker before calling
    if let (Some(provider), Some(cooldown)) = (provider, cooldown) {
        match cooldown.check(provider) {
            CooldownVerdict::Reject {
                reason,
                retry_after_secs,
            } => {
                return Err(CarrierError::LlmDriver(format!(
                    "Provider '{provider}' is in cooldown ({reason}). Retry in {retry_after_secs}s."
                )));
            }
            CooldownVerdict::AllowProbe => {
                debug!(
                    provider,
                    is_stream, "Allowing probe request through circuit breaker"
                );
            }
            CooldownVerdict::Allow => {}
        }
    }

    let mut last_error = None;

    for attempt in 0..=MAX_RETRIES {
        // Per-call stall timeout semantics: the loop always streams, so the
        // governing detection is the driver's layered idles (first-token 20s,
        // inter-chunk content-idle 120s). The outer timeout below is only a
        // zombie-stream backstop, deliberately NOT a 180s total cap (that
        // killed healthy long generations mid-stream — see
        // STREAM_WALL_CLOCK_TIMEOUT_SECS).

        // The agent loop always streams internally. `stream_tx` only controls
        // whether incremental events are ALSO forwarded to a consumer (e.g. the
        // dashboard SSE/WS client). When there is no consumer we still stream —
        // draining into an internal sink — so every call (including all channel
        // production traffic) benefits from streaming's idle-timeout resilience
        // instead of non-streaming's 60s header timeout. See
        // docs/STREAMING-UNIFICATION.md.
        let call = async {
            match &stream_tx {
                Some(tx) => driver.stream(request.clone(), tx.clone()).await,
                None => {
                    let (drain_tx, mut drain_rx) = mpsc::channel::<StreamEvent>(64);
                    let drain =
                        tokio::spawn(async move { while drain_rx.recv().await.is_some() {} });
                    let result = driver.stream(request.clone(), drain_tx).await;
                    drain.abort();
                    result
                }
            }
        };

        // Acquire per-call concurrency permit so the slot is only held
        // during actual API work, not across the entire agent loop.
        let _permit = if let Some(sem) = llm_concurrency_limit {
            match tokio::time::timeout(std::time::Duration::from_secs(30), sem.acquire()).await {
                Ok(Ok(permit)) => Some(permit),
                Ok(Err(_)) => {
                    last_error = Some("LLM concurrency semaphore closed".to_string());
                    if attempt == MAX_RETRIES {
                        return Err(CarrierError::RateLimited);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(
                        BASE_RETRY_DELAY_MS * 2u64.pow(attempt),
                    ))
                    .await;
                    continue;
                }
                Err(_) => {
                    warn!("LLM concurrency permit acquisition timed out — all slots occupied");
                    last_error = Some("All LLM concurrency slots occupied".to_string());
                    if attempt == MAX_RETRIES {
                        return Err(CarrierError::Internal(
                            "All LLM concurrency slots occupied — try again in a moment"
                                .to_string(),
                        ));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(
                        BASE_RETRY_DELAY_MS * 2u64.pow(attempt),
                    ))
                    .await;
                    continue;
                }
            }
        } else {
            None
        };

        // The loop always streams now (see call above), so always use the
        // streaming wall-clock backstop: long generations are allowed as long
        // as the stream keeps producing tokens (idle timeouts in the driver
        // catch real stalls); this bound only prevents indefinite hangs from
        // zombie streams that keep emitting *some* token forever.
        let timeout = std::time::Duration::from_secs(STREAM_WALL_CLOCK_TIMEOUT_SECS);
        let result = match tokio::time::timeout(timeout, call).await {
            Ok(r) => r,
            Err(_) => {
                warn!(
                    attempt,
                    timeout_secs = timeout.as_secs(),
                    "LLM call timed out (wall-clock)"
                );
                last_error = Some("LLM call timed out".to_string());
                if attempt == MAX_RETRIES {
                    return Err(CarrierError::LlmDriver(format!(
                        "LLM call timed out after {}s (wall-clock) — server may be unresponsive",
                        timeout.as_secs()
                    )));
                }
                tokio::time::sleep(std::time::Duration::from_millis(
                    BASE_RETRY_DELAY_MS * 2u64.pow(attempt),
                ))
                .await;
                continue;
            }
        };
        match result {
            Ok(response) => {
                if let (Some(provider), Some(cooldown)) = (provider, cooldown) {
                    cooldown.record_success(provider);
                }
                return Ok(response);
            }
            Err(LlmError::RateLimited { retry_after_ms }) => {
                if attempt == MAX_RETRIES {
                    if let (Some(provider), Some(cooldown)) = (provider, cooldown) {
                        cooldown.record_failure(provider, false);
                    }
                    return Err(CarrierError::LlmDriver(format!(
                        "Rate limited after {} retries",
                        MAX_RETRIES
                    )));
                }
                let delay = std::cmp::max(retry_after_ms, BASE_RETRY_DELAY_MS * 2u64.pow(attempt));
                warn!(
                    attempt,
                    delay_ms = delay,
                    is_stream,
                    "Rate limited, retrying after delay"
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                last_error = Some("Rate limited".to_string());
            }
            Err(LlmError::Overloaded { retry_after_ms }) => {
                if attempt == MAX_RETRIES {
                    if let (Some(provider), Some(cooldown)) = (provider, cooldown) {
                        cooldown.record_failure(provider, false);
                    }
                    return Err(CarrierError::LlmDriver(format!(
                        "Model overloaded after {} retries",
                        MAX_RETRIES
                    )));
                }
                let delay = std::cmp::max(retry_after_ms, BASE_RETRY_DELAY_MS * 2u64.pow(attempt));
                warn!(
                    attempt,
                    delay_ms = delay,
                    is_stream,
                    "Model overloaded, retrying after delay"
                );
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                last_error = Some("Overloaded".to_string());
            }
            Err(e) => {
                let raw_error = e.to_string();
                let status = match &e {
                    LlmError::Api { status, .. } => Some(*status),
                    _ => None,
                };
                let classified = llm_errors::classify_error(&raw_error, status);
                warn!(
                    category = ?classified.category,
                    retryable = classified.is_retryable,
                    raw = %raw_error,
                    is_stream,
                    "LLM error classified: {}",
                    classified.sanitized_message
                );

                if let (Some(provider), Some(cooldown)) = (provider, cooldown) {
                    cooldown.record_failure(provider, classified.is_billing);
                }

                let user_msg = if classified.category == llm_errors::LlmErrorCategory::Format {
                    format!("{} — raw: {}", classified.sanitized_message, raw_error)
                } else {
                    classified.sanitized_message
                };
                return Err(CarrierError::LlmDriver(user_msg));
            }
        }
    }

    Err(CarrierError::LlmDriver(
        last_error.unwrap_or_else(|| "Unknown error".to_string()),
    ))
}

/// Call LLM with unified fallback across Brain endpoints.
/// When `stream_tx` is `Some`, uses streaming mode; otherwise non-streaming.
pub(in crate::agent_loop) async fn call_with_fallback(
    brain: Option<&Arc<dyn Brain>>,
    fallback_driver: &dyn LlmDriver,
    modality: &str,
    request: CompletionRequest,
    stream_tx: Option<mpsc::Sender<StreamEvent>>,
    llm_concurrency_limit: Option<&Arc<tokio::sync::Semaphore>>,
) -> CarrierResult<CompletionResponse> {
    let Some(brain) = brain else {
        return call_with_retry(
            fallback_driver,
            request,
            stream_tx,
            None,
            None,
            llm_concurrency_limit,
        )
        .await;
    };

    let endpoints = brain.endpoints_for(modality);
    if endpoints.is_empty() {
        return Err(CarrierError::LlmDriver(format!(
            "No available endpoints for modality '{modality}' — all endpoints circuit-broken or not configured"
        )));
    }

    let mut last_error: Option<CarrierError> = None;
    for ep in &endpoints {
        if let Some(driver) = brain.driver_for_endpoint(&ep.id) {
            let mut req = request.clone();
            req.model = ep.model.clone();
            let start = std::time::Instant::now();
            let tx_arg = stream_tx.clone();
            match call_with_retry(
                &*driver,
                req,
                tx_arg,
                Some(&ep.provider),
                None,
                llm_concurrency_limit,
            )
            .await
            {
                Ok(response) => {
                    let latency = start.elapsed().as_millis() as u64;
                    brain.report(carrier_types::brain::EndpointReport {
                        endpoint_id: ep.id.clone(),
                        success: true,
                        latency_ms: latency,
                        error: None,
                    });
                    return Ok(response);
                }
                Err(e) => {
                    let latency = start.elapsed().as_millis() as u64;
                    let err_str = format!("{e}");
                    brain.report(carrier_types::brain::EndpointReport {
                        endpoint_id: ep.id.clone(),
                        success: false,
                        latency_ms: latency,
                        error: Some(err_str),
                    });
                    tracing::warn!(
                        endpoint = %ep.id,
                        error = %e,
                        "Endpoint failed in fallback chain, trying next"
                    );
                    last_error = Some(e);
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        CarrierError::LlmDriver(format!("All endpoints exhausted for modality '{modality}'"))
    }))
}

// ---------------------------------------------------------------------------
// Turn trimming
// ---------------------------------------------------------------------------

/// Trim old messages from the session, keeping only the most recent N.
///
/// Messages are removed from the front of the list (oldest first).
/// The caller is responsible for having already generated TurnSummaries
/// for the turns being removed.
pub(in crate::agent_loop) fn trim_oldest_turns(messages: &mut Vec<Message>, max_retained: usize) {
    if messages.len() <= max_retained {
        return;
    }
    // Drain from the front until we're at the threshold.
    // We drain in pairs (user + assistant) to keep whole turns.
    let excess = messages.len() - max_retained;
    // Round up to the nearest even number to preserve turn boundaries
    let drain_count = if excess.is_multiple_of(2) {
        excess
    } else {
        excess + 1
    };
    messages.drain(..drain_count.min(messages.len()));
}

// ---------------------------------------------------------------------------
// Turn summary
// ---------------------------------------------------------------------------

/// Generate a TurnSummary from the messages of a single conversation turn.
///
/// Extracts the user's intent and the assistant's outcome, then uses a
/// fast LLM call to produce a concise 1-2 sentence summary.
pub(in crate::agent_loop) async fn generate_turn_summary(
    turn_msgs: &[Message],
    brain: &Arc<dyn Brain>,
) -> Option<TurnSummary> {
    // Helper to extract text from a message
    fn extract_text(msg: &Message) -> String {
        match &msg.content {
            MessageContent::Text(t) => t.clone(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    // Extract user message (first in the slice)
    let user_text = turn_msgs
        .iter()
        .find(|m| m.role == Role::User)
        .map(extract_text)
        .unwrap_or_default();

    // Extract assistant response (last assistant message)
    let assistant_text = turn_msgs
        .iter()
        .rfind(|m| m.role == Role::Assistant)
        .map(extract_text)
        .unwrap_or_default();

    // Collect tool names used this turn
    let mut tools_used = Vec::new();
    for msg in turn_msgs {
        if let MessageContent::Blocks(blocks) = &msg.content {
            for block in blocks {
                if let ContentBlock::ToolUse { name, .. } = block {
                    if !tools_used.contains(name) {
                        tools_used.push(name.clone());
                    }
                }
            }
        }
    }

    let prompt = format!(
        "Summarize this conversation turn. Extract: user intent, outcome, and key facts.\n\n\
         User: {}\nAssistant: {}\n\n\
         Respond in this exact format:\n\
         INTENT: <what user wanted>\n\
         OUTCOME: <what was accomplished>\n\
         FACTS: <comma-separated key facts, or NONE>\n\n\
         Key facts include: user preferences, personal info (phone, email, accounts), \
         entity names (projects, organizations), decisions made, or events mentioned. \
         Omit procedural details and tool mechanics.",
        user_text, assistant_text
    );

    let request = CompletionRequest {
        model: String::new(),
        messages: vec![Message {
            role: Role::User,
            content: MessageContent::Text(prompt),
        }],
        tools: Vec::new(),
        max_tokens: SUMMARY_MAX_TOKENS,
        temperature: 0.3,
        system: Some("You are a conversation summarizer. Be concise and precise. Always follow the requested format.".to_string()),
        thinking: None,
        extra: Default::default(),
    };

    match brain.complete(SUMMARY_MODALITY, request).await {
        Ok(response) => {
            let text = response.text().trim().to_string();
            if text.is_empty() {
                return None;
            }
            // Parse structured output
            let (user_intent, assistant_outcome, key_facts) = parse_summary_output(&text);
            Some(TurnSummary {
                turn_number: 0, // filled in by caller
                timestamp: chrono::Utc::now().to_rfc3339(),
                user_intent,
                assistant_outcome,
                tools_used,
                key_facts,
            })
        }
        Err(e) => {
            warn!("Turn summary generation failed: {}", e);
            None
        }
    }
}

/// Parse the structured summary output from the LLM.
fn parse_summary_output(text: &str) -> (String, String, Vec<String>) {
    let mut intent = String::new();
    let mut outcome = String::new();
    let mut facts = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("INTENT:") {
            intent = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("OUTCOME:") {
            outcome = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("FACTS:") {
            let rest = rest.trim();
            if !rest.eq_ignore_ascii_case("NONE") && !rest.is_empty() {
                facts = rest
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }

    // Fallback if structured parsing didn't work
    if intent.is_empty() && outcome.is_empty() {
        let parts: Vec<&str> = text.split("→").collect();
        if parts.len() >= 2 {
            intent = parts[0].trim().to_string();
            outcome = parts[1].trim().to_string();
        } else {
            intent = text.to_string();
        }
    }

    (intent, outcome, facts)
}
