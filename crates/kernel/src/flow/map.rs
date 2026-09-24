//! Map steps: batch, interactive, and inline body execution.

use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::StreamExt;
use serde_json::Value;
use tracing::{info, warn};

use carrier_runtime::kernel_handle::KernelHandle;
use carrier_runtime::llm_driver::Brain;
use carrier_types::agent::{AgentId, AgentManifest};
use carrier_types::error::CarrierError;
use carrier_types::flow::{StepDef, StepKind};
use carrier_types::message::TokenUsage;

use super::dag::partition_flow_steps;
use super::template::{decide_cancel, eval_when, render_over_array, render_template};
use super::types::*;
use crate::error::{KernelError, KernelResult};
use crate::kernel::CarrierKernel;

impl CarrierKernel {
    /// Execute a `map` step. Dispatches on the body form:
    /// - `body: [steps]` set -> interactive map (stage E.2): iterate `over`,
    ///   running the inline body per element; a body `user_input` suspends with
    ///   `map_context`.
    /// - else (`flow`+`with`) -> batch map (stage E.1): invoke the named
    ///   sub-flow per element, collect results.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn exec_map_step(
        &self,
        step: &StepDef,
        agent_id: AgentId,
        manifest: &AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        brain: Option<&Arc<dyn Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
        outputs: &HashMap<String, Value>,
        input: &Value,
        agent_name: &str,
        base_system_prompt: &str,
        user_message: &str,
        resume: Option<&ResumeState>,
    ) -> KernelResult<MapOutcome> {
        if step.body.is_some() {
            self.exec_interactive_map(
                step,
                agent_id,
                manifest,
                tools,
                brain,
                kernel_handle,
                sender_id,
                owner_id,
                channel_type,
                outputs,
                input,
                agent_name,
                base_system_prompt,
                user_message,
                resume,
            )
            .await
        } else {
            let (v, u, i) = self
                .exec_map_batch(
                    step,
                    agent_id,
                    manifest,
                    tools,
                    brain,
                    kernel_handle,
                    sender_id,
                    owner_id,
                    channel_type,
                    outputs,
                    input,
                    agent_name,
                )
                .await?;
            Ok(MapOutcome::Done(v, u, i))
        }
    }

    /// Batch `map` (stage E.1): iterate `over`, running the named sub-flow once
    /// per element with the element bound to `as`, and collect final values.
    /// `step.parallel` (>1) runs sub-flows concurrently (order preserved);
    /// `None`/`1` is serial and short-circuits on the first error.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn exec_map_batch(
        &self,
        step: &StepDef,
        agent_id: AgentId,
        manifest: &AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        brain: Option<&Arc<dyn Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
        outputs: &HashMap<String, Value>,
        input: &Value,
        agent_name: &str,
    ) -> KernelResult<(Value, TokenUsage, u32)> {
        let over_tpl = step.over.as_deref().ok_or_else(|| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "map step '{}' missing `over`",
                step.id
            )))
        })?;
        let arr = render_over_array(step, over_tpl, outputs, input)?;
        let as_name = step.as_name.as_deref().unwrap_or("item").to_string();
        let parallel = step.parallel.unwrap_or(1).max(1) as usize;
        let mut collected: Vec<Value> = Vec::new();
        let mut total_usage = TokenUsage::default();
        let mut total_iters = 0u32;

        info!(
            flow = "map",
            step = %step.id,
            elements = arr.len(),
            parallel,
            "map step iterating"
        );
        if parallel <= 1 {
            // Serial: short-circuit on first error (preserves stage E.1 behavior).
            for element in arr {
                // Inject the element under `as_name` into a cloned outputs map so
                // bare `{{ as_name.field }}` templates resolve via resolve_path.
                let mut sub_outputs = outputs.clone();
                sub_outputs.insert(as_name.clone(), element);
                let (val, usage, iters) = self
                    .invoke_subflow(
                        step,
                        agent_id,
                        manifest,
                        tools,
                        brain,
                        kernel_handle.clone(),
                        sender_id,
                        owner_id,
                        channel_type,
                        &sub_outputs,
                        input,
                        agent_name,
                    )
                    .await?;
                collected.push(val);
                total_usage.input_tokens += usage.input_tokens;
                total_usage.output_tokens += usage.output_tokens;
                total_iters += iters;
            }
        } else {
            // Parallel: run up to `parallel` sub-flows concurrently while
            // yielding results in input order (`buffered` preserves order).
            // NOTE: concurrent siblings share the FLOW_DEPTH task_local Cell, so
            // the depth guard may over-count for deeply nested parallel maps --
            // acceptable (safe-fail at the limit, never data loss); each loop's
            // LLM calls are still bounded by the global llm_concurrency_limit.
            let futs = arr.into_iter().map(|element| {
                let mut sub_outputs = outputs.clone();
                sub_outputs.insert(as_name.clone(), element);
                // `Option<Arc<_>>` isn't Copy: clone once per element in the
                // sync closure, then move the owned clone into the async block.
                let kernel_handle = kernel_handle.clone();
                // `async move` owns `sub_outputs`/`kernel_handle` so the borrows
                // inside outlive the future; the `&self`/`&step`/... refs are Copy.
                async move {
                    self.invoke_subflow(
                        step,
                        agent_id,
                        manifest,
                        tools,
                        brain,
                        kernel_handle,
                        sender_id,
                        owner_id,
                        channel_type,
                        &sub_outputs,
                        input,
                        agent_name,
                    )
                    .await
                }
            });
            let results: Vec<KernelResult<(Value, TokenUsage, u32)>> = futures::stream::iter(futs)
                .buffered(parallel)
                .collect()
                .await;
            // First error (in order) aborts; later results are discarded.
            for r in results {
                let (val, usage, iters) = r?;
                collected.push(val);
                total_usage.input_tokens += usage.input_tokens;
                total_usage.output_tokens += usage.output_tokens;
                total_iters += iters;
            }
        }
        info!(flow = "map", step = %step.id, collected = collected.len(), "map step completed");
        Ok((Value::Array(collected), total_usage, total_iters))
    }

    /// Interactive `map` (stage E.2): iterate `over` serially, running the
    /// inline `body` steps per element. A body `user_input` suspends the whole
    /// flow, persisting `map_context` (iteration progress) so the next user
    /// message resumes from the same element. A body cancel terminates the map
    /// (collected so far is preserved).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn exec_interactive_map(
        &self,
        step: &StepDef,
        agent_id: AgentId,
        manifest: &AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        brain: Option<&Arc<dyn Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
        outputs: &HashMap<String, Value>,
        input: &Value,
        agent_name: &str,
        base_system_prompt: &str,
        user_message: &str,
        resume: Option<&ResumeState>,
    ) -> KernelResult<MapOutcome> {
        let body_steps = step.body.as_ref().ok_or_else(|| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "interactive map step '{}' missing `body`",
                step.id
            )))
        })?;
        // An interactive map can suspend per element (body `user_input`), so it
        // MUST run serially -- parallelism would break resume (which element is
        // waiting?). Reject `parallel > 1` up front.
        if step.parallel.unwrap_or(1) > 1 {
            return Err(KernelError::Carrier(CarrierError::Internal(format!(
                "interactive map step '{}' has body (can suspend) and must be serial (parallel<=1), got parallel={}",
                step.id,
                step.parallel.unwrap_or(1)
            ))));
        }
        let as_name = step.as_name.as_deref().unwrap_or("item").to_string();

        // Resume: reconstruct iteration state from map_context. Otherwise fresh.
        let (over, mut current_index, mut collected, mut body_completed, mut resuming_element) =
            if let Some(mc) = resume.and_then(|r| r.map_context.as_ref()) {
                if mc.map_step_id == step.id {
                    (
                        mc.over.clone(),
                        mc.current_index,
                        mc.collected.clone(),
                        mc.body_completed.clone(),
                        true,
                    )
                } else {
                    // map_context is for a different map step (defensive): fresh.
                    let over = render_over_array(
                        step,
                        step.over.as_deref().unwrap_or(""),
                        outputs,
                        input,
                    )?;
                    (over, 0usize, Vec::new(), HashMap::new(), false)
                }
            } else {
                let over_tpl = step.over.as_deref().ok_or_else(|| {
                    KernelError::Carrier(CarrierError::Internal(format!(
                        "map step '{}' missing `over`",
                        step.id
                    )))
                })?;
                let over = render_over_array(step, over_tpl, outputs, input)?;
                (over, 0usize, Vec::new(), HashMap::new(), false)
            };

        // When resuming, the body's waiting user_input step + the user's reply.
        let (body_step_id, user_reply, body_cancel_keywords) = if resuming_element {
            let bid = resume.unwrap().waiting_step_id.clone();
            let reply = resume.unwrap().user_reply.clone();
            let kws = body_steps
                .iter()
                .find(|s| s.id == bid)
                .map(|s| s.cancel_keywords.clone())
                .unwrap_or_default();
            (Some(bid), reply, kws)
        } else {
            (None, String::new(), Vec::new())
        };

        let mut acc_usage = TokenUsage::default();
        let mut acc_iters = 0u32;

        info!(
            flow = "map",
            step = %step.id,
            elements = over.len(),
            current_index,
            resuming = resuming_element,
            "interactive map step iterating"
        );

        while current_index < over.len() {
            let element = over[current_index].clone();
            let body_resume = if resuming_element {
                Some(BodyResume {
                    body_completed: body_completed.clone(),
                    waiting_step_id: body_step_id.clone().unwrap_or_default(),
                    user_reply: user_reply.clone(),
                    cancel_keywords: body_cancel_keywords.clone(),
                })
            } else {
                None
            };

            match self
                .exec_body_steps(
                    body_steps,
                    &as_name,
                    &element,
                    agent_id,
                    manifest,
                    tools,
                    brain,
                    kernel_handle.clone(),
                    sender_id,
                    owner_id,
                    channel_type,
                    outputs,
                    input,
                    agent_name,
                    base_system_prompt,
                    user_message,
                    body_resume.as_ref(),
                )
                .await
            {
                Ok(BodyOutcome::Done {
                    outputs: bo,
                    final_value,
                    usage,
                    iterations,
                }) => {
                    acc_usage.input_tokens += usage.input_tokens;
                    acc_usage.output_tokens += usage.output_tokens;
                    acc_iters += iterations;
                    // A resumed element means the user just replied: a cancel
                    // decision terminates the whole map (this element is NOT
                    // collected; prior elements are preserved).
                    if resuming_element
                        && body_step_id
                            .as_deref()
                            .and_then(|bid| bo.get(bid))
                            .and_then(|v| v.get("decision"))
                            .and_then(|v| v.as_str())
                            == Some("cancel")
                    {
                        info!(
                            flow = "map",
                            step = %step.id,
                            current_index,
                            "interactive map terminated by body cancel"
                        );
                        break;
                    }
                    collected.push(final_value.unwrap_or(Value::Null));
                    body_completed.clear();
                    current_index += 1;
                    resuming_element = false;
                }
                Ok(BodyOutcome::Suspended {
                    question,
                    step_id,
                    outputs: bo,
                    expires_at,
                    usage,
                    iterations,
                }) => {
                    acc_usage.input_tokens += usage.input_tokens;
                    acc_usage.output_tokens += usage.output_tokens;
                    acc_iters += iterations;
                    body_completed = bo;
                    let mc = MapContext {
                        map_step_id: step.id.clone(),
                        over: over.clone(),
                        current_index,
                        collected: collected.clone(),
                        body_completed: body_completed.clone(),
                        as_name: as_name.clone(),
                    };
                    let map_context_json = serde_json::to_string(&mc)
                        .map_err(|e| KernelError::Carrier(CarrierError::Internal(e.to_string())))?;
                    info!(
                        flow = "map",
                        step = %step.id,
                        body_step = %step_id,
                        current_index,
                        "interactive map suspended at body user_input"
                    );
                    return Ok(MapOutcome::Suspended {
                        question,
                        body_step_id: step_id,
                        map_context_json,
                        expires_at,
                        usage: acc_usage,
                        iterations: acc_iters,
                    });
                }
                Err(e) => {
                    // A body step failed at runtime. Pause at the MAP step
                    // (body_step_id = step.id) carrying map_context with
                    // current_index frozen at this element, so run_flow's
                    // resume treats it as a top-level failure-pause (retry via
                    // FAILURE_CANCEL_KEYWORDS / cancel). Retry re-enters this
                    // element's body (prior elements are preserved in
                    // `collected`); cancel terminates the flow.
                    warn!(
                        flow = "map",
                        step = %step.id,
                        current_index,
                        total = over.len(),
                        error = %e,
                        "interactive map paused at failed body step"
                    );
                    let mc = MapContext {
                        map_step_id: step.id.clone(),
                        over: over.clone(),
                        current_index,
                        collected: collected.clone(),
                        body_completed: HashMap::new(),
                        as_name: as_name.clone(),
                    };
                    let map_context_json = serde_json::to_string(&mc)
                        .map_err(|e| KernelError::Carrier(CarrierError::Internal(e.to_string())))?;
                    let timeout_secs = self.config.user_input_timeout_secs;
                    let expires =
                        chrono::Utc::now() + chrono::Duration::seconds(timeout_secs as i64);
                    let report = format!(
                        "流程的 map 步「{}」在第 {}/{} 个元素执行失败：{}\n\n回复「重试」重跑该元素（已完成元素会保留），或「取消」终止流程。",
                        step.id,
                        current_index + 1,
                        over.len(),
                        e
                    );
                    return Ok(MapOutcome::Suspended {
                        question: report,
                        body_step_id: step.id.clone(),
                        map_context_json,
                        expires_at: Some(expires.to_rfc3339()),
                        usage: acc_usage,
                        iterations: acc_iters,
                    });
                }
            }
        }

        info!(
            flow = "map",
            step = %step.id,
            collected = collected.len(),
            "interactive map step completed"
        );
        Ok(MapOutcome::Done(
            Value::Array(collected),
            acc_usage,
            acc_iters,
        ))
    }

    /// Execute one element's inline body steps (the `body: [steps]` of an
    /// interactive map). Mirrors `run_flow`'s DAG loop (layers, `when` gates,
    /// skip-completed, resume-inject) but does NOT persist to `flow_runs` or
    /// append to the canonical session -- the parent map arm owns those.
    /// Returns `Suspended` when a body `user_input` pauses. Body supports
    /// `agent_loop`/`chat`/`user_input`/`tool`/`flow_exec`/batch-`map` (a nested
    /// interactive map -- `map` with its own `body` -- is rejected; it would
    /// need nested suspend/map_context).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn exec_body_steps(
        &self,
        body_steps: &[StepDef],
        as_name: &str,
        element: &Value,
        agent_id: AgentId,
        manifest: &AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        brain: Option<&Arc<dyn Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
        parent_outputs: &HashMap<String, Value>,
        input: &Value,
        agent_name: &str,
        base_system_prompt: &str,
        user_message: &str,
        resume: Option<&BodyResume>,
    ) -> KernelResult<BodyOutcome> {
        // Body render context = parent outputs (for {{ parse_shots }} etc.)
        // overlaid with body-completed steps, then the current element under
        // `as_name` (so bare {{ ep.field }} resolves).
        let mut outputs = parent_outputs.clone();
        if let Some(r) = resume {
            for (k, v) in &r.body_completed {
                outputs.insert(k.clone(), v.clone());
            }
        }
        outputs.insert(as_name.to_string(), element.clone());

        let layers = partition_flow_steps(body_steps)
            .map_err(|e| KernelError::Carrier(CarrierError::Internal(e)))?;
        let mut executed_order: Vec<String> = if resume.is_some() {
            body_steps
                .iter()
                .map(|s| s.id.clone())
                .filter(|id| outputs.contains_key(id))
                .collect()
        } else {
            Vec::new()
        };
        let mut total_usage = TokenUsage::default();
        let mut total_iterations = 0u32;

        for layer in &layers {
            for step in layer {
                // `when` gate.
                if let Some(when) = &step.when {
                    if !eval_when(when, &outputs, input) {
                        continue;
                    }
                }
                // Skip already-completed body steps (resume).
                if outputs.contains_key(&step.id) {
                    continue;
                }
                // Resume-inject: the user's reply becomes the waiting
                // user_input step's output { decision, text }. No persistence
                // here (the parent map arm owns flow_runs).
                if let Some(r) = resume {
                    if step.id == r.waiting_step_id {
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
                        continue;
                    }
                }

                let kind = step.kind.as_ref().ok_or_else(|| {
                    KernelError::Carrier(CarrierError::Internal(format!(
                        "body step '{}' has no kind",
                        step.id
                    )))
                })?;
                if !matches!(
                    kind,
                    StepKind::AgentLoop
                        | StepKind::Chat
                        | StepKind::UserInput
                        | StepKind::Tool
                        | StepKind::FlowExec
                        | StepKind::Map
                ) {
                    return Err(KernelError::Carrier(CarrierError::Internal(format!(
                        "body step '{}' kind '{:?}' not yet supported in map body",
                        step.id, kind
                    ))));
                }

                let step_prompt = step
                    .prompt
                    .as_deref()
                    .map(|p| render_template(p, &outputs, input))
                    .unwrap_or_default();
                let step_user_msg = if step_prompt.is_empty() {
                    user_message.to_string()
                } else {
                    step_prompt.clone()
                };

                if *kind == StepKind::UserInput {
                    let question = if step_prompt.is_empty() {
                        "请回复以继续。".to_string()
                    } else {
                        step_prompt.clone()
                    };
                    let timeout_secs = step
                        .timeout_hours
                        .map(|h| (h * 3600.0) as u64)
                        .unwrap_or(self.config.user_input_timeout_secs);
                    let expires =
                        chrono::Utc::now() + chrono::Duration::seconds(timeout_secs as i64);
                    return Ok(BodyOutcome::Suspended {
                        question,
                        step_id: step.id.clone(),
                        outputs,
                        expires_at: Some(expires.to_rfc3339()),
                        usage: total_usage,
                        iterations: total_iterations,
                    });
                }

                let (v, u, i) = if *kind == StepKind::AgentLoop {
                    self.run_step_agent_loop(
                        step,
                        &step_prompt,
                        &step_user_msg,
                        base_system_prompt,
                        agent_name,
                        manifest,
                        tools,
                        brain,
                        kernel_handle.clone(),
                        sender_id,
                        owner_id,
                        channel_type,
                        &outputs,
                        input,
                    )
                    .await?
                } else if *kind == StepKind::Tool {
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
                        input,
                        agent_name,
                    )
                    .await?
                } else if *kind == StepKind::FlowExec {
                    // Sub-flow invoked from a body step. invoke_subflow
                    // pre-rejects sub-flows containing user_input, so this
                    // cannot suspend -- safe inside the body loop.
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
                        input,
                        agent_name,
                    )
                    .await?
                } else if *kind == StepKind::Map {
                    // Batch map (flow+with) is allowed in a body. A nested
                    // interactive map (body-inside-body) would need nested
                    // suspend/map_context -- not yet supported.
                    if step.body.is_some() {
                        return Err(KernelError::Carrier(CarrierError::Internal(format!(
                            "body step '{}' is an interactive map (has body) -- nested interactive map not supported in map body",
                            step.id
                        ))));
                    }
                    self.exec_map_batch(
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
                        input,
                        agent_name,
                    )
                    .await?
                } else {
                    // Chat
                    self.run_step_chat(
                        step,
                        &step_user_msg,
                        base_system_prompt,
                        brain,
                        &outputs,
                        input,
                    )
                    .await?
                };
                total_usage.input_tokens += u.input_tokens;
                total_usage.output_tokens += u.output_tokens;
                total_iterations += i;
                outputs.insert(step.id.clone(), v);
                executed_order.push(step.id.clone());
            }
        }

        let final_value = executed_order
            .last()
            .and_then(|id| outputs.get(id))
            .cloned();
        Ok(BodyOutcome::Done {
            outputs,
            final_value,
            usage: total_usage,
            iterations: total_iterations,
        })
    }
}
