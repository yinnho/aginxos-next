//! api_tool_register — built-in tool for runtime API tool registration.
//!
//! M34b 起落盘单真源在 `aginx-carrier api register`（同名块替换幂等）；
//! 本模块只留定义 + 进程内 DYNAMIC_TOOLS 注册表更新（写盘成功后立即对
//! 所有化身可用，不必等守护重启）。TOML 解析留在本进程仅为了取回工具
//! 定义进注册表——校验权威在 CLI 侧。

use crate::tool_context::ToolContext;
use crate::tools::ToolModule;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::RwLock;
use carrier_types::api_tool::{ApiToolDef, ApiToolsConfig};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};

/// Global registry of dynamically registered API tools.
/// Updated after `aginx-carrier api register` succeeds on disk; read by
/// DeclarativeApiModule + messaging.rs.
static DYNAMIC_TOOLS: once_cell::sync::Lazy<RwLock<Vec<ApiToolDef>>> =
    once_cell::sync::Lazy::new(|| RwLock::new(Vec::new()));

/// Get all dynamically registered API tools (for inclusion in builtin_modules).
pub fn dynamic_tools() -> Vec<ApiToolDef> {
    DYNAMIC_TOOLS.read().map(|t| t.clone()).unwrap_or_default()
}

pub struct ApiToolRegisterModule;

#[async_trait]
impl ToolModule for ApiToolRegisterModule {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "api_tool_register".to_string(),
            description: "Register a new API tool from a TOML definition. \
                The tool becomes immediately available to all agents. \
                Provide a single [[tool]] block in TOML format."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "definition": {
                        "type": "string",
                        "description": "TOML definition of the API tool. \
                            Must be a valid [[tool]] block with at least name, description, url. \
                            Example: [[tool]]\\nname = \"weather\"\\ndescription = \"查询天气\"\\nurl = \"https://api.weather.com/v1\"\\nmethod = \"GET\"\\n..."
                    },
                    "global": {
                        "type": "boolean",
                        "default": false,
                        "description": "true = register globally (all agents), false = workspace-only"
                    }
                },
                "required": ["definition"]
            }),
        }]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        if name != "api_tool_register" {
            return None;
        }

        let definition = match input["definition"].as_str() {
            Some(s) => s,
            None => {
                return Some(Err(CarrierError::InvalidInput(
                    "Missing 'definition' parameter (TOML string)".to_string(),
                )))
            }
        };

        let global = input["global"].as_bool().unwrap_or(false);

        // 解析仅为取回工具定义进注册表；校验/写盘权威在 CLI。
        let config: ApiToolsConfig = match toml::from_str(definition) {
            Ok(c) => c,
            Err(e) => {
                return Some(Err(CarrierError::Serialization(format!(
                    "Invalid TOML: {e}"
                ))))
            }
        };
        if config.tool.is_empty() {
            return Some(Err(CarrierError::InvalidInput(
                "No [[tool]] block found in definition".to_string(),
            )));
        }
        let tool_def = config.tool[0].clone();
        let tool_name = tool_def.name.clone();

        // 落盘走 CLI（单真源写手）：stdin TOML + --global/--workspace。
        if let Err(e) = register_via_cli(definition, global, ctx.workspace_root).await {
            return Some(Err(e));
        }

        // 写盘成功 → 进程内注册表立即可用（retain 同名替换，CLI 落盘同款）。
        {
            let mut tools = match DYNAMIC_TOOLS.write() {
                Ok(t) => t,
                Err(e) => return Some(Err(CarrierError::Internal(format!("Registry lock: {e}")))),
            };
            tools.retain(|t| t.name != tool_name);
            tools.push(tool_def);
        }

        Some(Ok(format!(
            "✅ API tool '{}' registered successfully. It will be available on the next agent turn. (scope: {})",
            tool_name,
            if global { "global" } else { "workspace" }
        )))
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "api_tool_register" => PermissionLevel::Write,
            _ => PermissionLevel::Dangerous,
        }
    }
}

/// Spawn `aginx-carrier api register`（stdin=TOML 定义，stdout=D1 信封）。
/// 同信任域不 env_clear；kill_on_drop 取消不留孤儿。
async fn register_via_cli(
    definition: &str,
    global: bool,
    workspace_root: Option<&std::path::Path>,
) -> CarrierResult<()> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    if !global && workspace_root.is_none() {
        return Err(CarrierError::Internal(
            "No workspace root available".to_string(),
        ));
    }

    let mut cmd = tokio::process::Command::new("aginx-carrier");
    cmd.arg("api").arg("register").arg("--json");
    if global {
        cmd.arg("--global");
    } else {
        cmd.arg("--workspace").arg(workspace_root.unwrap());
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        CarrierError::Internal(format!(
            "aginx-carrier CLI not available ({e}) — api_tool_register writes through \
             `aginx-carrier api register`. Check PATH (/var/bin)."
        ))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(definition.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| CarrierError::Internal(format!("aginx-carrier api register subprocess failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let envelope: Value = serde_json::from_str(&stdout).map_err(|_| {
        let tail = if stdout.is_empty() { stderr } else { stdout };
        let preview = crate::str_utils::safe_truncate_str(&tail, 300);
        CarrierError::Internal(format!(
            "aginx-carrier api register returned a non-JSON response (exit {:?}): {preview}",
            output.status.code()
        ))
    })?;

    if envelope["ok"].as_bool().unwrap_or(false) {
        Ok(())
    } else {
        let msg = envelope["error"]
            .as_str()
            .or_else(|| envelope["error"]["message"].as_str())
            .unwrap_or("unknown aginx-carrier api register error");
        Err(CarrierError::Internal(msg.to_string()))
    }
}
