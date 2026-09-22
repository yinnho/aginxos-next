//! Handler for the ToolUse stop reason.
//!
//! When the LLM requests tool execution, this handler:
//! - Tracks tool calls for loop detection
//! - Executes each tool with timeout and truncation
//! - Handles flow_load deduplication
//! - Tracks consecutive tool errors
//! - Logs flow_load skill loads (tool list refresh now happens via flow inject)
//! - Detects task_plan and signals a loop break

use super::*;

use crate::context_budget::{truncate_tool_result_dynamic, ContextBudget};
use crate::hooks::HookRegistry;
use crate::kernel_handle::KernelHandle;
use crate::llm_driver::{Brain, StreamEvent};
use crate::mcp::McpConnection;
use crate::tool_context::ToolContext;
use crate::tool_runner;
use carrier_memory::MemorySubstrate;
use carrier_types::message::{ContentBlock, Message, MessageContent, Role};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

/// When a tool fails this many consecutive times, escalate the feedback
/// to urge the LLM to change approach entirely. (Tools are NOT removed —
/// we educate, not punish.)
pub(in crate::agent_loop) const ERROR_ESCALATION_THRESHOLD: u32 = 3;

/// How many times the SAME tool may trip loop detection before we stop the
/// turn. Each detection already costs `LOOP_DETECTION_WINDOW` identical calls,
/// so at threshold 3 the agent has burned ~12 iterations and ignored two
/// escalating nudges — continuing just wastes the rest of `max_iterations`.
/// The tool is still never removed; we only fail the turn fast with a clear
/// reason (better than a silent `MaxIterationsExceeded`).
pub(in crate::agent_loop) const LOOP_BREAK_THRESHOLD: u32 = 3;

/// Action the main loop should take after handling a ToolUse.
pub(in crate::agent_loop) enum ToolUseAction {
    /// The loop should continue (normal tool execution completed).
    Continue,
    /// The loop should break — a task_plan was detected.
    BreakWithPlan(TaskPlan),
    /// The loop should fail fast — the agent is stuck in a tool loop after
    /// repeated corrective guidance (see [`LOOP_BREAK_THRESHOLD`]).
    BreakToolLoop(String),
}

/// flow_load 的预算收紧判定：仅当候选上限比当前更紧时应用。
/// `None` = 本轮尚无声明上限（聊天默认预算），任何有限上限都收紧；
/// 已有值时取 min 语义--flow_load 永远不能把 turn 的上限改宽松。
/// 候选值由调用方按"加载点轮次 + 声明值"锚定后传入（见 flow_load 块）。
fn should_tighten_max_iterations(current: Option<u32>, candidate: u32) -> bool {
    match current {
        Some(cur) => candidate < cur,
        None => true,
    }
}

/// Handle a `StopReason::ToolUse` response.
///
/// Executes each tool call, handles loop detection, error tracking,
/// skill deduplication, dynamic tool discovery, and task_plan detection.
///
/// Returns a `ToolUseAction` indicating whether the loop should continue
/// or break (when a task_plan is produced).
#[allow(clippy::too_many_arguments)]
pub(in crate::agent_loop) async fn handle_tool_use(
    response: &mut CompletionResponse,
    session: &mut Session,
    messages: &mut Vec<Message>,
    manifest: &AgentManifest,
    memory: &MemorySubstrate,
    kernel: Option<&Arc<dyn KernelHandle>>,
    memory_handle: Option<&Arc<dyn crate::memory_handle::MemoryHandle>>,
    brain: Option<&Arc<dyn Brain>>,
    hooks: Option<&HookRegistry>,
    on_phase: Option<&PhaseCallback>,
    stream_tx: &Option<tokio::sync::mpsc::Sender<StreamEvent>>,
    mcp_connections: Option<&dashmap::DashMap<String, McpConnection>>,
    workspace_root: Option<&Path>,
    process_manager: Option<&crate::process_manager::ProcessManager>,
    context_budget: &ContextBudget,
    hand_allowed_env: &[String],
    sender_id: Option<&str>,
    owner_id: Option<&str>,
    channel_type: Option<&str>,
    // Mutable loop state
    consecutive_max_tokens: &mut u32,
    any_tools_executed: &mut bool,
    tools_this_iter: &mut u32,
    recent_tool_calls: &mut Vec<(String, u64)>,
    loaded_flows: &mut std::collections::HashSet<String>,
    loaded_flow_shell_allow: &mut Vec<String>,
    loaded_flow_elevated_tools: &mut Vec<String>,
    error_tracker: &mut crate::agent_loop::state::ToolErrorTracker,
    tool_loop_rearm: &mut std::collections::HashMap<String, u32>,
    tool_call_counts: &mut std::collections::HashMap<(String, u64), u32>,
    // Declared-budget override: flow_load applies loaded flows' max_iterations
    // here (tighten-only — see the flow_load block below).
    max_iterations: &mut Option<u32>,
    // For task_plan save
    session_base_len: usize,
    iteration: u32,
) -> ToolUseAction {
    // Reset MaxTokens continuation counter on tool use
    *consecutive_max_tokens = 0;
    *any_tools_executed = true;
    // Note: tools_this_iter is bumped per SUCCESSFUL tool execution below (after
    // the execute_tool call), not here on entry. A ToolUse iteration where every
    // tool call errored counts as no-progress for the idle detector (Problem 3).

    let assistant_blocks = response.content.clone();

    // O6: Single-track — only push to messages, not session
    messages.push(Message {
        role: Role::Assistant,
        content: MessageContent::Blocks(assistant_blocks),
    });

    let caller_id_str = session.agent_name.to_string();

    // Track tool calls for loop detection BEFORE execution — denied/errored
    // calls count too: a model hammering a denied call (allowlist wall,
    // missing tool) is exactly the loop worth breaking.
    let mut cumulative_warnings: Vec<String> = Vec::new();
    for tc in &response.tool_calls {
        let key = super::helpers::tool_call_key(&tc.name, &tc.input);
        recent_tool_calls.push(key.clone());
        // Cumulative (whole-turn) repetition counter — distinct from the
        // sliding window above. Survives recent_tool_calls.clear(). Catches
        // ROTATING repetition (same call interleaved with others so the
        // consecutive window never fills) that detect_tool_loop misses.
        // Progressive (dsh repeat-tool-reminder cadence): remind at 3,
        // escalate at 5, abort at 8 — educate before punishing.
        let count = {
            let e = tool_call_counts.entry(key.clone()).or_insert(0);
            *e += 1;
            *e
        };
        if count == super::helpers::CUMULATIVE_BREAK_AT {
            warn!(
                agent = %manifest.name,
                tool = %key.0,
                count,
                threshold = super::helpers::CUMULATIVE_BREAK_AT,
                iteration,
                "Cumulative tool-call repetition (rotating) — aborting turn \
                 (consecutive-window loop detection misses interleaved repetition)"
            );
            return ToolUseAction::BreakToolLoop(format!(
                "agent re-called `{name}` with identical args {count}× total this turn \
                 (interleaved with other calls so the consecutive loop detector never fired) \
                 and ignored two escalating warnings. This is rotating repetition — the agent \
                 is re-reading inputs without progressing, which guidance does not fix. \
                 Turn aborted to save the iteration budget. Fix the flow/tool guidance \
                 and retry.",
                name = key.0
            ));
        }
        if count == super::helpers::CUMULATIVE_ESCALATE_AT {
            warn!(
                agent = %manifest.name,
                tool = %key.0,
                count,
                iteration,
                "Cumulative tool-call repetition — escalating warning injected"
            );
            cumulative_warnings.push(format!(
                "⚠️⚠️ 工具 `{name}`（参数 {preview}）本轮已第 {count} 次被完全相同地调用，\
                 此前的提醒没有生效。必须立刻停止重复该调用——下一步要么换一种做法（换参数/换工具），\
                 要么直接基于已有结果产出最终答案。第 {break_at} 次相同调用将终止本任务。",
                name = key.0,
                preview = super::helpers::tool_args_preview(&tc.input, 120),
                break_at = super::helpers::CUMULATIVE_BREAK_AT,
            ));
        } else if count == super::helpers::CUMULATIVE_REMIND_AT {
            warn!(
                agent = %manifest.name,
                tool = %key.0,
                count,
                iteration,
                "Cumulative tool-call repetition — reminder injected"
            );
            cumulative_warnings.push(format!(
                "注意：工具 `{name}`（参数 {preview}）本轮已第 {count} 次被完全相同地调用。\
                 重复同样的调用不会产生新结果——请改变做法：换参数、换工具，或直接基于已有结果产出答案。",
                name = key.0,
                preview = super::helpers::tool_args_preview(&tc.input, 120),
            ));
        }
    }
    for warning in cumulative_warnings {
        messages.push(Message::system(&warning));
    }
    if recent_tool_calls.len() > super::helpers::LOOP_DETECTION_WINDOW * 3 {
        let drain_count = recent_tool_calls.len() - super::helpers::LOOP_DETECTION_WINDOW * 2;
        recent_tool_calls.drain(..drain_count);
    }

    // Detect loop: same (name, input_hash) repeated LOOP_DETECTION_WINDOW times.
    // We educate, not punish (see d7037bd — "Tools are never removed"): yanking
    // the looping tool just leaves the agent floundering without the tool it
    // actually needs. Instead we inject an escalating system message so the LLM
    // changes approach (e.g. stop re-reading the same path and call file_write
    // to write/overwrite the target). The tool stays available. After
    // LOOP_BREAK_THRESHOLD ignored nudges we fail the turn fast rather than
    // burning the whole max_iterations budget.
    if let Some((looping_name, _)) =
        super::helpers::detect_tool_loop(recent_tool_calls, super::helpers::LOOP_DETECTION_WINDOW)
    {
        let rearm = tool_loop_rearm.entry(looping_name.clone()).or_insert(0);
        *rearm += 1;
        let rearm = *rearm;
        warn!(
            agent = %manifest.name,
            tool = %looping_name,
            consecutive = super::helpers::LOOP_DETECTION_WINDOW,
            rearm,
            iteration,
            "Tool loop detected — injecting corrective guidance (tool NOT removed)"
        );
        recent_tool_calls.clear();
        error_tracker.remove(&looping_name);
        if rearm >= LOOP_BREAK_THRESHOLD {
            // Stuck after repeated nudges — fail fast with a clear reason
            // instead of silently burning the rest of max_iterations.
            return ToolUseAction::BreakToolLoop(format!(
                "agent stuck in a tool loop on `{looping_name}` after {rearm} corrective \
                 nudges (identical call repeated {}× each time). The tool was not removed, \
                 but the turn is aborted to avoid wasting the iteration budget — fix the \
                 flow/tool guidance and retry.",
                super::helpers::LOOP_DETECTION_WINDOW
            ));
        }
        // Inject a system message; escalate wording on the 2nd+ nudge.
        let warning = if rearm >= 2 {
            format!(
                "⚠️⚠️ 工具 `{looping_name}` 已第 {rearm} 次陷入完全相同的循环——上一条纠正没有生效。\
                 ��必须在下一步换做法：停止调用 `{looping_name}`，直接产出最终答案或改用其他工具/路径。\
                 再循环一次，本任务将被终止。"
            )
        } else {
            format!(
                "工具 `{looping_name}` 连续多次返回相同结果——你在原地打转，请立刻换方式完成任务。\
                 常见情况：该写文件时直接调用 `file_write(path=..., content=...)` 写入或覆盖目标文件，\
                 不要反复 `file_read` 同一路径。本任务若加载了 flow，只用 flow `tools:` 声明的工具；\
                 若 flow 未加载导致工具集不对，成功后用 flow_update 固化正确工具列表。"
            )
        };
        messages.push(Message::system(&warning));
    }

    // Execute each tool call with timeout and truncation
    let mut tool_result_blocks = Vec::new();
    for tool_call in &response.tool_calls {
        // Canonicalize name up front so trailing punctuation from free-text
        // recovery (`web_search,`) and aliases hit the right tool path.
        let tool_name =
            carrier_types::tool_compat::normalize_tool_name(&tool_call.name).to_string();
        debug!(tool = %tool_name, id = %tool_call.id, "Executing tool");

        // Notify phase: ToolUse
        if let Some(cb) = on_phase {
            let sanitized: String = tool_name
                .chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect();
            cb(LoopPhase::ToolUse {
                tool_name: sanitized,
            });
        }

        // Fire BeforeToolCall hook (can block execution)
        if let Some(hook_reg) = hooks {
            let ctx = crate::hooks::HookContext {
                agent_name: &manifest.name,
                agent_id: &caller_id_str,
                event: carrier_types::agent::HookEvent::BeforeToolCall,
                data: serde_json::json!({
                    "tool_name": &tool_name,
                    "input": &tool_call.input,
                }),
            };
            if let Err(reason) = hook_reg.fire(&ctx) {
                tool_result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tool_call.id.clone(),
                    tool_name: tool_name.clone(),
                    content: format!("Hook blocked tool '{tool_name}': {reason}"),
                    is_error: true,
                });
                continue;
            }
        }

        // Resolve effective exec policy (per-agent override or global)
        let effective_exec_policy = manifest.exec_policy.as_ref();

        let home_dir_buf = kernel.and_then(|k| k.home_dir());
        let external_url_buf = kernel.and_then(|k| k.external_url());

        // Check if sender is a clone admin
        let is_clone_admin = if let (Some(sid), Some(root)) = (sender_id, workspace_root) {
            crate::plugin::admin_store::is_admin(root, sid)
        } else {
            false
        };

        // Turn-scoped system-flow elevation stamped onto manifest.metadata.
        let flow_elevated_owned: Vec<String> = manifest
            .metadata
            .get(carrier_types::flow::META_FLOW_ELEVATED_TOOLS)
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default();
        // Union tools granted by elevating flows loaded mid-turn via flow_load —
        // they bypass the level/admin gates exactly like turn-start elevated
        // tools (`flow_elevated` in tool_runner).
        let mut flow_elevated_owned = flow_elevated_owned;
        for t in loaded_flow_elevated_tools.iter() {
            if !flow_elevated_owned.contains(t) {
                flow_elevated_owned.push(t.clone());
            }
        }
        let mut flow_shell_allow_owned: Vec<String> = manifest
            .metadata
            .get(carrier_types::flow::META_FLOW_SHELL_ALLOW)
            .and_then(|v| {
                v.as_array().map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
            })
            .unwrap_or_default();
        // Union with patterns granted by flows loaded mid-turn via `flow_load`:
        // loading a flow injects its body but historically left the shell gate
        // frozen to the active/classified flow, so the loaded flow's `scripts/`
        // were denied ("Allowed patterns" of the wrong flow — 2026-08-15
        // wechat-writer: flow_load(topic-researcher) then denied its validator).
        for pat in loaded_flow_shell_allow.iter() {
            if !flow_shell_allow_owned.contains(pat) {
                flow_shell_allow_owned.push(pat.clone());
            }
        }
        let flow_deny_owned: Vec<String> = manifest
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
        let flow_allowed_owned: Vec<String> = manifest
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
        // Widen the frozen hard sandbox by the elevating loaded flows' declared
        // tools — an explicitly flow_load-ed flow's tools are sanctioned, so
        // the active flow's sandbox must not cage them (e.g. a chat turn
        // caged to a flow without shell_exec, then the agent loads a validator
        // flow that declares it).
        let mut flow_allowed_owned = flow_allowed_owned;
        for t in loaded_flow_elevated_tools.iter() {
            if !flow_allowed_owned.contains(t) {
                flow_allowed_owned.push(t.clone());
            }
        }

        let tool_ctx = ToolContext {
            kernel,
            memory: memory_handle,
            caller_agent_id: Some(&caller_id_str),
            mcp_connections,
            allowed_env_vars: if hand_allowed_env.is_empty() {
                None
            } else {
                Some(hand_allowed_env)
            },
            workspace_root,
            brain,
            exec_policy: effective_exec_policy,
            cli_exec_config: manifest.cli_exec.as_ref(),

            process_manager,
            sender_id,
            owner_id,
            home_dir: home_dir_buf.as_deref(),
            agent_name: Some(&manifest.name),
            subagent_configs: if manifest.subagents.is_empty() {
                None
            } else {
                Some(&manifest.subagents)
            },
            channel_type,
            max_tool_level: manifest.max_tool_level,
            is_clone_admin,
            external_url: external_url_buf.as_deref(),
            flow_elevated_tools: if flow_elevated_owned.is_empty() {
                None
            } else {
                Some(flow_elevated_owned.as_slice())
            },
            flow_shell_allow: if flow_shell_allow_owned.is_empty() {
                None
            } else {
                Some(flow_shell_allow_owned.as_slice())
            },
            flow_deny_tools: if flow_deny_owned.is_empty() {
                None
            } else {
                Some(flow_deny_owned.as_slice())
            },
            flow_allowed_tools: if flow_allowed_owned.is_empty() {
                None
            } else {
                Some(flow_allowed_owned.as_slice())
            },
        };

        // Timeout-wrapped execution
        let timeout_secs = if super::helpers::TOOL_LONG_TIMEOUT_NAMES.contains(&tool_name.as_str())
        {
            super::helpers::TOOL_TIMEOUT_LONG_SECS
        } else {
            super::helpers::TOOL_TIMEOUT_SECS
        };
        let result = match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            tool_runner::execute_tool(&tool_call.id, &tool_name, &tool_call.input, &tool_ctx),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                warn!(tool = %tool_name, "Tool execution timed out after {}s", timeout_secs);
                carrier_types::tool::ToolResult {
                    tool_use_id: tool_call.id.clone(),
                    content: format!("Tool '{tool_name}' timed out after {timeout_secs}s."),
                    is_error: true,
                }
            }
        };

        // Count only SUCCESSFUL tool executions as progress for the no-progress
        // detector (Problem 3): an iteration where every tool call errored
        // (permission denied, path traversal, not found, timeout) leaves
        // tools_this_iter == 0 and is treated as idle. A single success anywhere
        // in the iteration marks it as progress.
        if !result.is_error {
            *tools_this_iter = tools_this_iter.saturating_add(1);
        }

        // Fire AfterToolCall hook
        if let Some(hook_reg) = hooks {
            let ctx = crate::hooks::HookContext {
                agent_name: &manifest.name,
                agent_id: caller_id_str.as_str(),
                event: carrier_types::agent::HookEvent::AfterToolCall,
                data: serde_json::json!({
                    "tool_name": &tool_name,
                    "result": &result.content,
                    "is_error": result.is_error,
                }),
            };
            let _ = hook_reg.fire(&ctx);
        }

        // Skill load deduplication: if the same skill was already loaded
        // in this agent loop, replace the full content with a short hint.
        // This prevents the LLM from looping on flow_load without executing.
        if tool_name == "flow_load" {
            let skill_name = tool_call.input["name"]
                .as_str()
                .unwrap_or("")
                .to_lowercase();
            if !skill_name.is_empty() {
                if loaded_flows.contains(&skill_name) {
                    warn!(
                        agent = %manifest.name,
                        skill = %skill_name,
                        iteration,
                        "flow_load called for already-loaded flow — returning dedup hint"
                    );
                    let dedup_msg = format!(
                        "Flow '{skill_name}' 已经加载过了，请直接按步骤执行，不要再调用 flow_load。"
                    );
                    tool_result_blocks.push(ContentBlock::ToolResult {
                        tool_use_id: result.tool_use_id,
                        tool_name: tool_name.clone(),
                        content: dedup_msg,
                        is_error: false,
                    });
                    continue;
                } else {
                    loaded_flows.insert(skill_name.clone());
                    let loaded_def = carrier_types::flow::parse_flow_def(&result.content);
                    // Apply the loaded flow's declared max_iterations
                    // (tighten-only). active_flow stamps
                    // META_MAX_ITERATIONS_DECLARED before the loop starts, but
                    // a flow loaded mid-turn via flow_load never did - its
                    // budget silently didn't apply (2026-08-19: outline-writer
                    // declared 12, chat-initiated turn ran to iteration 15).
                    // The candidate is ANCHORED at the load point: the declared
                    // value counts from adoption, not from turn start. The
                    // enforcement check (state.declared_max_exceeded) compares
                    // against the cumulative iteration counter, so applying the
                    // raw declared value would hard-stop a long chat turn the
                    // moment it loads a small-cap flow (iteration 12 + declared
                    // 5 -> next loop check aborts mid-work).
                    // Turn-local by design: manifest metadata stays the
                    // turn-START source only - never re-derive max_iterations
                    // from the manifest mid-turn; re-entry (plan step / resume)
                    // rebuilds state from scratch and re-tightens on re-load.
                    if let Some(flow_max) = loaded_def.max_iterations {
                        let anchored = iteration.saturating_add(flow_max);
                        if should_tighten_max_iterations(*max_iterations, anchored) {
                            *max_iterations = Some(anchored);
                            info!(
                                agent = %manifest.name,
                                skill = %skill_name,
                                anchored_max_iterations = anchored,
                                "flow_load: applying flow-declared max_iterations (tighten, load-anchored)"
                            );
                        }
                    }
                    // Grant the loaded flow's `shell_allow` for the rest of the
                    // turn — mirrors what stamping it as active_flow would do.
                    // flow_load reads clone-authored flow files only, so this
                    // grants nothing the clone author didn't already declare.
                    for pat in &loaded_def.shell_allow {
                        if !pat.is_empty() && !loaded_flow_shell_allow.contains(pat) {
                            loaded_flow_shell_allow.push(pat.clone());
                        }
                    }
                    // Turn-scoped elevation for the loaded flow's declared
                    // tools. Without this, the shell_allow union above was dead
                    // code for a Write-capped agent: the level gate
                    // (`level > max_tool_level && !flow_elevated`) rejects
                    // shell_exec BEFORE the pattern gate ever runs, so an agent
                    // that explicitly loaded a validator flow in a chat turn
                    // still couldn't run its scripts (2026-08-17 86bus:
                    // interactive outline step denied "requires Dangerous
                    // level but agent is limited to Write" while the same flow
                    // elevated fine via cron active_flow). An explicit
                    // flow_load is sanctioned intent — the default_flow
                    // fallback cage never reaches this path.
                    if loaded_def.elevates() {
                        let mut granted = 0;
                        for t in &loaded_def.tools {
                            if !t.is_empty() && !loaded_flow_elevated_tools.contains(t) {
                                loaded_flow_elevated_tools.push(t.clone());
                                granted += 1;
                            }
                        }
                        if granted > 0 {
                            info!(
                                agent = %manifest.name,
                                skill = %skill_name,
                                granted,
                                total = loaded_flow_elevated_tools.len(),
                                "flow_load: flow elevates — granting turn-scoped tool authority"
                            );
                        }
                    }
                }
            }
        }

        // Dynamic truncation based on context budget (replaces flat MAX_TOOL_RESULT_CHARS)
        let final_content = truncate_tool_result_dynamic(&result.content, context_budget);

        // Notify client of tool execution result (detect dead consumer)
        if let Some(tx) = stream_tx {
            let preview: String = final_content.chars().take(300).collect();
            if tx
                .send(StreamEvent::ToolExecutionResult {
                    id: tool_call.id.clone(),
                    name: tool_name.clone(),
                    result_preview: preview,
                    is_error: result.is_error,
                })
                .await
                .is_err()
            {
                warn!(agent = %manifest.name, "Stream consumer disconnected — continuing tool loop but will not stream further");
            }
        }

        tool_result_blocks.push(ContentBlock::ToolResult {
            tool_use_id: result.tool_use_id,
            tool_name: tool_name.clone(),
            content: final_content,
            is_error: result.is_error,
        });
    }

    // Detect tool errors and inject guidance to prevent fabrication
    let error_count = tool_result_blocks
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolResult { is_error: true, .. }))
        .count();

    // Record success/failure in sliding window tracker (O5: replaces HashMap reset-on-success)
    let succeeded_tools: Vec<&str> = tool_result_blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                is_error: false,
                tool_name,
                ..
            } => Some(tool_name.as_str()),
            _ => None,
        })
        .collect();
    for name in &succeeded_tools {
        error_tracker.record(name, true);
    }

    if error_count > 0 {
        // Collect failed tool names AND their error messages for actionable feedback
        let failed_tools: Vec<(&str, &str)> = tool_result_blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult {
                    is_error: true,
                    tool_name,
                    content,
                    ..
                } => Some((tool_name.as_str(), content.as_str())),
                _ => None,
            })
            .collect();

        let failed_names: Vec<&str> = failed_tools.iter().map(|(n, _)| *n).collect();

        // Increment consecutive error counters (keep counting, but do NOT remove tools)
        for name in &failed_names {
            error_tracker.record(name, false);
        }

        info!(
            agent = %manifest.name,
            iteration,
            error_count,
            failed_tools = ?failed_names,
            "Tool errors in agent loop iteration"
        );

        // Build actionable, per-tool error analysis with escalating detail.
        // We do NOT remove tools — the LLM can still succeed with correct params.
        let mut guidance = String::from("[工具错误分析 — 不要编造结果，也不要用相同参数重试。\n");
        for (name, err_msg) in &failed_tools {
            let count = error_tracker.consecutive_failures(name).max(1);
            // Truncate long error messages for readability
            let short_err: String = err_msg.chars().take(200).collect();
            let ellipsis = if err_msg.chars().count() > 200 {
                "..."
            } else {
                ""
            };

            // Escalating detail based on how many times this tool has failed
            let suggestion = if count >= ERROR_ESCALATION_THRESHOLD {
                format!(
                    " ⚠️ 这个工具已经连续失败 {count} 次。你可能一直用错了方法——\
                     仔细看上面的错误原因，换个完全不同的参数或方案，或者直接告诉用户遇到了什么困难。"
                )
            } else if count == 2 {
                " 上次也失败了，请仔细确认参数后再试。".to_string()
            } else {
                String::new()
            };

            guidance.push_str(&format!(" ❌ {name} → {short_err}{ellipsis}{suggestion}\n"));
        }
        guidance.push_str("修正方法：分析上面的错误原因，用不同的参数或换一个合适的工具重试。]");

        tool_result_blocks.push(ContentBlock::Text {
            text: guidance,
            provider_metadata: None,
        });
    }

    let tool_results_msg = Message {
        role: Role::User,
        content: MessageContent::Blocks(tool_result_blocks.clone()),
    };
    // O6: Single-track — only push to messages, not session
    messages.push(tool_results_msg);

    // flow_load 可观测性（流式路径）。tool_search 已退役（M31 D3 批1，
    // 宪法性替代：`ag commands`）——工具发现的两条活路径：flow `tools:`
    // 注入（messaging.rs inject_flow_tools）与文本恢复期的名字解析
    // （mod.rs 恢复块），都不经这里。
    let flow_load_count = response
        .tool_calls
        .iter()
        .filter(|tc| tc.name == "flow_load")
        .count();
    if flow_load_count > 0 {
        info!(count = flow_load_count, "Skill(s) loaded");
    }

    // Note: no per-iteration save here — save happens at loop end
    // (success -> full save, failure -> summary only)

    // Detect task_plan: extract plan data and break out of the loop
    if let Some(tc) = response.tool_calls.iter().find(|tc| tc.name == "task_plan") {
        let title = tc.input["title"].as_str().unwrap_or("").to_string();
        let steps: Vec<TaskStep> = tc.input["steps"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        Some(TaskStep {
                            id: s["id"].as_str()?.to_string(),
                            prompt: s["prompt"].as_str()?.to_string(),
                            depends_on: s["depends_on"]
                                .as_array()
                                .map(|d| {
                                    d.iter()
                                        .filter_map(|v| v.as_str().map(String::from))
                                        .collect()
                                })
                                .unwrap_or_default(),
                            flow: s["flow"].as_str().map(String::from),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if !steps.is_empty() {
            info!(
                plan_title = %title,
                steps = steps.len(),
                "task_plan detected — breaking out of agent loop"
            );
            // Save session before breaking (inline version of save_new! macro)
            super::helpers::sync_loop_messages(messages, session, session_base_len);
            let new_msgs = &session.messages[session_base_len..];
            if let Err(e) = memory
                .save_session_append_async(
                    session.id,
                    &session.agent_name,
                    new_msgs,
                    session.context_window_tokens,
                    session.label.as_deref(),
                    Some(&session.turn_summaries),
                )
                .await
            {
                warn!("Failed to save session before plan break: {e}");
            }
            return ToolUseAction::BreakWithPlan(TaskPlan { title, steps });
        }
    }

    ToolUseAction::Continue
}

#[cfg(test)]
mod tests {
    use super::should_tighten_max_iterations;

    /// 无声明上限（聊天默认预算）-> 任何有限上限都收紧。
    /// 注意 None 分支收到的已是"加载点轮次 + 声明值"锚定后的候选值。
    #[test]
    fn tighten_applies_when_no_current_cap() {
        assert!(should_tighten_max_iterations(None, 12));
        assert!(should_tighten_max_iterations(None, 1));
    }

    /// 更紧的候选上限应用（min 语义，永不放宽）。
    #[test]
    fn tighten_applies_only_when_stricter() {
        assert!(should_tighten_max_iterations(Some(60), 12));
        assert!(!should_tighten_max_iterations(Some(12), 12));
        assert!(!should_tighten_max_iterations(Some(10), 12));
    }

    /// 加载点锚定：长 turn 中途加载小预算 flow，候选 = 已耗轮次 + 声明值，
    /// 不会立刻硬停（iteration 12 + 声明 5 -> 上限 17 而非 5）。
    #[test]
    fn anchored_candidate_not_below_elapsed_iterations() {
        let elapsed = 12u32;
        let flow_max = 5u32;
        let anchored = elapsed.saturating_add(flow_max);
        assert_eq!(anchored, 17);
        // 无既有上限：应用锚定值（17），而非裸声明值（5）。
        assert!(should_tighten_max_iterations(None, anchored));
        // 既有上限比锚定值更紧：保持既有值。
        assert!(!should_tighten_max_iterations(Some(15), anchored));
        // 既有上限更松：收紧到锚定值。
        assert!(should_tighten_max_iterations(Some(60), anchored));
    }
}
