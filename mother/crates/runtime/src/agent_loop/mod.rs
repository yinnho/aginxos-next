//! Core agent execution loop.
//!
//! The agent loop handles receiving a user message, recalling relevant memories,
//! calling the LLM, executing tool calls, and saving the conversation.
//!
//! The implementation is split across modules:
//! - `context` — LoopContext: bundles all mutable state and references
//! - `state`   — LoopState: runtime loop counters, budget, pressure, error tracking
//! - `helpers` — retry logic, fallback chain, loop detection, turn trimming/summary
//! - `end_turn` — handler for EndTurn / StopSequence
//! - `tool_use` — handler for ToolUse (tool execution, error tracking, discovery)
//! - `max_tokens` — handler for MaxTokens (continuation / partial response)
//!
//! ## Phase structure (O7 state machine)
//!
//! ```text
//! INIT → [PREPARE_TURN → LLM_CALL → DISPATCH → NEXT_TURN]* → TEARDOWN
//! ```

mod context;
mod end_turn;
mod helpers;
mod knowledge;
mod max_tokens;
mod state;
mod tool_use;

use crate::agent_loop::context::LoopContext;
use crate::agent_loop::state::LoopState;
use crate::context_budget::{apply_context_guard, ContextBudget};
use crate::context_overflow::{recover_from_overflow, RecoveryStage};
use crate::kernel_handle::KernelHandle;
use crate::llm_driver::{Brain, CompletionRequest, CompletionResponse, LlmDriver, StreamEvent};

use crate::mcp::McpConnection;
use crate::text_tool_recovery::detect_text_tool_mentions;
use carrier_memory::session::Session;
use carrier_memory::MemorySubstrate;
use carrier_types::agent::AgentManifest;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::message::{ContentBlock, Message, Role, StopReason, TokenUsage};
// Re-export for tests (via `use super::*`)
#[allow(unused_imports)]
pub(crate) use carrier_types::message::MessageContent;
use carrier_types::tool::ToolDefinition;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

// Re-export constants that external modules (tests) reference.
pub use helpers::TOOL_LONG_TIMEOUT_NAMES;
pub use helpers::TOOL_TIMEOUT_LONG_SECS;
pub use helpers::TOOL_TIMEOUT_SECS;
pub use max_tokens::MAX_CONTINUATIONS;
// Re-export constants and functions used by tests via `use super::*`.
pub use helpers::{
    detect_soft_loop, detect_tool_loop, tool_args_preview, tool_call_key, tool_input_hash,
};
pub use helpers::{
    BASE_RETRY_DELAY_MS, CUMULATIVE_BREAK_AT, CUMULATIVE_ESCALATE_AT, CUMULATIVE_REMIND_AT,
    LOOP_DETECTION_WINDOW, MAX_HISTORY_MESSAGES, MAX_RETRIES, SOFT_LOOP_WINDOW,
};
// Re-export the kv-drawer knowledge merge so the kernel's compaction path can
// flush facts with the same idempotent semantics as the per-turn path.
pub use knowledge::merge_key_facts;

/// Consecutive no-progress iterations (no tool call, no final answer, not
/// actively generating via MaxTokens) after which the turn is aborted as stuck.
/// Aligns with `CUMULATIVE_REMIND_AT` - 3 idle turns is clearly spinning.
const NO_PROGRESS_THRESHOLD: u32 = 3;

const MAX_TEXT_RECOVERY_RETRIES: u32 = 2;

/// Agent lifecycle phase within the execution loop.
/// Used for UX indicators (typing, reactions) without coupling to channel types.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopPhase {
    /// Agent is calling the LLM.
    Thinking,
    /// Agent is executing a tool.
    ToolUse { tool_name: String },
    /// Agent is streaming tokens.
    Streaming,
    /// Agent finished successfully.
    Done,
    /// Agent encountered an error.
    Error,
}

/// Callback for agent lifecycle phase changes.
/// Implementations should be non-blocking (fire-and-forget) to avoid slowing the loop.
pub type PhaseCallback = Arc<dyn Fn(LoopPhase) + Send + Sync>;

/// A step within a task plan produced by the `task_plan` tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskStep {
    pub id: String,
    pub prompt: String,
    pub depends_on: Vec<String>,
    /// Optional named flow to load for this step (e.g. "article-writer").
    /// When set, `execute_plan` injects the flow body + declared tools +
    /// max_iterations/elevation for this step — exactly like an explicit
    /// `active_flow` turn. Without it the step runs bare (no flow guidance,
    /// full agent toolset), which is why plan steps used to ignore the flow's
    /// hard rules and flounder (loop on file_read, reach for document_generate).
    #[serde(default)]
    pub flow: Option<String>,
}

/// A task plan produced by the `task_plan` tool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskPlan {
    pub title: String,
    pub steps: Vec<TaskStep>,
}

/// Result of an agent loop execution.
#[derive(Debug)]
pub struct AgentLoopResult {
    /// The final text response from the agent.
    pub response: String,
    /// Total token usage across all LLM calls.
    pub total_usage: TokenUsage,
    /// Number of iterations the loop ran.
    pub iterations: u32,
    /// True when the agent intentionally chose not to reply.
    ///
    /// Set by the agent loop (and multi-step flows) when the turn is silent:
    /// `[[silent]]`, whole-text no-reply sentinels (`NO_REPLY`,
    /// `[no reply needed]`, … — see `outbound::is_no_reply_sentinel`), etc.
    /// When true, `response` is empty for channel delivery; session history may
    /// still store a stable `[no reply needed]` marker. Outbound sinks keep a
    /// text-sentinel safety net for non-loop producers.
    pub silent: bool,
    /// Reply directives extracted from the agent's response.
    pub directives: carrier_types::message::ReplyDirectives,
    /// Task plan produced by the task_plan tool, if any.
    pub plan: Option<TaskPlan>,
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Run the agent execution loop for a single user message.
///
/// This is the core of Carrier: it loads session context, recalls memories,
/// runs the LLM in a tool-use loop, and saves the updated session.
///
/// Pass `stream_tx = Some(tx)` to receive incremental `StreamEvent`s during
/// execution; pass `None` for a non-streaming (blocking) call.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_loop(
    manifest: &AgentManifest,
    user_message: &str,
    session: &mut Session,
    memory: &MemorySubstrate,
    driver: Arc<dyn LlmDriver>,
    tools: &[ToolDefinition],
    kernel: Option<Arc<dyn KernelHandle>>,
    stream_tx: Option<mpsc::Sender<StreamEvent>>,
    mcp_connections: Option<&dashmap::DashMap<String, McpConnection>>,
    workspace_root: Option<&Path>,
    on_phase: Option<&PhaseCallback>,
    hooks: Option<&crate::hooks::HookRegistry>,
    context_window_tokens: Option<usize>,
    process_manager: Option<&crate::process_manager::ProcessManager>,
    user_content_blocks: Option<Vec<ContentBlock>>,
    brain: Option<Arc<dyn Brain>>,
    memory_handle: Option<Arc<dyn crate::memory_handle::MemoryHandle>>,
    sender_id: Option<&str>,
    owner_id: Option<&str>,
    channel_type: Option<&str>,
    llm_concurrency_limit: Option<Arc<tokio::sync::Semaphore>>,
) -> CarrierResult<AgentLoopResult> {
    // No inner wall-clock deadline: the loop runs until natural completion or
    // stuck detection (tool-call repetition via `BreakToolLoop`, or no-progress
    // idle via `NO_PROGRESS_THRESHOLD`). The outer `bounded_turn` / cron timeout
    // (config `agent_turn_timeout_secs`, default 4h) is a daemon-hang backstop
    // only - it must never be the thing that kills legitimate long work.
    run_agent_loop_impl(
        manifest,
        user_message,
        session,
        memory,
        driver,
        tools,
        kernel,
        stream_tx,
        mcp_connections,
        workspace_root,
        on_phase,
        hooks,
        context_window_tokens,
        process_manager,
        user_content_blocks,
        brain,
        memory_handle,
        sender_id,
        owner_id,
        channel_type,
        llm_concurrency_limit,
    )
    .await
}

/// Streaming variant of [`run_agent_loop`].
///
/// Equivalent to calling `run_agent_loop` with `stream_tx = Some(tx)`.
/// Kept as a convenience wrapper for existing call sites.
#[allow(clippy::too_many_arguments)]
pub async fn run_agent_loop_streaming(
    manifest: &AgentManifest,
    user_message: &str,
    session: &mut Session,
    memory: &MemorySubstrate,
    driver: Arc<dyn LlmDriver>,
    tools: &[ToolDefinition],
    kernel: Option<Arc<dyn KernelHandle>>,
    stream_tx: mpsc::Sender<StreamEvent>,
    mcp_connections: Option<&dashmap::DashMap<String, McpConnection>>,
    workspace_root: Option<&Path>,
    on_phase: Option<&PhaseCallback>,
    hooks: Option<&crate::hooks::HookRegistry>,
    context_window_tokens: Option<usize>,
    process_manager: Option<&crate::process_manager::ProcessManager>,
    user_content_blocks: Option<Vec<ContentBlock>>,
    brain: Option<Arc<dyn Brain>>,
    memory_handle: Option<Arc<dyn crate::memory_handle::MemoryHandle>>,
    sender_id: Option<&str>,
    owner_id: Option<&str>,
    channel_type: Option<&str>,
    llm_concurrency_limit: Option<Arc<tokio::sync::Semaphore>>,
) -> CarrierResult<AgentLoopResult> {
    run_agent_loop(
        manifest,
        user_message,
        session,
        memory,
        driver,
        tools,
        kernel,
        Some(stream_tx),
        mcp_connections,
        workspace_root,
        on_phase,
        hooks,
        context_window_tokens,
        process_manager,
        user_content_blocks,
        brain,
        memory_handle,
        sender_id,
        owner_id,
        channel_type,
        llm_concurrency_limit,
    )
    .await
}

// ---------------------------------------------------------------------------
// Phase: INIT — build context, load session, restore state
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_agent_loop_impl(
    manifest: &AgentManifest,
    user_message: &str,
    session: &mut Session,
    memory: &MemorySubstrate,
    driver: Arc<dyn LlmDriver>,
    tools: &[ToolDefinition],
    kernel: Option<Arc<dyn KernelHandle>>,
    stream_tx: Option<mpsc::Sender<StreamEvent>>,
    mcp_connections: Option<&dashmap::DashMap<String, McpConnection>>,
    workspace_root: Option<&Path>,
    on_phase: Option<&PhaseCallback>,
    hooks: Option<&crate::hooks::HookRegistry>,
    context_window_tokens: Option<usize>,
    process_manager: Option<&crate::process_manager::ProcessManager>,
    user_content_blocks: Option<Vec<ContentBlock>>,
    brain: Option<Arc<dyn Brain>>,
    memory_handle: Option<Arc<dyn crate::memory_handle::MemoryHandle>>,
    sender_id: Option<&str>,
    owner_id: Option<&str>,
    channel_type: Option<&str>,
    llm_concurrency_limit: Option<Arc<tokio::sync::Semaphore>>,
) -> CarrierResult<AgentLoopResult> {
    info!(agent = %manifest.name, "Starting agent loop");
    // P1-A observational bypass: turn envelope events (message-level surface
    // events are appended inside save_session_append_async).
    memory.session_events_append(
        &manifest.name,
        &session.id.0.to_string(),
        vec![carrier_memory::SessionEventKind::TurnStart],
    );

    let hand_allowed_env: Vec<String> = manifest
        .metadata
        .get("hand_allowed_env")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // Fire BeforePromptBuild hook
    let agent_id_str = session.agent_name.clone();
    if let Some(hook_reg) = hooks {
        let ctx = crate::hooks::HookContext {
            agent_name: &manifest.name,
            agent_id: agent_id_str.as_str(),
            event: carrier_types::agent::HookEvent::BeforePromptBuild,
            data: serde_json::json!({
                "system_prompt": &manifest.model.system_prompt,
                "user_message": user_message,
            }),
        };
        let _ = hook_reg.fire(&ctx);
    }

    let system_prompt = manifest.model.system_prompt.clone();
    let session_base_len = session.messages.len();

    // Add the user message to session history.
    if let Some(blocks) = user_content_blocks {
        session.messages.push(Message::user_with_blocks(blocks));
    } else {
        session.messages.push(Message::user(user_message));
    }

    let llm_messages: Vec<Message> = session
        .messages
        .iter()
        .filter(|m| m.role != Role::System)
        .cloned()
        .collect();

    let mut messages = crate::session_repair::validate_and_repair(&llm_messages);

    // Inject canonical context
    if let Some(cc_msg) = manifest
        .metadata
        .get("canonical_context_msg")
        .and_then(|v| v.as_str())
    {
        if !cc_msg.is_empty() {
            messages.insert(0, Message::user(cc_msg));
        }
    }

    // Safety valve: trim excessively long message histories
    if messages.len() > helpers::MAX_HISTORY_MESSAGES {
        let trim_count = messages.len() - helpers::MAX_HISTORY_MESSAGES;
        warn!(
            agent = %manifest.name,
            total_messages = messages.len(),
            trimming = trim_count,
            "Trimming old messages to prevent context overflow"
        );
        crate::context_overflow::pair_aware_drain(&mut messages, trim_count);
    }

    let ctx_window = context_window_tokens.unwrap_or(helpers::DEFAULT_CONTEXT_WINDOW);
    let context_budget = ContextBudget::new(ctx_window);

    let mut state = LoopState::new(ctx_window);
    state.max_iterations = manifest
        .metadata
        .get(carrier_types::flow::META_MAX_ITERATIONS_DECLARED)
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    // O8: Restore last run summary from cross-session state
    if let Some(mh) = &memory_handle {
        let agent_key = format!("loop_state:{}", manifest.name);
        if let Ok(Some(val)) = mh.kv_get(
            &manifest.name,
            owner_id.unwrap_or(""),
            sender_id.unwrap_or(""),
            &agent_key,
        ) {
            if let Ok(last_run) =
                serde_json::from_value::<crate::agent_loop::state::LastRunSummary>(val)
            {
                info!(
                    agent = %manifest.name,
                    last_iterations = last_run.iterations,
                    last_stop_reason = %last_run.stop_reason,
                    last_outcome = ?last_run.outcome,
                    "Restored last run summary from cross-session state"
                );
                state.last_run = Some(last_run);
            }
        }
    }

    // Inject last run context into messages if available
    if let Some(last) = &state.last_run {
        messages.push(Message::system(last.prompt_line()));
    }

    let mut ctx = LoopContext {
        manifest,
        user_message,
        agent_id_str,
        session,
        messages,
        session_base_len,
        memory,
        memory_handle,
        driver,
        brain,
        system_prompt,
        stream_tx,
        llm_concurrency_limit,
        tools_owned: tools.to_vec(),
        discovered_tool_names: std::collections::HashSet::new(),
        loaded_flows: std::collections::HashSet::new(),
        loaded_flow_shell_allow: Vec::new(),
        loaded_flow_elevated_tools: Vec::new(),
        kernel,
        mcp_connections,
        workspace_root,
        process_manager,
        context_budget,
        on_phase,
        hooks,
        sender_id,
        owner_id,
        channel_type,
        hand_allowed_env,
        context_window_tokens: ctx_window,
        state,
        detected_plan: None,
    };

    // ---- Main loop ----
    // No iteration cap: runs until natural completion (Complete), a detected
    // task plan (BreakForPlan), or stuck detection (returns Err). max_iterations
    // is advisory only.
    loop {
        let action = match loop_iteration(&mut ctx).await {
            Ok(a) => a,
            Err(e) => {
                // Stuck / tool-loop / LLM failure: persist so the next turn
                // sees "上次卡死" instead of a clean slate.
                let outcome = state::outcome_from_loop_err(&e);
                ctx.log_event(ctx.turn_end_event(&match &outcome {
                    state::RunOutcome::Stuck(r) => format!("stuck: {r}"),
                    state::RunOutcome::Error(r) => format!("error: {r}"),
                    _ => "error".to_string(),
                }));
                ctx.persist_last_run(outcome);
                return Err(e);
            }
        };
        match action {
            LoopAction::Continue => {}
            LoopAction::Complete(result) => {
                ctx.log_event(ctx.turn_end_event(if result.silent {
                    "silent"
                } else {
                    "complete"
                }));
                return Ok(result);
            }
            LoopAction::BreakForPlan => break,
        }
    }

    // ---- TEARDOWN ---- (reached only via BreakForPlan)
    ctx.log_event(ctx.turn_end_event("task_plan"));
    teardown(&mut ctx).await
}

// ---------------------------------------------------------------------------
// Loop iteration result
// ---------------------------------------------------------------------------

/// What the main loop should do after a single iteration.
enum LoopAction {
    /// Continue to the next iteration.
    Continue,
    /// Return this result to the caller (loop finished successfully).
    Complete(AgentLoopResult),
    /// Break out of the loop because a task_plan was detected.
    BreakForPlan,
}

// ---------------------------------------------------------------------------
// Single iteration: PREPARE_TURN → LLM_CALL → DISPATCH
// ---------------------------------------------------------------------------

async fn loop_iteration(ctx: &mut LoopContext<'_>) -> CarrierResult<LoopAction> {
    let iteration = ctx.state.iteration;
    debug!(iteration, "Streaming agent loop iteration");

    // Reset per-iteration tool counter for the no-progress detector.
    ctx.state.tools_this_iter = 0;

    // ---- PREPARE_TURN ----
    prepare_turn(ctx);

    // ---- LLM_CALL ----
    let modality = select_modality(ctx);
    let response = call_llm(ctx, &modality).await?;

    ctx.state.total_usage.input_tokens += response.usage.input_tokens;
    ctx.state.total_usage.output_tokens += response.usage.output_tokens;
    ctx.state.context_tokens_used_estimate = response.usage.input_tokens as usize;
    ctx.state.context_pressure =
        state::ContextPressure::from_usage_pct(ctx.state.context_usage_pct());

    // ---- Text tool call recovery ----
    // Capture stop_reason before `response` is moved/consumed; it drives the
    // no-progress check below. (StopReason is Copy.)
    let stop_reason = response.stop_reason;
    let response = match handle_text_recovery(ctx, response, &modality).await {
        TextRecoveryOutcome::Continue => {
            // Active recovery attempt (model narrated tools as text) - counts as
            // progress, so reset the idle streak.
            ctx.state.idle_streak = 0;
            ctx.state.iteration += 1;
            return Ok(LoopAction::Continue);
        }
        TextRecoveryOutcome::Proceed(resp) => resp,
    };

    // ---- DISPATCH ----
    let action = dispatch(ctx, response, &modality).await?;
    ctx.state.iteration += 1;

    // ---- No-progress detection ----
    // An iteration that neither called a SUCCESSFUL tool nor produced a final
    // answer nor was actively generating (MaxTokens) is "idle". A few idle turns
    // in a row means the agent is spinning (or calling only failing tools)
    // without converging -> abort as stuck. A successful tool call, completion,
    // or active generation (MaxTokens) resets the streak. Note: a ToolUse
    // iteration where every tool errored is idle (tools_this_iter stays 0) -
    // stop_reason==ToolUse alone no longer counts as progress.
    let made_progress = !matches!(action, LoopAction::Continue)
        || ctx.state.tools_this_iter > 0
        || matches!(stop_reason, StopReason::MaxTokens);
    if let Some(streak) = ctx.state.record_iteration_progress(made_progress) {
        warn!(
            iteration = ctx.state.iteration,
            idle_streak = streak,
            "No progress for {streak} consecutive iterations - aborting turn as stuck"
        );
        return Err(CarrierError::LoopStuck(format!(
            "agent 连续 {streak} 轮无进展（无工具调用、无最终答案），判定卡死，终止本轮"
        )));
    }

    if ctx.state.declared_max_exceeded() {
        let n = ctx.state.max_iterations.unwrap_or(0);
        warn!(
            iteration = ctx.state.iteration,
            max_iterations = n,
            "Declared flow/subagent max_iterations exceeded — aborting"
        );
        return Err(CarrierError::LoopStuck(format!(
            "agent 已跑 {} 轮，超过本轮预算上限 max_iterations={n}+2（active_flow 声明值或 flow_load 加载点锚定值），判定卡死，终止本轮",
            ctx.state.iteration
        )));
    }

    Ok(action)
}

// ---------------------------------------------------------------------------
// Phase: PREPARE_TURN — context recovery, guard, status injection
// ---------------------------------------------------------------------------

fn prepare_turn(ctx: &mut LoopContext<'_>) {
    // Extract tools slice before mutating messages, to satisfy borrow checker
    // without cloning. Both recover_from_overflow and apply_context_guard only
    // read tools (they don't modify it).
    let tools = ctx.tools_owned.clone();
    let system_prompt = ctx.system_prompt.clone();
    let context_window_tokens = ctx.context_window_tokens;

    // Context overflow recovery pipeline
    let recovery = recover_from_overflow(
        &mut ctx.messages,
        &system_prompt,
        &tools,
        context_window_tokens,
    );
    match &recovery {
        RecoveryStage::None => {}
        RecoveryStage::FinalError => {
            warn!("Context overflow unrecoverable — suggest /reset or /compact");
            if let Some(tx) = &ctx.stream_tx {
                if tx
                    .try_send(StreamEvent::PhaseChange {
                        phase: "context_warning".to_string(),
                        detail: Some(
                            "Context overflow unrecoverable. Use /reset or /compact.".to_string(),
                        ),
                    })
                    .is_err()
                {
                    warn!("Stream consumer disconnected while sending context overflow warning");
                }
            }
        }
        _ => {
            if let Some(tx) = &ctx.stream_tx {
                if tx.try_send(StreamEvent::PhaseChange {
                    phase: "context_warning".to_string(),
                    detail: Some("Older messages trimmed to stay within context limits. Use /compact for smarter summarization.".to_string()),
                }).is_err() {
                    warn!("Stream consumer disconnected while sending context trim warning");
                }
            }
        }
    }

    // Context guard: compact oversized tool results before LLM call
    apply_context_guard(&mut ctx.messages, &ctx.context_budget, &tools);

    // Phase callback
    if let Some(cb) = ctx.on_phase {
        if ctx.stream_tx.is_some() && ctx.state.iteration == 0 {
            cb(LoopPhase::Streaming);
        } else {
            cb(LoopPhase::Thinking);
        }
    }

    // Inject loop status every turn so the model always has full context.
    {
        let status_msg = ctx.state.build_status_message();
        let should_inject = ctx
            .messages
            .last()
            .is_none_or(|m| !m.content.text_content().starts_with("📊 Turn"));
        if should_inject {
            tracing::info!(
                iteration = ctx.state.iteration,
                idle_streak = ctx.state.idle_streak,
                context_pressure = ?ctx.state.context_pressure,
                error_tools = ?ctx.state.error_tracker.failed_tools().collect::<Vec<_>>(),
                "Injecting loop status"
            );
            ctx.messages.push(Message::system(status_msg));
        }
    }
}

// ---------------------------------------------------------------------------
// Phase: Select modality
// ---------------------------------------------------------------------------

fn select_modality(ctx: &mut LoopContext<'_>) -> String {
    let default_modality = if ctx.manifest.model.modality.is_empty() {
        "chat"
    } else {
        &ctx.manifest.model.modality
    };
    // No time-budget pressure: just pick the configured modality. The previous
    // "remaining < 120s -> force reasoning for wrap-up" was a countdown-to-wall
    // hack that no longer applies (no wall-clock governor).
    helpers::pick_modality(ctx.brain.as_ref(), ctx.state.iteration, default_modality)
}

// ---------------------------------------------------------------------------
// Phase: LLM_CALL
// ---------------------------------------------------------------------------

async fn call_llm(ctx: &mut LoopContext<'_>, modality: &str) -> CarrierResult<CompletionResponse> {
    // Dedup tools by name before every LLM call. Duplicates can arise when
    // text-tool recovery re-adds tools already injected by a
    // flow; OpenAI-compatible APIs then reject with
    // "function name X is duplicated".
    let tools_for_llm = {
        let mut seen = std::collections::HashSet::new();
        ctx.tools()
            .iter()
            .filter(|t| seen.insert(t.name.clone()))
            .cloned()
            .collect::<Vec<_>>()
    };
    let request = CompletionRequest {
        model: String::new(),
        messages: ctx.messages.clone(),
        tools: tools_for_llm,
        max_tokens: ctx.manifest.model.max_tokens,
        temperature: ctx.manifest.model.temperature,
        system: Some(ctx.system_prompt.clone()),
        thinking: None,
        extra: Default::default(),
    };

    helpers::call_with_fallback(
        ctx.brain.as_ref(),
        &*ctx.driver,
        modality,
        request,
        ctx.stream_tx.clone(),
        ctx.llm_concurrency_limit.as_ref(),
    )
    .await
}

// ---------------------------------------------------------------------------
// Text tool call recovery
// ---------------------------------------------------------------------------

enum TextRecoveryOutcome {
    /// Retry this iteration (text recovery injected system messages).
    Continue,
    /// Proceed to dispatch with the (possibly modified) response.
    Proceed(CompletionResponse),
}

async fn handle_text_recovery(
    ctx: &mut LoopContext<'_>,
    mut response: CompletionResponse,
    modality: &str,
) -> TextRecoveryOutcome {
    if !matches!(
        response.stop_reason,
        StopReason::EndTurn | StopReason::StopSequence
    ) || !response.tool_calls.is_empty()
    {
        return TextRecoveryOutcome::Proceed(response);
    }

    // Detect whether the model narrated tool calls as `[Called name]` text
    // instead of emitting structured tool_use. aginxbrain normalizes raw
    // provider dialects to OpenAI tool_calls upstream, so we no longer scrape
    // provider-specific text formats — we only catch this provider-independent
    // narration and nudge the model to retry with structured tool_use (we do
    // NOT scrape arguments or execute text-described calls).
    let mentions = detect_text_tool_mentions(&response.text());
    if mentions.is_empty() {
        return TextRecoveryOutcome::Proceed(response);
    }

    // Flow deny_tools: never re-introduce blocked tools via text recovery.
    let deny: Vec<String> = ctx
        .manifest
        .metadata
        .get(carrier_types::flow::META_FLOW_DENY_TOOLS)
        .and_then(|v| {
            v.as_array().map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default();
    let flow_allowed: Vec<String> = ctx
        .manifest
        .metadata
        .get(carrier_types::flow::META_FLOW_ALLOWED_TOOLS)
        .and_then(|v| {
            v.as_array().map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default();
    let allowed_slice = if flow_allowed.is_empty() {
        None
    } else {
        Some(flow_allowed.as_slice())
    };

    // Discovery: resolve narrated tool names via the kernel and add any new
    // ones to the toolset so the retry can actually call them with tool_use.
    // Clone the handle out of `ctx` so we can freely mutate ctx.tools_owned.
    let kernel = ctx.kernel.clone();
    if let Some(k) = kernel.as_ref() {
        let max_level = ctx.manifest.max_tool_level;
        for name in &mentions {
            if deny.iter().any(|d| d == name) {
                info!(tool = %name, "Skipping text-narrated tool (flow deny_tools)");
                continue;
            }
            if !crate::tool_runner::tool_permitted_in_flow(name, allowed_slice) {
                info!(tool = %name, "Skipping text-narrated tool (flow tools sandbox)");
                continue;
            }
            // Dedup against tools already present (flow inject / prior tool_search).
            // Without this, recovery can re-add `web_fetch` etc. and the LLM API
            // rejects the request: "function name web_fetch is duplicated".
            if ctx.tools_owned.iter().any(|t| &t.name == name) {
                continue;
            }
            if let Some(def) = k
                .search_tools(name, 1, max_level)
                .into_iter()
                .next()
                .map(|(_, def)| def)
            {
                info!(tool = %def.name, schema = %def.input_schema, "Discovered tool schema");
                ctx.discovered_tool_names.insert(def.name.clone());
                ctx.tools_owned.push(def);
            }
        }
    }

    // Nudge the model to retry with structured tool_use (capped) instead of
    // executing text-described calls.
    if ctx.state.text_recovery_retries >= MAX_TEXT_RECOVERY_RETRIES {
        if ctx.state.text_recovery_final {
            // 08-21 86bus: after the final no-tools attempt the model could
            // still parrot the narration ("我需要调用工具：x。") — never relay
            // that to the user; replace the reply with an honest fallback.
            warn!(
                agent = %ctx.manifest.name,
                ctx.state.iteration,
                "Text recovery final attempt still narrating — replacing reply with fallback"
            );
            response.content = vec![ContentBlock::Text {
                text: crate::text_tool_recovery::NARRATION_FALLBACK_REPLY.to_string(),
                provider_metadata: None,
            }];
            response.tool_calls.clear();
            return TextRecoveryOutcome::Proceed(response);
        }
        // One final attempt with tools forbidden. The old code pushed this
        // guidance and then PROCEEDED with the narrated response — the guidance
        // was never consumed (turn ended) and the narration text went out as
        // the final answer. Continue instead so the extra call actually reads it.
        ctx.state.text_recovery_final = true;
        warn!(
            agent = %ctx.manifest.name,
            retries = ctx.state.text_recovery_retries,
            ctx.state.iteration,
            "Giving up structured tool recovery — final attempt, natural language only"
        );
        ctx.messages.push(Message::system(
            "多次尝试后你仍用文本描述工具调用而非结构化 tool_use。本轮不要再调用任何工具，直接用自然语言回复用户；禁止输出 [Called ...]、[调用 ...] 或『我需要调用工具：…』这类文本。",
        ));
        ctx.state.log_turn(
            modality,
            "text_recovery_final",
            response.usage.input_tokens as u32,
            response.usage.output_tokens as u32,
            Vec::new(),
            0,
        );
        TextRecoveryOutcome::Continue
    } else {
        ctx.state.text_recovery_retries += 1;
        warn!(
            agent = %ctx.manifest.name,
            tools = ?mentions,
            ctx.state.iteration,
            retry = ctx.state.text_recovery_retries,
            "LLM described tool calls as text — retrying with structured tool_use"
        );
        let tool_names = mentions.join("、");
        // 08-21 86bus 教训：不要把「我需要调用工具：X。」作为 assistant 先例注入——
        // 模型放弃工具后会逐字复读它当最终答案（原文直达用户）。工具名写进
        // system 引导即可，不造 assistant 文本。
        ctx.messages.push(Message::system(format!(
            "你刚才把工具调用（{tool_names}）写成了文本，用户会直接看到这段原始文本。这些工具已在你的可用工具列表中，请直接用 tool_use 发起结构化调用并带上完整参数。禁止输出 [Called ...]、[调用 ...] 或『我需要调用工具：…』这类文本。"
        )));
        ctx.state.log_turn(
            modality,
            "text_recovery_retry",
            response.usage.input_tokens as u32,
            response.usage.output_tokens as u32,
            response
                .tool_calls
                .iter()
                .map(|tc| tc.name.clone())
                .collect(),
            0,
        );
        TextRecoveryOutcome::Continue
    }
}

// ---------------------------------------------------------------------------
// Phase: DISPATCH — route by StopReason
// ---------------------------------------------------------------------------

async fn dispatch(
    ctx: &mut LoopContext<'_>,
    response: CompletionResponse,
    modality: &str,
) -> CarrierResult<LoopAction> {
    match response.stop_reason {
        StopReason::EndTurn | StopReason::StopSequence => {
            // EndTurn: sync messages, then delegate
            match end_turn::handle_end_turn(
                &response,
                ctx.session,
                &mut ctx.messages,
                ctx.manifest,
                ctx.memory,
                ctx.kernel.as_ref(),
                ctx.memory_handle.as_ref(),
                ctx.brain.as_ref(),
                ctx.hooks,
                ctx.on_phase,
                ctx.session_base_len,
                ctx.user_message,
                ctx.owner_id,
                ctx.sender_id,
                ctx.channel_type,
                &ctx.agent_id_str,
                ctx.state.iteration,
                ctx.state.total_usage,
                ctx.state.any_tools_executed,
            )
            .await?
            {
                end_turn::EndTurnAction::Retry => return Ok(LoopAction::Continue),
                end_turn::EndTurnAction::Complete(result) => {
                    ctx.persist_last_run(state::RunOutcome::Complete);
                    return Ok(LoopAction::Complete(result));
                }
            }
        }
        StopReason::ToolUse => {
            match tool_use::handle_tool_use(
                &mut { response },
                ctx.session,
                &mut ctx.messages,
                ctx.manifest,
                ctx.memory,
                ctx.kernel.as_ref(),
                ctx.memory_handle.as_ref(),
                ctx.brain.as_ref(),
                ctx.hooks,
                ctx.on_phase,
                &ctx.stream_tx,
                ctx.mcp_connections,
                ctx.workspace_root,
                ctx.process_manager,
                &ctx.context_budget,
                &ctx.hand_allowed_env,
                ctx.sender_id,
                ctx.owner_id,
                ctx.channel_type,
                &mut ctx.state.consecutive_max_tokens,
                &mut ctx.state.any_tools_executed,
                &mut ctx.state.tools_this_iter,
                &mut ctx.state.recent_tool_calls,
                &mut ctx.loaded_flows,
                &mut ctx.loaded_flow_shell_allow,
                &mut ctx.loaded_flow_elevated_tools,
                &mut ctx.state.error_tracker,
                &mut ctx.state.tool_loop_rearm,
                &mut ctx.state.tool_call_counts,
                &mut ctx.state.max_iterations,
                ctx.session_base_len,
                ctx.state.iteration,
            )
            .await
            {
                tool_use::ToolUseAction::Continue => {}
                tool_use::ToolUseAction::BreakWithPlan(plan) => {
                    ctx.detected_plan = Some(plan);
                    return Ok(LoopAction::BreakForPlan);
                }
                tool_use::ToolUseAction::BreakToolLoop(msg) => {
                    return Err(CarrierError::LoopStuck(msg));
                }
            }
        }
        StopReason::MaxTokens => {
            match max_tokens::handle_max_tokens(
                &response,
                ctx.session,
                &mut ctx.messages,
                ctx.memory,
                &ctx.stream_tx,
                &mut ctx.state.consecutive_max_tokens,
                ctx.hooks,
                &ctx.agent_id_str,
                ctx.manifest,
                ctx.state.iteration,
                ctx.state.total_usage,
                ctx.session_base_len,
            )
            .await
            {
                max_tokens::MaxTokensAction::Continue => {
                    ctx.state.log_turn(
                        modality,
                        "max_tokens_continue",
                        response.usage.input_tokens as u32,
                        response.usage.output_tokens as u32,
                        vec![],
                        0,
                    );
                }
                max_tokens::MaxTokensAction::Complete(result) => {
                    ctx.persist_last_run(state::RunOutcome::Complete);
                    return Ok(LoopAction::Complete(result));
                }
            }
        }
    }
    Ok(LoopAction::Continue)
}

// ---------------------------------------------------------------------------
// Phase: TEARDOWN - persist loop state, return plan (or abnormal-exit error)
// ---------------------------------------------------------------------------

async fn teardown(ctx: &mut LoopContext<'_>) -> CarrierResult<AgentLoopResult> {
    // Reached only via BreakForPlan (a task_plan was detected mid-loop). There
    // is no max-iterations kill anymore - the loop runs until completion or
    // stuck detection - so this path exclusively returns the detected plan.
    ctx.persist_last_run(state::RunOutcome::Complete);

    // O6: Single-track - sync before teardown save
    helpers::sync_loop_messages(&ctx.messages, ctx.session, ctx.session_base_len);

    // Fire AgentLoopEnd hook
    if let Some(hook_reg) = ctx.hooks {
        let ctx_hook = crate::hooks::HookContext {
            agent_name: &ctx.manifest.name,
            agent_id: ctx.agent_id_str.as_str(),
            event: carrier_types::agent::HookEvent::AgentLoopEnd,
            data: serde_json::json!({
                "reason": "plan_detected",
                "iterations": ctx.state.iteration,
            }),
        };
        let _ = hook_reg.fire(&ctx_hook);
    }

    // If task_plan was detected, return success with the plan
    if let Some(plan) = ctx.detected_plan.take() {
        return Ok(AgentLoopResult {
            response: format!(
                "Plan '{}' created with {} steps. Executing...",
                plan.title,
                plan.steps.len()
            ),
            total_usage: ctx.state.total_usage,
            iterations: ctx.state.iteration + 1,
            silent: false,
            directives: Default::default(),
            plan: Some(plan),
        });
    }

    // Defensive: teardown reached without a plan (should not happen - the loop
    // only `break`s on BreakForPlan). Treat as an abnormal exit.
    let e = CarrierError::Internal(format!(
        "agent loop exited abnormally at iteration {} without completion or plan",
        ctx.state.iteration
    ));
    ctx.persist_last_run(state::outcome_from_loop_err(&e));
    Err(e)
}

#[cfg(test)]
mod tests;
