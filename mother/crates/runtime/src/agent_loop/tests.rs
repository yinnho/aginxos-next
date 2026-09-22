use super::state;
use super::*;
use crate::llm_driver::{CompletionResponse, LlmError};
use async_trait::async_trait;
use carrier_types::tool::ToolCall;
use std::sync::atomic::{AtomicU32, Ordering};

#[test]
fn test_context_pressure_from_usage_pct() {
    assert_eq!(
        state::ContextPressure::from_usage_pct(0.2),
        state::ContextPressure::Normal
    );
    assert_eq!(
        state::ContextPressure::from_usage_pct(0.55),
        state::ContextPressure::Elevated
    );
    assert_eq!(
        state::ContextPressure::from_usage_pct(0.75),
        state::ContextPressure::High
    );
    assert_eq!(
        state::ContextPressure::from_usage_pct(0.9),
        state::ContextPressure::Critical
    );
}

#[test]
fn test_no_progress_threshold_constant() {
    assert_eq!(NO_PROGRESS_THRESHOLD, 3);
}

#[test]
fn test_outcome_from_loop_err_stuck_no_progress() {
    let e = carrier_types::error::CarrierError::LoopStuck(
        "agent 连续 3 轮无进展（无工具调用、无最终答案），判定卡死，终止本轮".into(),
    );
    match state::outcome_from_loop_err(&e) {
        state::RunOutcome::Stuck(s) => assert!(s.contains("无进展")),
        other => panic!("expected Stuck, got {other:?}"),
    }
}

#[test]
fn test_outcome_from_loop_err_stuck_tool_loop() {
    let e = carrier_types::error::CarrierError::LoopStuck(
        "agent stuck in a tool loop on `file_read` after 3 corrective nudges".into(),
    );
    assert!(matches!(
        state::outcome_from_loop_err(&e),
        state::RunOutcome::Stuck(_)
    ));
}

#[test]
fn test_outcome_from_loop_err_internal_with_stuck_wording_is_error() {
    // Structural, not textual: an Internal error that merely *sounds*
    // stuck must classify as Error — only LoopStuck is Stuck.
    let e = carrier_types::error::CarrierError::Internal("判定卡死 wording".into());
    assert!(matches!(
        state::outcome_from_loop_err(&e),
        state::RunOutcome::Error(_)
    ));
}

#[test]
fn test_outcome_from_loop_err_other_is_error() {
    let e = carrier_types::error::CarrierError::Network("llm timeout".into());
    match state::outcome_from_loop_err(&e) {
        state::RunOutcome::Error(s) => assert!(s.contains("timeout")),
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn test_last_run_stuck_prompt_warns_next_turn() {
    let last = state::LastRunSummary {
        timestamp: "t".into(),
        iterations: 4,
        stop_reason: "stuck: idle".into(),
        tokens_used: 10,
        outcome: state::RunOutcome::Stuck("连续 3 轮无进展".into()),
    };
    let line = last.prompt_line();
    assert!(line.contains("卡死"), "{line}");
    assert!(line.contains("不要重复"), "{line}");
}

#[test]
fn test_last_run_stuck_prompt_strips_operator_advice() {
    // stuck reason 尾部带运维向建议（"Fix the flow/tool guidance and retry"），
    // 整段注入会把下一轮模型带去"修 guidance"而不是干活（08-21 86bus 自噬循环）。
    // prompt_line 只保留第一句。
    let last = state::LastRunSummary {
        timestamp: "t".into(),
        iterations: 12,
        stop_reason: "stuck: rotating".into(),
        tokens_used: 10,
        outcome: state::RunOutcome::Stuck(
            "agent re-called `file_read` with identical args 8x total this turn. \
             This is rotating repetition. Turn aborted to save the iteration budget. \
             Fix the flow/tool guidance and retry."
                .into(),
        ),
    };
    let line = last.prompt_line();
    assert!(line.contains("identical args 8x"), "{line}");
    assert!(!line.contains("Fix the flow"), "{line}");
    assert!(!line.contains("iteration budget"), "{line}");
}

#[test]
fn test_record_progress_aborts_after_threshold_idle_iters() {
    // No progress for 3 consecutive iterations -> Some(streak) on the 3rd.
    let mut state = LoopState::new(128_000);
    assert_eq!(
        state.record_iteration_progress(false),
        None,
        "1st idle: under threshold"
    );
    assert_eq!(
        state.record_iteration_progress(false),
        None,
        "2nd idle: under threshold"
    );
    let stuck = state.record_iteration_progress(false);
    assert_eq!(stuck, Some(3), "3rd idle: threshold reached");
}

#[test]
fn test_record_progress_resets_on_tool_or_completion() {
    let mut state = LoopState::new(128_000);
    // Two idle, then progress (tool call) resets the streak.
    assert_eq!(state.record_iteration_progress(false), None);
    assert_eq!(state.record_iteration_progress(false), None);
    assert_eq!(
        state.record_iteration_progress(true),
        None,
        "progress resets streak"
    );
    // Streak restarted - needs 3 more idle to trip.
    assert_eq!(state.record_iteration_progress(false), None);
    assert_eq!(state.record_iteration_progress(false), None);
    assert_eq!(state.record_iteration_progress(false), Some(3));
}

#[test]
fn test_record_progress_progress_never_aborts() {
    let mut state = LoopState::new(128_000);
    for _ in 0..100 {
        assert_eq!(state.record_iteration_progress(true), None);
    }
}

#[test]
fn declared_max_iterations_soft_at_n_hard_at_n_plus_2() {
    let mut state = LoopState::new(128_000);
    state.max_iterations = Some(3);
    state.iteration = 2; // current turn = 3
    let msg = state.build_status_message();
    assert!(msg.contains("预算 3 轮"), "{msg}");
    assert!(!state.declared_max_exceeded());
    state.iteration = 4; // N+1 completed
    assert!(!state.declared_max_exceeded());
    state.iteration = 5; // N+2
    assert!(state.declared_max_exceeded());
}

#[test]
fn undeclared_max_iterations_never_exceeded() {
    let mut state = LoopState::new(128_000);
    state.iteration = 10_000;
    assert!(!state.declared_max_exceeded());
    assert!(!state.build_status_message().contains("预算"));
}

#[test]
fn test_retry_constants() {
    assert_eq!(MAX_RETRIES, 3);
    assert_eq!(BASE_RETRY_DELAY_MS, 1000);
}

#[test]
fn test_dynamic_truncate_short_unchanged() {
    use crate::context_budget::{truncate_tool_result_dynamic, ContextBudget};
    let budget = ContextBudget::new(200_000);
    let short = "Hello, world!";
    assert_eq!(truncate_tool_result_dynamic(short, &budget), short);
}

#[test]
fn test_dynamic_truncate_over_limit() {
    use crate::context_budget::{truncate_tool_result_dynamic, ContextBudget};
    let budget = ContextBudget::new(200_000);
    let long = "x".repeat(budget.per_result_cap() + 10_000);
    let result = truncate_tool_result_dynamic(&long, &budget);
    assert!(result.len() <= budget.per_result_cap() + 200);
    assert!(result.contains("[TRUNCATED:"));
}

#[test]
fn test_dynamic_truncate_newline_boundary() {
    use crate::context_budget::{truncate_tool_result_dynamic, ContextBudget};
    // Small budget to force truncation
    let budget = ContextBudget::new(1_000);
    let content = (0..200)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let result = truncate_tool_result_dynamic(&content, &budget);
    // Should break at a newline, not mid-line
    let before_marker = result.split("[TRUNCATED:").next().unwrap();
    let trimmed = before_marker.trim_end();
    assert!(!trimmed.is_empty());
}

#[test]
fn test_max_continuations_constant() {
    assert_eq!(MAX_CONTINUATIONS, 5);
}

#[test]
fn test_tool_timeout_constants() {
    assert_eq!(TOOL_TIMEOUT_SECS, 120);
    assert_eq!(TOOL_TIMEOUT_LONG_SECS, 300);
    assert!(TOOL_LONG_TIMEOUT_NAMES.contains(&"image_generate"));
}

#[test]
fn test_max_history_messages() {
    assert_eq!(MAX_HISTORY_MESSAGES, 30);
}

// --- Loop detection ---

fn make_call(name: &str, input: serde_json::Value) -> (String, u64) {
    tool_call_key(name, &input)
}

#[test]
fn test_input_hash_ignores_object_key_order() {
    // Loop identity must not depend on the order the model emitted keys
    // (serde_json default BTreeMap gives canonical sorted-key
    // serialization). Parsed from raw strings so a future
    // `preserve_order` feature flip would fail this test.
    let a: serde_json::Value = serde_json::from_str(r#"{"q":"rust","limit":10}"#).unwrap();
    let b: serde_json::Value = serde_json::from_str(r#"{"limit":10,"q":"rust"}"#).unwrap();
    assert_eq!(tool_input_hash(&a), tool_input_hash(&b));
    // Nested objects too.
    let c: serde_json::Value = serde_json::from_str(r#"{"filter":{"b":2,"a":1},"q":"x"}"#).unwrap();
    let d: serde_json::Value = serde_json::from_str(r#"{"q":"x","filter":{"a":1,"b":2}}"#).unwrap();
    assert_eq!(tool_input_hash(&c), tool_input_hash(&d));
}

#[test]
fn test_tool_call_key_normalizes_name() {
    // Free-text recovery artifacts like `web_search,` must count as the
    // SAME call as a clean `web_search` — otherwise the repetition
    // counters reset on every dirty name and miss the loop.
    let clean = tool_call_key("web_search", &serde_json::json!({"q": "rust"}));
    let dirty = tool_call_key("web_search,", &serde_json::json!({"q": "rust"}));
    assert_eq!(clean, dirty);
}

#[test]
fn test_loop_detection_blocks_consecutive_same_call() {
    let recent: Vec<(String, u64)> = (0..LOOP_DETECTION_WINDOW)
        .map(|_| make_call("test_query", serde_json::json!({"q": "rust"})))
        .collect();
    let result = detect_tool_loop(&recent, LOOP_DETECTION_WINDOW);
    assert!(
        result.is_some(),
        "Should detect loop with same call repeated"
    );
    assert_eq!(result.unwrap().0, "test_query");
}

#[test]
fn test_loop_detection_allows_pagination() {
    // Same tool name but different inputs (pagination) — not a loop
    let recent: Vec<(String, u64)> = (0..LOOP_DETECTION_WINDOW)
        .map(|i| {
            make_call(
                "test_query",
                serde_json::json!({"q": format!("rust page {}", i)}),
            )
        })
        .collect();
    let result = detect_tool_loop(&recent, LOOP_DETECTION_WINDOW);
    assert!(
        result.is_none(),
        "Pagination with different queries should not be flagged"
    );
}

#[test]
fn test_loop_detection_requires_full_window() {
    // 3 same calls is below threshold of 4
    let recent: Vec<(String, u64)> = (0..3)
        .map(|_| make_call("test_query", serde_json::json!({"q": "rust"})))
        .collect();
    let result = detect_tool_loop(&recent, LOOP_DETECTION_WINDOW);
    assert!(result.is_none(), "Below-threshold count should not trigger");
}

#[test]
fn test_loop_detection_breaks_on_different_tool() {
    // 3 test_query + 1 web_fetch + 3 test_query → last 4 are mixed (window is 4)
    let mut recent: Vec<(String, u64)> = (0..3)
        .map(|_| make_call("test_query", serde_json::json!({"q": "rust"})))
        .collect();
    recent.push(make_call(
        "web_fetch",
        serde_json::json!({"url": "https://example.com"}),
    ));
    recent.extend((0..3).map(|_| make_call("test_query", serde_json::json!({"q": "rust"}))));
    let result = detect_tool_loop(&recent, LOOP_DETECTION_WINDOW);
    assert!(
        result.is_none(),
        "Mixed tail should not trigger loop detection"
    );
}

#[test]
fn test_loop_detection_window_constant() {
    assert_eq!(LOOP_DETECTION_WINDOW, 4);
}

// --- Cumulative (rotating) repetition ---

#[test]
fn test_cumulative_threshold_constants() {
    // Progressive cadence (dsh repeat-tool-reminder): remind → escalate → break.
    assert_eq!(CUMULATIVE_REMIND_AT, 3);
    assert_eq!(CUMULATIVE_ESCALATE_AT, 5);
    assert_eq!(CUMULATIVE_BREAK_AT, 8);
}

#[test]
fn test_args_preview_truncates() {
    let long = serde_json::json!({"content": "x".repeat(300)});
    let preview = tool_args_preview(&long, 120);
    assert!(preview.chars().count() <= 121, "preview must be bounded");
    assert!(preview.ends_with('…'));
    let short = serde_json::json!({"path": "大纲.md"});
    assert_eq!(tool_args_preview(&short, 120), r#"{"path":"大纲.md"}"#);
}

/// Mirrors the in-loop counter in handle_tool_use: a per-(name, input_hash)
/// count accumulated across the WHOLE turn (interleaved calls included).
/// Rotating repetition (same call recurring but never 4-in-a-row) must still
/// trip the cumulative threshold — this is the exact gap the 16:29 ai-writer
/// turn fell through (file_read on 4 paths cycled, each read 3×, consecutive
/// window never filled, 600s burned).
fn cumulative_count(recent: &[(String, u64)]) -> std::collections::HashMap<(String, u64), u32> {
    let mut counts: std::collections::HashMap<(String, u64), u32> =
        std::collections::HashMap::new();
    for entry in recent {
        *counts.entry(entry.clone()).or_insert(0) += 1;
    }
    counts
}

#[test]
fn test_cumulative_repetition_triggers_on_rotating_pattern() {
    // file_read("大纲") interleaved with reads of other files — never 4
    // consecutive, but 3 total. Consecutive detector misses this.
    let mut recent: Vec<(String, u64)> = Vec::new();
    recent.push(make_call(
        "file_read",
        serde_json::json!({"path": "大纲.md"}),
    ));
    recent.push(make_call(
        "file_read",
        serde_json::json!({"path": "素材.md"}),
    ));
    recent.push(make_call(
        "file_read",
        serde_json::json!({"path": "大纲.md"}),
    ));
    recent.push(make_call(
        "file_read",
        serde_json::json!({"path": "风格.md"}),
    ));
    recent.push(make_call(
        "file_read",
        serde_json::json!({"path": "大纲.md"}),
    ));

    // Consecutive window (4) does NOT fire — tail is mixed.
    assert!(detect_tool_loop(&recent, LOOP_DETECTION_WINDOW).is_none());

    // Cumulative count of 大纲.md hits the reminder threshold (3).
    let counts = cumulative_count(&recent);
    let dagan = make_call("file_read", serde_json::json!({"path": "大纲.md"}));
    let dagan_count = *counts.get(&dagan).unwrap();
    assert_eq!(dagan_count, 3);
    assert!(dagan_count >= CUMULATIVE_REMIND_AT);
}

#[test]
fn test_cumulative_repetition_below_threshold_no_trigger() {
    // Only 2 identical calls — under the reminder threshold.
    let recent: Vec<(String, u64)> = vec![
        make_call("file_read", serde_json::json!({"path": "大纲.md"})),
        make_call("file_read", serde_json::json!({"path": "素材.md"})),
        make_call("file_read", serde_json::json!({"path": "大纲.md"})),
    ];
    let counts = cumulative_count(&recent);
    let dagan = make_call("file_read", serde_json::json!({"path": "大纲.md"}));
    let dagan_count = *counts.get(&dagan).unwrap();
    assert_eq!(dagan_count, 2);
    assert!(dagan_count < CUMULATIVE_REMIND_AT);
}

#[test]
fn test_cumulative_repetition_distinguishes_args() {
    // Same tool name, different args — pagination. Must NOT trip cumulative.
    let recent: Vec<(String, u64)> = vec![
        make_call("web_search", serde_json::json!({"q": "rust page 1"})),
        make_call("web_search", serde_json::json!({"q": "rust page 2"})),
        make_call("web_search", serde_json::json!({"q": "rust page 3"})),
    ];
    let counts = cumulative_count(&recent);
    // No single (name, hash) pair reaches the reminder threshold.
    assert!(counts.values().all(|c| *c < CUMULATIVE_REMIND_AT));
}

// --- Integration tests for empty response guards ---

fn test_manifest() -> AgentManifest {
    AgentManifest {
        name: "test-agent".to_string(),
        model: carrier_types::agent::ModelConfig {
            system_prompt: "You are a test agent.".to_string(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Mock driver that simulates: first call returns ToolUse with no text,
/// second call returns EndTurn with empty text. This reproduces the bug
/// where the LLM ends with no text after a tool-use cycle.
struct EmptyAfterToolUseDriver {
    call_count: AtomicU32,
}

impl EmptyAfterToolUseDriver {
    fn new() -> Self {
        Self {
            call_count: AtomicU32::new(0),
        }
    }
}

#[async_trait]
impl LlmDriver for EmptyAfterToolUseDriver {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let call = self.call_count.fetch_add(1, Ordering::Relaxed);
        if call == 0 {
            // First call: LLM wants to use a tool (with no text block)
            Ok(CompletionResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tool_1".to_string(),
                    name: "fake_tool".to_string(),
                    input: serde_json::json!({"query": "test"}),
                    provider_metadata: None,
                }],
                stop_reason: StopReason::ToolUse,
                tool_calls: vec![ToolCall {
                    id: "tool_1".to_string(),
                    name: "fake_tool".to_string(),
                    input: serde_json::json!({"query": "test"}),
                }],
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
                media: None,
            })
        } else {
            // Second call: LLM returns EndTurn with EMPTY text (the bug)
            Ok(CompletionResponse {
                content: vec![],
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 0,
                },
                media: None,
            })
        }
    }
}

/// Mock driver that returns empty text with MaxTokens stop reason,
/// repeated MAX_CONTINUATIONS times to trigger the max continuations path.
struct EmptyMaxTokensDriver;

/// Returns a ToolUse for a DIFFERENT non-existent tool each call
/// (fake_tool_0, fake_tool_1, ...). Different names/args keep the cumulative
/// loop detector from firing, but every call errors at execution (tool not
/// registered). Tests that the no-progress detector kills the turn after 3
/// consecutive all-failed iterations (Problem 3: failed tools = no progress).
struct AllFailingToolsDriver {
    call_count: AtomicU32,
}

impl AllFailingToolsDriver {
    fn new() -> Self {
        Self {
            call_count: AtomicU32::new(0),
        }
    }
}

#[async_trait]
impl LlmDriver for AllFailingToolsDriver {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let n = self.call_count.fetch_add(1, Ordering::Relaxed);
        let name = format!("fake_tool_{n}");
        let input = serde_json::json!({"q": format!("arg-{n}")});
        Ok(CompletionResponse {
            content: vec![ContentBlock::ToolUse {
                id: format!("t{n}"),
                name: name.clone(),
                input: input.clone(),
                provider_metadata: None,
            }],
            stop_reason: StopReason::ToolUse,
            tool_calls: vec![ToolCall {
                id: format!("t{n}"),
                name,
                input,
            }],
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
            },
            media: None,
        })
    }
}

#[async_trait]
impl LlmDriver for EmptyMaxTokensDriver {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(CompletionResponse {
            content: vec![],
            stop_reason: StopReason::MaxTokens,
            tool_calls: vec![],
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 0,
            },
            media: None,
        })
    }
}

/// Mock driver that returns normal text (sanity check).
struct NormalDriver;

#[async_trait]
impl LlmDriver for NormalDriver {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(CompletionResponse {
            content: vec![ContentBlock::Text {
                text: "Hello from the agent!".to_string(),
                provider_metadata: None,
            }],
            stop_reason: StopReason::EndTurn,
            tool_calls: vec![],
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 8,
            },
            media: None,
        })
    }
}

#[tokio::test]
async fn test_empty_response_after_tool_use_returns_fallback() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(EmptyAfterToolUseDriver::new());

    let result = run_agent_loop(
        &manifest,
        "Do something with tools",
        &mut session,
        &memory,
        driver,
        &[],  // no tools registered — the tool call will fail, which is fine
        None, // kernel
        None, // stream_tx
        None, // mcp_connections
        None, // workspace_root
        None, // on_phase
        None, // hooks
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Loop should complete without error");

    // The response MUST NOT be empty — it should contain our fallback text
    assert!(
        !result.response.trim().is_empty(),
        "Response should not be empty after tool use, got: {:?}",
        result.response
    );
    assert!(
        result.response.contains("已执行操作"),
        "Expected fallback message, got: {:?}",
        result.response
    );
}

#[tokio::test]
async fn test_tool_error_injects_no_fabrication_guidance() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(EmptyAfterToolUseDriver::new());

    run_agent_loop(
        &manifest,
        "Do something with tools",
        &mut session,
        &memory,
        driver,
        &[],  // no tools registered — the tool call will fail, which is fine
        None, // kernel
        None, // stream_tx
        None, // mcp_connections
        None, // workspace_root
        None, // on_phase
        None, // hooks
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Loop should complete without error");

    let guidance_seen = session.messages.iter().any(|msg| {
        match &msg.content {
            MessageContent::Blocks(blocks) => blocks.iter().any(|block| {
                matches!(block, ContentBlock::Text { text, .. } if text.contains("工具错误分析"))
            }),
            _ => false,
        }
    });

    assert!(
        guidance_seen,
        "Expected tool error guidance in session messages after failed tool call"
    );
}

#[tokio::test]
async fn test_empty_response_max_tokens_returns_fallback() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(EmptyMaxTokensDriver);

    let result = run_agent_loop(
        &manifest,
        "Tell me something long",
        &mut session,
        &memory,
        driver,
        &[],
        None,
        None, // stream_tx
        None,
        None,
        None, // on_phase
        None, // hooks
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Loop should complete without error");

    // Should hit MAX_CONTINUATIONS and return fallback instead of empty
    assert!(
        !result.response.trim().is_empty(),
        "Response should not be empty on max tokens, got: {:?}",
        result.response
    );
    assert!(
        result.response.contains("token limit"),
        "Expected max-tokens fallback message, got: {:?}",
        result.response
    );
}

#[tokio::test]
async fn test_normal_response_not_replaced_by_fallback() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(NormalDriver);

    let result = run_agent_loop(
        &manifest,
        "Say hello",
        &mut session,
        &memory,
        driver,
        &[],
        None,
        None, // stream_tx
        None,
        None,
        None, // on_phase
        None, // hooks
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Loop should complete without error");

    // Normal response should pass through unchanged
    assert_eq!(result.response, "Hello from the agent!");
}

#[tokio::test]
async fn test_no_progress_kills_after_all_failing_tools() {
    // Every iteration calls a different non-existent tool (so cumulative
    // loop detection doesn't fire) that errors at execution. After 3
    // consecutive all-failed iterations the no-progress detector must kill
    // the turn (Problem 3: failed tools = no progress).
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(AllFailingToolsDriver::new());

    let result = run_agent_loop(
        &manifest,
        "do stuff",
        &mut session,
        &memory,
        driver,
        &[],
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await;

    let err = result.expect_err("should be killed by no-progress detector");
    let msg = err.to_string();
    assert!(
        msg.contains("无进展") || msg.contains("卡死"),
        "expected no-progress kill, got: {msg}"
    );
}

#[tokio::test]
async fn test_streaming_empty_response_after_tool_use_returns_fallback() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(EmptyAfterToolUseDriver::new());
    let (tx, _rx) = mpsc::channel(64);

    let result = run_agent_loop_streaming(
        &manifest,
        "Do something with tools",
        &mut session,
        &memory,
        driver,
        &[],
        None,
        tx,
        None,
        None,
        None, // on_phase
        None, // hooks
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Streaming loop should complete without error");

    assert!(
        !result.response.trim().is_empty(),
        "Streaming response should not be empty after tool use, got: {:?}",
        result.response
    );
    assert!(
        result.response.contains("已执行操作"),
        "Expected fallback message in streaming, got: {:?}",
        result.response
    );
}

/// Mock driver that returns empty text on first call (EndTurn), then normal text on second.
/// This tests the one-shot retry logic for iteration 0 empty responses.
struct EmptyThenNormalDriver {
    call_count: AtomicU32,
}

impl EmptyThenNormalDriver {
    fn new() -> Self {
        Self {
            call_count: AtomicU32::new(0),
        }
    }
}

#[async_trait]
impl LlmDriver for EmptyThenNormalDriver {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        let call = self.call_count.fetch_add(1, Ordering::Relaxed);
        if call == 0 {
            // First call: empty EndTurn (triggers retry)
            Ok(CompletionResponse {
                content: vec![],
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 0,
                },
                media: None,
            })
        } else {
            // Second call (retry): normal response
            Ok(CompletionResponse {
                content: vec![ContentBlock::Text {
                    text: "Recovered after retry!".to_string(),
                    provider_metadata: None,
                }],
                stop_reason: StopReason::EndTurn,
                tool_calls: vec![],
                usage: TokenUsage {
                    input_tokens: 15,
                    output_tokens: 8,
                },
                media: None,
            })
        }
    }
}

/// Mock driver that always returns empty EndTurn (no recovery on retry).
/// Tests that the fallback message appears when retry also fails.
struct AlwaysEmptyDriver;

#[async_trait]
impl LlmDriver for AlwaysEmptyDriver {
    async fn complete(&self, _request: CompletionRequest) -> Result<CompletionResponse, LlmError> {
        Ok(CompletionResponse {
            content: vec![],
            stop_reason: StopReason::EndTurn,
            tool_calls: vec![],
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 0,
            },
            media: None,
        })
    }
}

#[tokio::test]
async fn test_empty_first_response_retries_and_recovers() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(EmptyThenNormalDriver::new());

    let result = run_agent_loop(
        &manifest,
        "Hello",
        &mut session,
        &memory,
        driver,
        &[],
        None,
        None, // stream_tx
        None,
        None,
        None,
        None,
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Loop should recover via retry");

    assert_eq!(result.response, "Recovered after retry!");
    assert_eq!(
        result.iterations, 2,
        "Should have taken 2 iterations (retry)"
    );
}

#[tokio::test]
async fn test_empty_first_response_fallback_when_retry_also_empty() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(AlwaysEmptyDriver);

    let result = run_agent_loop(
        &manifest,
        "Hello",
        &mut session,
        &memory,
        driver,
        &[],
        None,
        None, // stream_tx
        None,
        None,
        None,
        None,
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Loop should complete with fallback");

    // No tools were executed, so should get the empty response message
    assert!(
        result.response.contains("没有返回内容"),
        "Expected empty response fallback (no tools executed), got: {:?}",
        result.response
    );
}

#[tokio::test]
async fn test_max_history_messages_constant() {
    assert_eq!(MAX_HISTORY_MESSAGES, 30);
}

#[tokio::test]
async fn test_streaming_empty_response_max_tokens_returns_fallback() {
    let memory = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let _agent_id = "test-agent".to_string();
    let mut session = carrier_memory::session::Session {
        id: carrier_types::agent::SessionId::new(),
        agent_name: "test-agent".to_string(),
        messages: Vec::new(),
        context_window_tokens: 0,
        turn_summaries: Vec::new(),
        label: None,
    };
    let manifest = test_manifest();
    let driver: Arc<dyn LlmDriver> = Arc::new(EmptyMaxTokensDriver);
    let (tx, _rx) = mpsc::channel(64);

    let result = run_agent_loop_streaming(
        &manifest,
        "Tell me something long",
        &mut session,
        &memory,
        driver,
        &[],
        None,
        tx,
        None,
        None,
        None, // on_phase
        None, // hooks
        None, // context_window_tokens
        None, // process_manager
        None, // user_content_blocks
        None, // brain
        None, // memory_handle
        None, // sender_id
        None, // owner_id
        None, // channel_type
        None, // llm_concurrency_limit
    )
    .await
    .expect("Streaming loop should complete without error");

    assert!(
        !result.response.trim().is_empty(),
        "Streaming response should not be empty on max tokens, got: {:?}",
        result.response
    );
    assert!(
        result.response.contains("token limit"),
        "Expected max-tokens fallback in streaming, got: {:?}",
        result.response
    );
}
