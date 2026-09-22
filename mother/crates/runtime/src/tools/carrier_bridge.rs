//! carrier 桥 — 内核耦合工具的外置实现桥（M33 D3 批3）。
//!
//! 实现已整体搬到 `aginx-carrier tool <name>` 机读面（单真源）：
//! schedule_create/list/delete + cron_create/list/cancel 来自被删的
//! tools/scheduling.rs，agent_send/spawn/list/kill/restart 来自被删的
//! tools/agent_mgmt.rs，location_get/system_time 来自被删的 tools/misc.rs。
//! 本模块只留：
//! - definitions()：与被删模块**逐字节相同**的 ToolDefinition（名字/schema/
//!   description 不动——flow `tools:` 加载期冻结、CORE_TOOL_NAMES、教学文本
//!   全部依赖这批名字）。
//! - execute()：spawn `aginx-carrier tool <name>`，stdin 喂入参 JSON（含
//!   `_ctx` 身份 + 递归深度），stdout 收 D1 信封
//!   （{"ok":true,"data":…} / {"ok":false,"error":…}）。
//!
//! 与 web/agf 桥（M31/M32）的两点刻意差异：
//! - **不 env_clear**。子进程是 carrier CLI 自己——内核的 CLI 面，与 runtime
//!   同信任域（不是外置包代码）。agent_send 的目标轮要打 brain、开 carrier.db，
//!   env（brain key、PATH、HOME）必须原样继承；秘密只会流回内核自己的手里。
//! - **递归护栏跨进程续传**。agent_send 先在本进程 check_call_depth（原
//!   tool_agent_send 同位），再把 current+1 经 `_ctx.depth` 传给子进程；子进程
//!   以该深度 scope 目标轮（见 carrier tool_cmd.rs），连环 send 不会因换进程
//!   而逃逸 MAX_AGENT_CALL_DEPTH。
//!
//! 语义：定义恒广播；执行在 aginx-carrier 不可用时干净报错（包在场门执行，
//! 不门广告——flow 冻结不因缺 CLI 漂移；与 web/agf 桥同款）。

use super::ToolModule;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;

pub struct CarrierBridge;

/// 桥承载的全部工具名（与 carrier tool_cmd::TOOL_NAMES 一一对应）。
pub const BRIDGE_TOOL_NAMES: &[&str] = &[
    "schedule_create",
    "schedule_list",
    "schedule_delete",
    "cron_create",
    "cron_list",
    "cron_cancel",
    "agent_send",
    "agent_spawn",
    "agent_list",
    "agent_kill",
    "agent_restart",
    "location_get",
    "system_time",
];

#[async_trait]
impl ToolModule for CarrierBridge {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            // --- schedule_*（自 tools/scheduling.rs 逐字节搬来） ---
            ToolDefinition {
                name: "schedule_create".to_string(),
                description: "Schedule a recurring task using natural language or cron syntax. Examples: 'every 5 minutes', 'daily at 9am', 'weekdays at 6pm', '0 */5 * * *'.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "description": { "type": "string", "description": "What this schedule does (e.g., 'Check for new emails')" },
                        "schedule": { "type": "string", "description": "Natural language or cron expression (e.g., 'every 5 minutes', 'daily at 9am', '0 */5 * * *')" },
                        "agent": { "type": "string", "description": "Agent name or ID to run this task (optional, defaults to self)" }
                    },
                    "required": ["description", "schedule"]
                }),
            },
            ToolDefinition {
                name: "schedule_list".to_string(),
                description: "List all scheduled tasks with their IDs, descriptions, schedules, and next run times.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "schedule_delete".to_string(),
                description: "Remove a scheduled task by its ID.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "The schedule ID to remove" }
                    },
                    "required": ["id"]
                }),
            },
            // --- cron_*（自 tools/scheduling.rs 逐字节搬来） ---
            ToolDefinition {
                name: "cron_create".to_string(),
                description: "Create a scheduled/cron job. Supports one-shot (at), recurring (every N seconds), and cron expressions. Max 50 jobs per agent.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "Job name (max 128 chars, alphanumeric + spaces/hyphens/underscores)" },
                        "schedule": {
                            "type": "object",
                            "description": "Schedule: {\"kind\":\"at\",\"in_secs\":120} (RECOMMENDED for one-shot: relative seconds from now, server computes the absolute time - never do timezone math yourself) or {\"kind\":\"at\",\"at\":\"2025-01-01T00:00:00Z\"} (absolute RFC3339, only when the exact wall-clock time matters) or {\"kind\":\"every\",\"every_secs\":300} or {\"kind\":\"cron\",\"expr\":\"0 8 * * *\"}. Cron expressions default to server local timezone; pass {\"kind\":\"cron\",\"expr\":\"...\",\"tz\":\"UTC\"} or any IANA tz (e.g. \"Asia/Shanghai\") to override."
                        },
                        "action": {
                            "type": "object",
                            "description": "Action: {\"kind\":\"system_event\",\"text\":\"...\"} or {\"kind\":\"agent_turn\",\"message\":\"...\",\"timeout_secs\":300,\"active_flow\":\"<flow_name>\",\"session_label\":\"<label>\"} or {\"kind\":\"push\",\"channel\":\"weixin-oa\",\"bot_id\":\"<app_id>\",\"payload\":{\"text\":\"...\"},\"target\":\"admins|followers|<openid>\"} (scheduled fixed push, no LLM) or {\"kind\":\"follower_report\",\"channel\":\"weixin-oa\",\"bot_id\":\"<app_id>\"} (follower-growth digest to admins since previous fire, no LLM). active_flow (optional) pins the flow to run, bypassing the LLM classifier. session_label (optional, for chained pipelines) runs the turn in its own isolated session so user chat can't interleave — pass the SAME label for every step of one pipeline."
                        },
                        "delivery": {
                            "type": "object",
                            "description": "Delivery target: {\"kind\":\"none\"} or {\"kind\":\"channel\",\"channel\":\"telegram\"} or {\"kind\":\"last_channel\"}"
                        },
                        "one_shot": { "type": "boolean", "description": "If true, auto-delete after execution. Default: false" },
                        "chain": {
                            "type": "object",
                            "description": "Chained-pipeline identity (RECOMMENDED for pipeline steps): {\"chain_id\":\"<pipeline_id>\",\"step\":N,\"total_steps\":M}. chain_id = the pipeline id (same as session_label/output dir), step = 1-based current step, total_steps = chain length. The system alerts if a non-tail step (step < total_steps) completes without scheduling its successor — pass this on EVERY step of a chained pipeline, including the first one you create for step 1; the tail step (step == total_steps) legitimately creates no successor."
                        }
                    },
                    "required": ["name", "schedule", "action"]
                }),
            },
            ToolDefinition {
                name: "cron_list".to_string(),
                description: "List all scheduled/cron jobs for the current agent.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "cron_cancel".to_string(),
                description: "Cancel a scheduled/cron job by its ID.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "The UUID of the cron job to cancel" }
                    },
                    "required": ["job_id"]
                }),
            },
            // --- agent_*（自 tools/agent_mgmt.rs 逐字节搬来） ---
            ToolDefinition {
                name: "agent_send".to_string(),
                description: "Send a message to another agent and receive their response. Accepts UUID or agent name. Use agent_find first to discover agents.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The target agent's UUID or name" },
                        "message": { "type": "string", "description": "The message to send to the agent" }
                    },
                    "required": ["agent_id", "message"]
                }),
            },
            ToolDefinition {
                name: "agent_spawn".to_string(),
                description: "Spawn a new agent from a TOML manifest. Returns the new agent's ID and name.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "manifest_toml": {
                            "type": "string",
                            "description": "The agent manifest in TOML format (must include name, module, [model], and [capabilities])"
                        }
                    },
                    "required": ["manifest_toml"]
                }),
            },
            ToolDefinition {
                name: "agent_list".to_string(),
                description: "List all currently running agents with their IDs, names, states, and models.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "agent_kill".to_string(),
                description: "Kill (terminate) another agent by its ID.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The agent's UUID to kill" }
                    },
                    "required": ["agent_id"]
                }),
            },
            ToolDefinition {
                name: "agent_restart".to_string(),
                description: "Restart another agent by its ID. Cancels any running task and resets state to Running. Useful after modifying an agent's configuration to apply changes.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "agent_id": { "type": "string", "description": "The target agent's UUID or name" }
                    },
                    "required": ["agent_id"]
                }),
            },
            // --- 杂项（自 tools/misc.rs 逐字节搬来） ---
            ToolDefinition {
                name: "location_get".to_string(),
                description: "Get the current geographical location based on IP address."
                    .to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDefinition {
                name: "system_time".to_string(),
                description: "Get the current date, time, timezone, and Unix epoch.".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
        ]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        if !BRIDGE_TOOL_NAMES.contains(&name) {
            return None;
        }
        // 原位护栏（被删 tool_agent_send 的 check_call_depth 同位同语义）。
        if name == "agent_send" {
            if let Err(e) = super::check_call_depth() {
                return Some(Err(e));
            }
        }
        Some(run_carrier_tool(name, input, ctx).await)
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        // 与三个被删模块的原 permission_level 逐条对齐。
        match tool_name {
            // misc.rs
            "location_get" | "system_time" => PermissionLevel::ReadOnly,
            // scheduling.rs
            "schedule_list" | "cron_list" => PermissionLevel::None,
            "schedule_create" | "schedule_delete" | "cron_create" | "cron_cancel" => {
                PermissionLevel::Write
            }
            // agent_mgmt.rs
            "agent_list" => PermissionLevel::None,
            "agent_send" | "agent_spawn" | "agent_restart" => PermissionLevel::Execute,
            "agent_kill" => PermissionLevel::Dangerous,
            _ => PermissionLevel::Dangerous,
        }
    }
}

// ---------------------------------------------------------------------------
// spawn + 信封解包（web/agf 桥同款；env 策略见模块头"刻意差异"）
// ---------------------------------------------------------------------------

/// Spawn `aginx-carrier tool <name>`（stdin=入参 JSON 含 `_ctx`，stdout=D1 信封）。
async fn run_carrier_tool(name: &str, input: &Value, ctx: &ToolContext<'_>) -> CarrierResult<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    // 入参克隆 + `_ctx` 注入：身份（归属/发送者/化身名）+ 递归深度。
    // agent_send 传 current+1（子进程以其 scope 目标轮）；其余工具传 current
    // （不递增——只有 send 展开一层代理调用）。
    let mut payload = input.clone();
    {
        let obj = payload
            .as_object_mut()
            .ok_or_else(|| CarrierError::InvalidInput("tool input must be a JSON object".into()))?;
        let c = obj
            .entry("_ctx")
            .or_insert_with(|| serde_json::json!({}));
        let c = c.as_object_mut().unwrap();
        if let Some(ca) = ctx.caller_agent_id {
            c.insert("caller_agent_id".into(), serde_json::json!(ca));
        }
        if let Some(sid) = ctx.sender_id {
            c.insert("sender_id".into(), serde_json::json!(sid));
        }
        if let Some(oid) = ctx.owner_id {
            c.insert("owner_id".into(), serde_json::json!(oid));
        }
        if let Some(an) = ctx.agent_name {
            c.insert("agent_name".into(), serde_json::json!(an));
        }
        let depth = crate::tool_runner::current_agent_call_depth();
        c.insert(
            "depth".into(),
            serde_json::json!(if name == "agent_send" { depth + 1 } else { depth }),
        );
    }

    // 不 sandbox_command：见模块头——子进程是内核自己的 CLI 面，同信任域，
    // env 原样继承（brain key / PATH / HOME）。kill_on_drop：取消不留孤儿。
    let mut cmd = tokio::process::Command::new("aginx-carrier");
    cmd.arg("tool").arg(name);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        CarrierError::Internal(format!(
            "aginx-carrier CLI not available ({e}) — kernel-coupled tools (schedule/cron/agent_*) \
             execute through it. Check PATH (/var/bin)."
        ))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort write; the CLI reads stdin to EOF before executing.
        let bytes = serde_json::to_vec(&payload).unwrap_or_default();
        let _ = stdin.write_all(&bytes).await;
        let _ = stdin.shutdown().await;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| CarrierError::Internal(format!("aginx-carrier tool {name} subprocess failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let envelope: Value = serde_json::from_str(&stdout).map_err(|_| {
        let tail = if stdout.is_empty() { stderr } else { stdout };
        let preview = crate::str_utils::safe_truncate_str(&tail, 300);
        CarrierError::Internal(format!(
            "aginx-carrier tool {name} returned a non-JSON response (exit {:?}): {preview}",
            output.status.code()
        ))
    })?;

    if envelope["ok"].as_bool().unwrap_or(false) {
        envelope["data"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                CarrierError::Serialization(
                    "aginx-carrier envelope missing string data field".to_string(),
                )
            })
    } else {
        let msg = envelope["error"]
            .as_str()
            .or_else(|| envelope["error"]["message"].as_str())
            .unwrap_or("unknown aginx-carrier tool error");
        Err(CarrierError::Internal(msg.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_match_bridge_names() {
        let bridge = CarrierBridge;
        let defs = bridge.definitions();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        let mut expected = BRIDGE_TOOL_NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn permission_levels_match_legacy() {
        let bridge = CarrierBridge;
        // misc.rs
        assert!(matches!(bridge.permission_level("location_get"), PermissionLevel::ReadOnly));
        assert!(matches!(bridge.permission_level("system_time"), PermissionLevel::ReadOnly));
        // scheduling.rs
        assert!(matches!(bridge.permission_level("schedule_list"), PermissionLevel::None));
        assert!(matches!(bridge.permission_level("cron_list"), PermissionLevel::None));
        assert!(matches!(bridge.permission_level("schedule_create"), PermissionLevel::Write));
        assert!(matches!(bridge.permission_level("cron_cancel"), PermissionLevel::Write));
        // agent_mgmt.rs
        assert!(matches!(bridge.permission_level("agent_list"), PermissionLevel::None));
        assert!(matches!(bridge.permission_level("agent_send"), PermissionLevel::Execute));
        assert!(matches!(bridge.permission_level("agent_spawn"), PermissionLevel::Execute));
        assert!(matches!(bridge.permission_level("agent_restart"), PermissionLevel::Execute));
        assert!(matches!(bridge.permission_level("agent_kill"), PermissionLevel::Dangerous));
    }

    #[test]
    fn bridge_covers_exactly_thirteen_tools() {
        assert_eq!(BRIDGE_TOOL_NAMES.len(), 13);
    }

    #[tokio::test]
    async fn unknown_name_is_none() {
        let bridge = CarrierBridge;
        let ctx = crate::tool_context::ToolContext {
            kernel: None,
            memory: None,
            caller_agent_id: None,
            mcp_connections: None,
            allowed_env_vars: None,
            workspace_root: None,
            brain: None,
            exec_policy: None,
            cli_exec_config: None,
            process_manager: None,
            sender_id: None,
            owner_id: None,
            home_dir: None,
            agent_name: None,
            subagent_configs: None,
            channel_type: None,
            max_tool_level: carrier_types::tool::PermissionLevel::Write,
            is_clone_admin: false,
            external_url: None,
            flow_elevated_tools: None,
            flow_shell_allow: None,
            flow_deny_tools: None,
            flow_allowed_tools: None,
        };
        assert!(bridge.execute("nope", &serde_json::json!({}), &ctx).await.is_none());
    }
}
