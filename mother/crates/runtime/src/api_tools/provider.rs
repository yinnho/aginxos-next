//! DeclarativeApiModule — api_tools.toml 工具的内核面（M34b 起纯桥）。
//!
//! 执行链已整体搬到 `aginx-carrier api call`（单真源，M34a）：resolve 链
//! （参数先经另一 api 工具预解）、ctx 注入（sender_id→openid，channel 门
//! 与 only_if_absent 让位）、JSON body、URL 模板、HMAC-SHA256 签一发一、
//! error_check、extract tiers。本模块只留：
//!
//! - definitions()：ToolDefinition 逐字节不动（名字/schema/description——
//!   flow `tools:` 加载期冻结、CORE_TOOL_NAMES、per-agent 可见性判断全部
//!   依赖这批名字）。
//! - execute()：spawn `aginx-carrier api call <name> --json`，stdin 喂入参
//!   JSON（含 `_ctx{sender_id, channel_type}`），stdout 收 D1 信封。
//!   toml 装载面 = 全局 + 本化身工作区（后者同名覆盖前者——与
//!   messaging.rs 的 per-agent 可见性语义一致）。
//!
//! 与 carrier_bridge（M33）同款纪律：子进程是内核自己的 CLI 面、同信任域，
//! 不 env_clear（.env 里的 API 键靠 HOME 继承）；包在场门执行不门广告——
//! CLI 不可用时干净报错，flow 冻结不因缺 CLI 漂移。

use crate::tool_context::ToolContext;
use crate::tools::ToolModule;
use async_trait::async_trait;
use carrier_types::api_tool::ApiToolDef;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;

pub struct DeclarativeApiModule {
    tools: Vec<ApiToolDef>,
}

impl DeclarativeApiModule {
    pub fn new(tools: Vec<ApiToolDef>) -> Self {
        Self { tools }
    }

    fn find_config(&self, name: &str) -> Option<&ApiToolDef> {
        self.tools.iter().find(|t| t.name == name)
    }
}

#[async_trait]
impl ToolModule for DeclarativeApiModule {
    fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|t| ToolDefinition {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: serde_json::from_str(&t.input_schema_json())
                    .unwrap_or(Value::Object(serde_json::Map::new())),
            })
            .collect()
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        self.find_config(name)?; // 只执行广告过的；未知名交给别的模块
        Some(run_api_call(name, input, ctx).await)
    }

    fn permission_level(&self, _tool_name: &str) -> PermissionLevel {
        PermissionLevel::ReadOnly
    }
}

// ---------------------------------------------------------------------------
// spawn + 信封解包（carrier_bridge 同款）
// ---------------------------------------------------------------------------

/// Spawn `aginx-carrier api call <name> --json`（stdin=入参 JSON 含
/// `_ctx`，stdout=D1 信封）。
async fn run_api_call(name: &str, input: &Value, ctx: &ToolContext<'_>) -> CarrierResult<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    // 入参克隆 + `_ctx` 注入：provider 执行链只吃身份两元
    // （sender_id→openid 注入、channel 门）。
    let mut payload = input.clone();
    {
        let obj = payload
            .as_object_mut()
            .ok_or_else(|| CarrierError::InvalidInput("tool input must be a JSON object".into()))?;
        let c = obj
            .entry("_ctx")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(cm) = c.as_object_mut() {
            if let Some(sid) = ctx.sender_id {
                cm.insert("sender_id".into(), serde_json::json!(sid));
            }
            if let Some(ch) = ctx.channel_type {
                cm.insert("channel_type".into(), serde_json::json!(ch));
            }
        }
    }

    // toml 装载面：全局 + 本化身工作区（后者同名覆盖——per-agent 语义）。
    // 不 sandbox_command：子进程是内核自己的 CLI 面，同信任域，env 原样
    // 继承（.env 的 API 键、PATH、HOME）。kill_on_drop：取消不留孤儿。
    let mut cmd = tokio::process::Command::new("aginx-carrier");
    cmd.arg("api").arg("call").arg(name).arg("--json")
        .arg("--toml")
        .arg(carrier_types::config::home_dir().join("api_tools.toml"));
    if let Some(ws) = ctx.workspace_root {
        cmd.arg("--toml").arg(ws.join("api_tools.toml"));
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        CarrierError::Internal(format!(
            "aginx-carrier CLI not available ({e}) — api_tools execute through \
             `aginx-carrier api call`. Check PATH (/var/bin)."
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
        .map_err(|e| CarrierError::Internal(format!("aginx-carrier api call {name} subprocess failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let envelope: Value = serde_json::from_str(&stdout).map_err(|_| {
        let tail = if stdout.is_empty() { stderr } else { stdout };
        let preview = crate::str_utils::safe_truncate_str(&tail, 300);
        CarrierError::Internal(format!(
            "aginx-carrier api call {name} returned a non-JSON response (exit {:?}): {preview}",
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
            .unwrap_or("unknown aginx-carrier api error");
        Err(CarrierError::Internal(msg.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_tools(toml_str: &str) -> Vec<ApiToolDef> {
        toml::from_str::<carrier_types::api_tool::ApiToolsConfig>(toml_str)
            .unwrap()
            .tool
    }

    /// definitions() 名字/schema 逐字节保持（flow 冻结依赖）。
    #[test]
    fn definitions_keep_names_and_schema_shape() {
        let tools = parse_tools(
            r#"
[[tool]]
name = "weather_query"
description = "查询天气"
url = "https://api.weather.com/v1"
method = "GET"
[tool.params]
city = { required = true, type = "string", description = "城市名" }
days = { type = "integer", description = "天数", default = 3 }
"#,
        );
        let module = DeclarativeApiModule::new(tools);
        let defs = module.definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "weather_query");
        assert_eq!(defs[0].description, "查询天气");
        let schema = &defs[0].input_schema;
        assert_eq!(schema["properties"]["city"]["type"], "string");
        assert_eq!(schema["properties"]["days"]["type"], "integer");
        assert_eq!(schema["required"][0], "city");
    }

    /// 未知名不接单（None = 交给别的模块/最终 tool_unknown）。
    #[tokio::test]
    async fn unknown_name_is_not_ours() {
        let module = DeclarativeApiModule::new(vec![]);
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
            max_tool_level: PermissionLevel::Write,
            is_clone_admin: false,
            external_url: None,
            flow_elevated_tools: None,
            flow_shell_allow: None,
            flow_deny_tools: None,
            flow_allowed_tools: None,
        };
        assert!(module
            .execute("nope_such", &serde_json::json!({}), &ctx)
            .await
            .is_none());
    }

    /// 权限面保持 ReadOnly（API 调用按配置只读门）。
    #[test]
    fn permission_level_stays_read_only() {
        let module = DeclarativeApiModule::new(vec![]);
        assert_eq!(module.permission_level("anything"), PermissionLevel::ReadOnly);
    }
}
