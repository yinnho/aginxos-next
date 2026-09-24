//! Single-step runners: agent_loop, chat, tool.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tracing::warn;

use carrier_runtime::agent_loop::{
    run_agent_loop, TOOL_LONG_TIMEOUT_NAMES, TOOL_TIMEOUT_LONG_SECS, TOOL_TIMEOUT_SECS,
};
use carrier_runtime::kernel_handle::KernelHandle;
use carrier_runtime::llm_driver::{Brain, CompletionRequest};
use carrier_runtime::plugin::admin_store::is_admin;
use carrier_runtime::tool_context::ToolContext;
use carrier_runtime::tool_runner::execute_tool;
use carrier_types::agent::{AgentId, AgentManifest};
use carrier_types::error::CarrierError;
use carrier_types::flow::StepDef;
use carrier_types::message::{Message, TokenUsage};
use carrier_types::tool::ToolResult;

use super::template::{render_template, render_value, select_output};
use crate::error::{KernelError, KernelResult};
use crate::kernel::CarrierKernel;

impl CarrierKernel {
    /// Run a single `agent_loop` step in its own fresh session and return its
    /// output value + usage. Shared by `run_flow` (top-level) and
    /// `exec_body_steps` (map body). Resolves the driver + memory handle here
    /// so callers need not thread them.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_step_agent_loop(
        &self,
        step: &StepDef,
        step_prompt: &str,
        step_user_msg: &str,
        base_system_prompt: &str,
        agent_name: &str,
        manifest: &AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        brain: Option<&Arc<dyn Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
        outputs: &HashMap<String, Value>,
        input: &Value,
    ) -> KernelResult<(Value, TokenUsage, u32)> {
        let driver = self.resolve_driver(manifest)?;
        let memory_handle: Option<Arc<dyn carrier_runtime::memory_handle::MemoryHandle>> =
            Some(crate::handle::make_memory_handle(Arc::clone(&self.memory)));
        // base_system_prompt already carries the flow body (injected by
        // prepare_agent_context); add only the step directive.
        let step_system = format!(
            "{base_system_prompt}\n\n## 当前步骤: {}\n{step_prompt}",
            step.id,
        );
        let mut step_manifest = manifest.clone();
        step_manifest.model.system_prompt = step_system;
        let mut step_session = self
            .memory
            .create_session_async(agent_name.to_string())
            .await
            .map_err(KernelError::Carrier)?;
        let r = run_agent_loop(
            &step_manifest,
            step_user_msg,
            &mut step_session,
            &self.memory,
            driver,
            tools,
            kernel_handle,
            None,
            Some(&self.plugins.mcp_connections),
            manifest.workspace.as_deref(),
            None,
            Some(&self.coordination.hooks),
            None,
            Some(&self.coordination.process_manager),
            None,
            brain.cloned(),
            memory_handle,
            sender_id,
            owner_id,
            channel_type,
            Some(self.runtime.llm_concurrency_limit.clone()),
        )
        .await
        .map_err(KernelError::Carrier)?;
        let out_val = select_output(step, &r.response, outputs, input)?;
        Ok((out_val, r.total_usage, r.iterations))
    }

    /// Run a single `chat` step (one-shot LLM completion, no tools) and return
    /// its output value + usage. Shared by `run_flow` and `exec_body_steps`.
    pub(crate) async fn run_step_chat(
        &self,
        step: &StepDef,
        step_user_msg: &str,
        base_system_prompt: &str,
        brain: Option<&Arc<dyn Brain>>,
        outputs: &HashMap<String, Value>,
        input: &Value,
    ) -> KernelResult<(Value, TokenUsage, u32)> {
        let brain_ref = brain.ok_or_else(|| {
            KernelError::Carrier(CarrierError::Internal("chat step requires a brain".into()))
        })?;
        let task_text = step
            .task
            .as_deref()
            .map(|t| render_template(t, outputs, input))
            .unwrap_or_else(|| step_user_msg.to_string());
        let system = format!(
            "{base_system_prompt}\n\n## 当前步骤: {}\n{task_text}",
            step.id
        );
        let req = CompletionRequest {
            model: String::new(),
            messages: vec![Message::user(step_user_msg.to_string())],
            tools: Vec::new(),
            max_tokens: 4096,
            temperature: 0.7,
            system: Some(system),
            thinking: None,
            extra: Default::default(),
        };
        let resp = brain_ref
            .complete("fast", req)
            .await
            .map_err(KernelError::Carrier)?;
        let final_msg = resp.text();
        let out_val = select_output(step, &final_msg, outputs, input)?;
        Ok((out_val, resp.usage, 1))
    }

    /// Run a single `tool` step: resolve the tool by name, render `tool_args`
    /// templates, execute it via the shared `execute_tool` (with permission +
    /// admin-gate + timeout), and return its output. A tool error becomes an
    /// `Err` so `run_flow`'s `on_failure` can degrade. Shared by `run_flow`
    /// (top-level) and `exec_body_steps` (map body).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_step_tool(
        &self,
        step: &StepDef,
        agent_id: AgentId,
        manifest: &AgentManifest,
        brain: Option<&Arc<dyn Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
        outputs: &HashMap<String, Value>,
        input: &Value,
        agent_name: &str,
    ) -> KernelResult<(Value, TokenUsage, u32)> {
        let tool_name = step.tool_name.as_deref().ok_or_else(|| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "tool step '{}' missing `tool`/`tool_name`",
                step.id
            )))
        })?;
        let rendered_args = render_value(&step.tool_args, outputs, input);

        // Assemble the ToolContext (mirrors runtime/agent_loop/tool_use.rs).
        let memory_handle: Option<Arc<dyn carrier_runtime::memory_handle::MemoryHandle>> =
            Some(crate::handle::make_memory_handle(Arc::clone(&self.memory)));
        let caller_id = agent_id.to_string();
        let workspace_root: Option<&Path> = manifest.workspace.as_deref();
        let is_clone_admin =
            matches!((sender_id, workspace_root), (Some(sid), Some(root)) if is_admin(root, sid));
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
        let flow_shell_allow_owned: Vec<String> = manifest
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
        let tool_ctx = ToolContext {
            kernel: kernel_handle.as_ref(),
            memory: memory_handle.as_ref(),
            caller_agent_id: Some(&caller_id),
            mcp_connections: Some(&self.plugins.mcp_connections),
            allowed_env_vars: None,
            workspace_root,
            brain,
            exec_policy: manifest.exec_policy.as_ref(),
            cli_exec_config: manifest.cli_exec.as_ref(),
            process_manager: Some(&self.coordination.process_manager),
            sender_id,
            owner_id,
            home_dir: Some(self.config.home_dir.as_path()),
            agent_name: Some(agent_name),
            subagent_configs: if manifest.subagents.is_empty() {
                None
            } else {
                Some(&manifest.subagents)
            },
            channel_type,
            max_tool_level: manifest.max_tool_level,
            is_clone_admin,
            external_url: self.config.external_url.as_deref(),
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

        let tool_use_id = format!("flow:{}:{}", step.id, tool_name);
        let timeout_secs = if TOOL_LONG_TIMEOUT_NAMES.contains(&tool_name) {
            TOOL_TIMEOUT_LONG_SECS
        } else {
            TOOL_TIMEOUT_SECS
        };
        let result = match tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            execute_tool(&tool_use_id, tool_name, &rendered_args, &tool_ctx),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => {
                warn!(
                    flow = "tool_step",
                    step = %step.id,
                    tool = tool_name,
                    "tool step timed out after {}s",
                    timeout_secs
                );
                ToolResult {
                    tool_use_id,
                    content: format!("Tool '{}' timed out after {}s.", tool_name, timeout_secs),
                    is_error: true,
                }
            }
        };

        if result.is_error {
            return Err(KernelError::Carrier(CarrierError::Internal(format!(
                "tool step '{}' ('{}') failed: {}",
                step.id, tool_name, result.content
            ))));
        }
        // Tool content is often JSON; parse when possible, else keep raw string.
        // shell_exec wraps stdout as `Exit code / STDOUT / STDERR` — peel that
        // so structured fields from the tool remain available to the flow.
        let out_val = parse_tool_step_content(&result.content);
        // Tool execution uses no LLM tokens; count as one iteration.
        Ok((out_val, TokenUsage::default(), 1))
    }
}

/// Parse tool step stdout into JSON when possible (incl. shell_exec wrapper).
fn parse_tool_step_content(content: &str) -> Value {
    let trimmed = content.trim();
    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
        return v;
    }
    // shell_exec format
    if let Some(after) = content.split("STDOUT:").nth(1) {
        let body = after
            .split("STDERR:")
            .next()
            .unwrap_or(after)
            .trim()
            .trim_start_matches('\n');
        if let Ok(v) = serde_json::from_str::<Value>(body) {
            return v;
        }
        // first JSON object in body
        if let Some(start) = body.find('{') {
            if let Some(end) = body.rfind('}') {
                if end > start {
                    if let Ok(v) = serde_json::from_str::<Value>(&body[start..=end]) {
                        return v;
                    }
                }
            }
        }
    }
    Value::String(content.to_string())
}

#[cfg(test)]
mod parse_tool_content_tests {
    use super::parse_tool_step_content;
    use serde_json::json;

    #[test]
    fn parses_raw_json() {
        let v = parse_tool_step_content(r#"{"ok":true,"user_reply":"hi"}"#);
        assert_eq!(v["user_reply"], "hi");
    }

    #[test]
    fn parses_shell_exec_wrapper() {
        let content = r#"Exit code: 0

STDOUT:
{"ok": true, "user_reply": "done\nhttps://example.test/file", "result_id": "42"}
STDERR:
"#;
        let v = parse_tool_step_content(content);
        assert_eq!(v["result_id"], "42");
        assert!(v["user_reply"].as_str().unwrap().contains("done"));
    }

    #[test]
    fn falls_back_to_string() {
        let v = parse_tool_step_content("not json at all");
        assert_eq!(v, json!("not json at all"));
    }
}
