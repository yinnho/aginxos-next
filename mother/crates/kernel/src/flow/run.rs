//! Multi-step flow DAG main loop (`run_flow`).

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::{info, warn};

use carrier_memory::FlowRunRow;
use carrier_runtime::agent_loop::AgentLoopResult;
use carrier_runtime::kernel_handle::KernelHandle;
use carrier_runtime::llm_driver::Brain;
use carrier_types::agent::{AgentId, AgentManifest};
use carrier_types::error::CarrierError;
use carrier_types::flow::{FlowDef, StepKind};
use carrier_types::message::{Message, TokenUsage};

use super::dag::partition_flow_steps;
use super::report::build_failure_report;
use super::template::{decide_cancel, eval_when, render_template, value_to_string};
use super::types::*;
use super::FAILURE_CANCEL_KEYWORDS;
use crate::error::{KernelError, KernelResult};
use crate::kernel::CarrierKernel;

impl CarrierKernel {
    /// Execute a multi-step flow as a DAG. Returns the final step's output as an
    /// [`AgentLoopResult`] (`Completed`), or `Suspended` when a `user_input`
    /// step pauses execution to await the human's reply.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_flow(
        &self,
        agent_id: AgentId,
        flow: &FlowDef,
        base_system_prompt: &str,
        user_message: &str,
        session: &mut carrier_memory::session::Session,
        manifest: &AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        brain: Option<&Arc<dyn Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
        resume: Option<&ResumeState>,
        input_overrides: Option<&serde_json::Map<String, Value>>,
    ) -> KernelResult<FlowOutcome> {
        let agent_name = self
            .registry
            .get(agent_id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| agent_id.to_string());

        // `input` is the template-context input (used by render/when/select);
        // it is also serialized into the flow_runs row on a fresh run. For a
        // sub-flow invoked via `flow_exec`/`map`, `input_overrides` carries the
        // rendered `with` params (e.g. `topic`) so `{{ input.topic }}` resolves.
        let mut input_map = serde_json::Map::new();
        input_map.insert(
            "user_message".into(),
            Value::String(user_message.to_string()),
        );
        input_map.insert(
            "user_id".into(),
            Value::String(sender_id.unwrap_or("").to_string()),
        );
        if let Some(overrides) = input_overrides {
            for (k, v) in overrides {
                input_map.insert(k.clone(), v.clone());
            }
        }
        let input: Value = Value::Object(input_map);

        // Record the run (history/audit; suspend/resume). On resume we reuse the
        // existing flow_runs row instead of creating a new one.
        let run_id = match resume {
            Some(r) => r.run_id.clone(),
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                let now = chrono::Utc::now().to_rfc3339();
                let input_json = input.to_string();
                let _ = self.memory.flow_runs().create(&FlowRunRow {
                    run_id: id.clone(),
                    session_id: session.id.0.to_string(),
                    agent_id: agent_id.to_string(),
                    sender_id: sender_id.unwrap_or("").to_string(),
                    flow_name: flow.name.clone(),
                    input: input_json,
                    completed_steps: "{}".into(),
                    waiting_at: None,
                    map_context: None,
                    status: "running".into(),
                    created_at: now.clone(),
                    updated_at: now,
                    expires_at: None,
                });
                id
            }
        };

        let layers = partition_flow_steps(&flow.steps)
            .map_err(|e| KernelError::Carrier(CarrierError::Internal(e)))?;

        info!(
            flow = %flow.name,
            layers = layers.len(),
            steps = flow.steps.len(),
            "Flow execution starting"
        );

        // On resume, pre-populate outputs with the already-completed steps and
        // seed `executed_order` in flow order so the final-selection fallback
        // works. On a fresh run both start empty.
        let mut outputs: HashMap<String, Value> = match resume {
            Some(r) => r.pre_outputs.clone(),
            None => HashMap::new(),
        };
        let mut executed_order: Vec<String> = if resume.is_some() {
            flow.steps
                .iter()
                .map(|s| s.id.clone())
                .filter(|id| outputs.contains_key(id))
                .collect()
        } else {
            Vec::new()
        };
        let mut total_usage = TokenUsage::default();
        let mut total_iterations = 0u32;

        let mut failed: Option<KernelError> = None;

        'outer: for (layer_idx, layer) in layers.iter().enumerate() {
            for step in layer {
                // `when` gate (skipped steps do not produce outputs).
                if let Some(when) = &step.when {
                    if !eval_when(when, &outputs, &input) {
                        info!(flow = %flow.name, step = %step.id, "flow step skipped (when=false)");
                        continue;
                    }
                }

                // Resume: skip steps already completed before the suspend point.
                if outputs.contains_key(&step.id) {
                    continue;
                }

                // Resume: the user's reply becomes the `user_input` step's
                // output `{ decision, text }`. No LLM call; do not append to
                // session here (the completion path or a subsequent `UserInput`
                // branch appends the reply turn). For a failure-pause resume
                // (waiting step is NOT a `user_input`), the reply instead
                // resolves to cancel (-> done) or retry (-> re-run this step).
                if let Some(r) = resume {
                    if step.id == r.waiting_step_id {
                        let is_user_input = step.kind.as_ref() == Some(&StepKind::UserInput);
                        if is_user_input {
                            let decision = if decide_cancel(&r.user_reply, &r.cancel_keywords) {
                                "cancel"
                            } else {
                                "proceed"
                            };
                            outputs.insert(
                                step.id.clone(),
                                serde_json::json!({ "decision": decision, "text": r.user_reply }),
                            );
                            executed_order.push(step.id.clone());
                            let completed =
                                serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
                            let _ = self
                                .memory
                                .flow_runs()
                                .update_status(&run_id, "running", &completed);
                            info!(
                                flow = %flow.name,
                                step = %step.id,
                                decision,
                                "flow resumed with user reply"
                            );
                            continue;
                        } else {
                            // Failure-pause resume: interpret the reply with the
                            // fixed failure-cancel keywords (a failed step has
                            // no per-step `cancel_keywords`).
                            let keywords: Vec<String> = FAILURE_CANCEL_KEYWORDS
                                .iter()
                                .map(|s| s.to_string())
                                .collect();
                            if decide_cancel(&r.user_reply, &keywords) {
                                let completed =
                                    serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
                                let _ = self.memory.flow_runs().update_status(
                                    &run_id,
                                    "cancelled",
                                    &completed,
                                );
                                info!(
                                    flow = %flow.name,
                                    step = %step.id,
                                    "flow cancelled by user at failed step"
                                );
                                return Ok(FlowOutcome::Completed {
                                    result: AgentLoopResult {
                                        response: format!("已取消流程「{}」。", flow.name),
                                        total_usage,
                                        iterations: total_iterations,
                                        silent: false,
                                        directives: Default::default(),
                                        plan: None,
                                    },
                                    final_value: None,
                                });
                            }
                            // Retry: fall through to dispatch to re-run this step.
                            info!(
                                flow = %flow.name,
                                step = %step.id,
                                "retrying failed step"
                            );
                        }
                    }
                }

                let kind = step.kind.as_ref().ok_or_else(|| {
                    KernelError::Carrier(CarrierError::Internal(format!(
                        "step '{}' has no kind",
                        step.id
                    )))
                })?;
                if !kind.is_executable() {
                    failed = Some(KernelError::Carrier(CarrierError::Internal(format!(
                        "step '{}' kind '{:?}' not yet supported in run_flow",
                        step.id, kind
                    ))));
                    break 'outer;
                }

                let step_prompt = step
                    .prompt
                    .as_deref()
                    .map(|p| render_template(p, &outputs, &input))
                    .unwrap_or_default();
                let step_user_msg = if step_prompt.is_empty() {
                    user_message.to_string()
                } else {
                    step_prompt.clone()
                };

                let dispatch: KernelResult<(Value, TokenUsage, u32)> = match kind {
                    StepKind::AgentLoop => {
                        self.run_step_agent_loop(
                            step,
                            &step_prompt,
                            &step_user_msg,
                            base_system_prompt,
                            &agent_name,
                            manifest,
                            tools,
                            brain,
                            kernel_handle.clone(),
                            sender_id,
                            owner_id,
                            channel_type,
                            &outputs,
                            &input,
                        )
                        .await
                    }
                    StepKind::Chat => {
                        self.run_step_chat(
                            step,
                            &step_user_msg,
                            base_system_prompt,
                            brain,
                            &outputs,
                            &input,
                        )
                        .await
                    }
                    StepKind::UserInput => {
                        // Suspend the flow: send the rendered prompt to the
                        // user as the (intermediate) reply, persist the run as
                        // `waiting`, and return `Suspended`.
                        let question = if step_prompt.is_empty() {
                            "请回复以继续。".to_string()
                        } else {
                            step_prompt.clone()
                        };
                        // Record the user turn + the question in the canonical
                        // session (mirrors the completion path's append below).
                        let new_messages =
                            vec![Message::user(user_message), Message::assistant(&question)];
                        session.messages.extend_from_slice(&new_messages);
                        let _ = self
                            .memory
                            .save_session_append_async(
                                session.id,
                                &agent_name,
                                &new_messages,
                                session.context_window_tokens,
                                session.label.as_deref(),
                                None,
                            )
                            .await;
                        // Compute the deadline and mark the run waiting.
                        let timeout_secs = step
                            .timeout_hours
                            .map(|h| (h * 3600.0) as u64)
                            .unwrap_or(self.config.user_input_timeout_secs);
                        let expires =
                            chrono::Utc::now() + chrono::Duration::seconds(timeout_secs as i64);
                        let _ = self.memory.flow_runs().set_waiting(
                            &run_id,
                            &step.id,
                            None,
                            Some(&expires.to_rfc3339()),
                        );
                        let completed =
                            serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
                        let _ = self
                            .memory
                            .flow_runs()
                            .update_status(&run_id, "waiting", &completed);
                        info!(
                            flow = %flow.name,
                            step = %step.id,
                            "flow suspended at user_input step"
                        );
                        return Ok(FlowOutcome::Suspended {
                            question,
                            total_usage,
                            iterations: total_iterations,
                        });
                    }
                    StepKind::FlowExec => {
                        self.exec_flow_step(
                            step,
                            agent_id,
                            manifest,
                            tools,
                            brain,
                            kernel_handle.clone(),
                            sender_id,
                            owner_id,
                            channel_type,
                            &outputs,
                            &input,
                            &agent_name,
                        )
                        .await
                    }
                    StepKind::Map => {
                        match self
                            .exec_map_step(
                                step,
                                agent_id,
                                manifest,
                                tools,
                                brain,
                                kernel_handle.clone(),
                                sender_id,
                                owner_id,
                                channel_type,
                                &outputs,
                                &input,
                                &agent_name,
                                base_system_prompt,
                                user_message,
                                resume,
                            )
                            .await
                        {
                            Ok(MapOutcome::Done(v, u, i)) => Ok((v, u, i)),
                            Ok(MapOutcome::Suspended {
                                question,
                                body_step_id,
                                map_context_json,
                                expires_at,
                                usage,
                                iterations,
                            }) => {
                                total_usage.input_tokens += usage.input_tokens;
                                total_usage.output_tokens += usage.output_tokens;
                                total_iterations += iterations;
                                // Mirror the UserInput suspend path, but the
                                // waiting step is the body user_input and we
                                // persist map iteration progress.
                                let new_messages = vec![
                                    Message::user(user_message),
                                    Message::assistant(&question),
                                ];
                                session.messages.extend_from_slice(&new_messages);
                                let _ = self
                                    .memory
                                    .save_session_append_async(
                                        session.id,
                                        &agent_name,
                                        &new_messages,
                                        session.context_window_tokens,
                                        session.label.as_deref(),
                                        None,
                                    )
                                    .await;
                                let _ = self.memory.flow_runs().set_waiting(
                                    &run_id,
                                    &body_step_id,
                                    Some(&map_context_json),
                                    expires_at.as_deref(),
                                );
                                let completed =
                                    serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
                                let _ = self
                                    .memory
                                    .flow_runs()
                                    .update_status(&run_id, "waiting", &completed);
                                info!(
                                    flow = %flow.name,
                                    step = %step.id,
                                    body_step = %body_step_id,
                                    "flow suspended inside map body"
                                );
                                return Ok(FlowOutcome::Suspended {
                                    question,
                                    total_usage,
                                    iterations: total_iterations,
                                });
                            }
                            // A map-step error (malformed `over`, a failing
                            // sub-flow in the batch, an interactive-map body
                            // error) routes through the dispatch Err branch ->
                            // failure-pause, consistent with other step kinds.
                            Err(e) => Err(e),
                        }
                    }
                    StepKind::Tool => {
                        self.run_step_tool(
                            step,
                            agent_id,
                            manifest,
                            brain,
                            kernel_handle.clone(),
                            sender_id,
                            owner_id,
                            channel_type,
                            &outputs,
                            &input,
                            &agent_name,
                        )
                        .await
                    }
                    StepKind::Unknown(_) => unreachable!(),
                };

                match dispatch {
                    Ok((out_val, usage, iters)) => {
                        total_usage.input_tokens += usage.input_tokens;
                        total_usage.output_tokens += usage.output_tokens;
                        total_iterations += iters;

                        outputs.insert(step.id.clone(), out_val);
                        executed_order.push(step.id.clone());
                        info!(
                            flow = %flow.name,
                            step = %step.id,
                            layer = layer_idx,
                            "flow step completed"
                        );
                        let completed =
                            serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
                        let _ = self
                            .memory
                            .flow_runs()
                            .update_status(&run_id, "running", &completed);
                    }
                    Err(e) => {
                        // Runtime error -> failure-pause: build a progress
                        // report, persist the run as `waiting` at this (failed)
                        // step, and suspend so the user can retry or cancel.
                        // Definition errors (no kind / not executable) are
                        // caught above and hard-fail via `failed`; this arm is
                        // only reached for dispatch errors.
                        let report = build_failure_report(flow, step, &e, &outputs);
                        warn!(
                            flow = %flow.name,
                            step = %step.id,
                            error = %e,
                            "flow paused at failed step"
                        );
                        let new_messages =
                            vec![Message::user(user_message), Message::assistant(&report)];
                        session.messages.extend_from_slice(&new_messages);
                        let _ = self
                            .memory
                            .save_session_append_async(
                                session.id,
                                &agent_name,
                                &new_messages,
                                session.context_window_tokens,
                                session.label.as_deref(),
                                None,
                            )
                            .await;
                        // Reuse the user_input timeout window; no per-step
                        // timeout_hours applies to failure-pause.
                        let timeout_secs = self.config.user_input_timeout_secs;
                        let expires =
                            chrono::Utc::now() + chrono::Duration::seconds(timeout_secs as i64);
                        let _ = self.memory.flow_runs().set_waiting(
                            &run_id,
                            &step.id,
                            None,
                            Some(&expires.to_rfc3339()),
                        );
                        let completed =
                            serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
                        let _ = self
                            .memory
                            .flow_runs()
                            .update_status(&run_id, "waiting", &completed);
                        return Ok(FlowOutcome::Suspended {
                            question: report,
                            total_usage,
                            iterations: total_iterations,
                        });
                    }
                }
            }
        }

        if let Some(e) = failed {
            let completed = serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
            let _ = self
                .memory
                .flow_runs()
                .update_status(&run_id, "failed", &completed);
            return Err(e);
        }

        // Final output: explicit `final` step (if executed), else last executed.
        let final_id = flow
            .final_step
            .as_deref()
            .filter(|id| outputs.contains_key(*id))
            .map(|s| s.to_string())
            .or_else(|| executed_order.last().cloned());
        let final_response = final_id
            .as_deref()
            .and_then(|id| outputs.get(id))
            .map(value_to_string)
            .unwrap_or_default();
        let final_value = final_id.as_deref().and_then(|id| outputs.get(id)).cloned();

        // Align with agent-loop silence contract: whole-text no-reply sentinels
        // become silent=true + empty channel response; session still records a
        // stable marker for prune/audit (same as end_turn).
        let silent = carrier_runtime::outbound::is_no_reply_sentinel(&final_response);
        let session_assistant = if silent {
            "[no reply needed]".to_string()
        } else {
            final_response.clone()
        };
        let channel_response = if silent {
            String::new()
        } else {
            final_response
        };

        // Record the user exchange in the canonical session (run_agent_loop
        // would have done this for the single-step path).
        let new_messages = vec![
            Message::user(user_message),
            Message::assistant(&session_assistant),
        ];
        session.messages.extend_from_slice(&new_messages);
        let _ = self
            .memory
            .save_session_append_async(
                session.id,
                &agent_name,
                &new_messages,
                session.context_window_tokens,
                session.label.as_deref(),
                None,
            )
            .await;

        let completed = serde_json::to_string(&outputs).unwrap_or_else(|_| "{}".into());
        let _ = self
            .memory
            .flow_runs()
            .update_status(&run_id, "completed", &completed);

        info!(
            flow = %flow.name,
            final_step = ?final_id,
            steps_completed = outputs.len(),
            total_iterations,
            silent,
            "Flow execution completed"
        );

        Ok(FlowOutcome::Completed {
            result: AgentLoopResult {
                response: channel_response,
                total_usage,
                iterations: total_iterations,
                silent,
                directives: carrier_types::message::ReplyDirectives {
                    silent,
                    ..Default::default()
                },
                plan: None,
            },
            final_value,
        })
    }
}
