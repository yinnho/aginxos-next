//! Sub-flow invocation (`flow_exec` / map flow form).

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tracing::info;

use carrier_runtime::kernel_handle::KernelHandle;
use carrier_runtime::llm_driver::Brain;
use carrier_types::agent::{AgentId, AgentManifest};
use carrier_types::error::CarrierError;
use carrier_types::flow::StepDef;
use carrier_types::message::TokenUsage;

use super::template::{flow_contains_user_input, render_template};
use super::types::*;
use super::{FLOW_DEPTH, MAX_FLOW_DEPTH};
use crate::error::{KernelError, KernelResult};
use crate::kernel::CarrierKernel;

impl CarrierKernel {
    /// Execute a `flow_exec` step: invoke the named sub-flow once and return
    /// its final value as this step's output.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn exec_flow_step(
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
            outputs,
            input,
            agent_name,
        )
        .await
    }

    /// Shared sub-flow invocation for `flow_exec` and `map`. Loads the sub-flow
    /// by name, builds its `input` from `step.with` (rendered) plus the parent
    /// `user_message`/`user_id` passthrough, runs it in a fresh session under a
    /// depth scope, and returns its final value + usage.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn invoke_subflow(
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
        // Depth guard against runaway recursion (flow_exec -> flow_exec -> ...).
        let current = FLOW_DEPTH.try_with(|d| d.get()).unwrap_or(0);
        if current >= MAX_FLOW_DEPTH {
            return Err(KernelError::Carrier(CarrierError::Internal(format!(
                "flow_exec depth limit ({}) exceeded at step '{}'",
                MAX_FLOW_DEPTH, step.id
            ))));
        }

        let flow_name = step.flow.as_deref().ok_or_else(|| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "step '{}' (flow_exec/map) missing `flow`",
                step.id
            )))
        })?;
        let workspace = manifest.workspace.as_deref().ok_or_else(|| {
            KernelError::Carrier(CarrierError::Internal(
                "flow_exec requires a manifest workspace".into(),
            ))
        })?;
        let sub_match =
            crate::prompt_sources::load_flow_by_name(workspace, flow_name).ok_or_else(|| {
                KernelError::Carrier(CarrierError::Internal(format!(
                    "flow_exec step '{}' references unknown flow '{}'",
                    step.id, flow_name
                )))
            })?;
        let sub_flow = &sub_match.flow_def;

        // Sub-flows invoked via flow_exec cannot suspend (no resume stack).
        // This includes interactive map bodies (which contain user_input).
        if flow_contains_user_input(sub_flow) {
            return Err(KernelError::Carrier(CarrierError::Internal(format!(
                "flow_exec sub-flow '{}' contains a user_input step (not allowed)",
                flow_name
            ))));
        }

        // Render each `with` value as a template against the current outputs
        // (for `map`, the element is already injected under `as_name`). These
        // become the sub-flow's `input.<key>` via `input_overrides`.
        let mut rendered_with: serde_json::Map<String, Value> = serde_json::Map::new();
        for (k, v) in &step.with {
            let tpl = v.as_str().unwrap_or("");
            let rendered = render_template(tpl, outputs, input);
            rendered_with.insert(k.clone(), Value::String(rendered));
        }
        // The root user_message is passed through (single-step sub-flows use it
        // as the topic via `{{ input.user_message }}`).
        let sub_user_msg = input
            .get("user_message")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Fresh sub-session (independent of the parent's); sub-flow rows are
        // created for audit by run_flow itself.
        let mut sub_session = self
            .memory
            .create_session_async(agent_name.to_string())
            .await
            .map_err(KernelError::Carrier)?;
        let sub_base_prompt = format!(
            "{}\n\n## 子 flow: {}\n{}",
            manifest.model.system_prompt, sub_flow.name, sub_flow.body
        );

        info!(flow = %sub_flow.name, parent_step = %step.id, "flow_exec invoking sub-flow");
        let outcome = FLOW_DEPTH
            .scope(Cell::new(current + 1), async {
                // `Box::pin` breaks the recursive async future's infinite size
                // (run_flow -> dispatch -> invoke_subflow -> run_flow ...).
                Box::pin(self.run_flow(
                    agent_id,
                    sub_flow,
                    &sub_base_prompt,
                    &sub_user_msg,
                    &mut sub_session,
                    manifest,
                    tools,
                    brain,
                    kernel_handle,
                    sender_id,
                    owner_id,
                    channel_type,
                    None,
                    Some(&rendered_with),
                ))
                .await
            })
            .await?;

        match outcome {
            FlowOutcome::Completed {
                result,
                final_value,
            } => {
                let val = final_value.unwrap_or_else(|| Value::String(result.response.clone()));
                Ok((val, result.total_usage, result.iterations))
            }
            FlowOutcome::Suspended { .. } => {
                Err(KernelError::Carrier(CarrierError::Internal(format!(
                    "flow_exec sub-flow '{}' suspended unexpectedly (should be pre-checked)",
                    flow_name
                ))))
            }
        }
    }
}
