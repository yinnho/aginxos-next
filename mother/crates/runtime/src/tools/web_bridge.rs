//! web 桥 — browser_* / web_search / web_fetch 工具的外置实现桥（M31 D3 批1；
//! 原 agb_bridge，D13 改姓 2026-09-09）。
//!
//! 实现已整体搬到 `aginx-web` CLI（crates/web，单真源）。本模块只留：
//! - definitions()：与原 tools/browser.rs / web_search.rs / web_fetch.rs
//!   **逐字节相同**的 ToolDefinition（名字/schema/description 不动——
//!   flow `tools:` 加载期冻结、金样本、教学文本全部依赖这批名字）。
//! - execute()：spawn `aginx-web tool <name>`，stdin 喂入参 JSON，stdout 收
//!   D1 信封（{"ok":true,"data":…} / {"ok":false,"error":…}）。
//!
//! 语义：定义恒广播；执行在 aginx-web 未安装时干净报错（v1：包在场门执行，
//! 不门广告——flow 冻结不因少包漂移）。
//!
//! tool_search 同批退役（宪法性替代：`ag commands`）。见 types CORE_TOOL_NAMES。

use super::ToolModule;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;

pub struct WebBridge;

/// 桥承载的全部工具名（与 crates/web 的 aginx_web::TOOL_NAMES 一一对应）。
pub const BRIDGE_TOOL_NAMES: &[&str] = &[
    "browser_navigate",
    "browser_read_page",
    "browser_click",
    "browser_evaluate",
    "browser_type",
    "browser_scroll",
    "browser_back",
    "browser_screenshot",
    "browser_wait",
    "browser_close",
    "web_search",
    "web_fetch",
];

#[async_trait]
impl ToolModule for WebBridge {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "browser_navigate".to_string(),
                description: "Open a URL in the browser and return the page content. \
Supports markdown, html, or text output. Use CSS selectors to extract specific regions. \
Set use_proxy=true for foreign sites that may be blocked."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Target URL to open"
                        },
                        "format": {
                            "type": "string",
                            "enum": ["markdown", "html", "text"],
                            "description": "Output format. Default: markdown"
                        },
                        "selector": {
                            "type": "string",
                            "description": "Optional CSS selector to extract a specific region"
                        },
                        "wait_secs": {
                            "type": "integer",
                            "description": "Seconds to wait after page load for JS rendering"
                        },
                        "use_proxy": {
                            "type": "boolean",
                            "description": "Route through proxy for foreign sites. Default: false"
                        }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "browser_click".to_string(),
                description: "Click an element on the page using JS element.click(). \
Returns the page text after clicking."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Page URL (will navigate first)"
                        },
                        "selector": {
                            "type": "string",
                            "description": "CSS selector of the element to click"
                        },
                        "wait_secs": {
                            "type": "integer",
                            "description": "Seconds to wait after page load before clicking"
                        }
                    },
                    "required": ["url", "selector"]
                }),
            },
            ToolDefinition {
                name: "browser_evaluate".to_string(),
                description: "Run arbitrary JavaScript on the page and return the result. \
Useful for scrolling, extracting data, filling forms, or any custom interaction. \
The script can be an expression or an async IIFE."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": {
                            "type": "string",
                            "description": "Page URL (will navigate first)"
                        },
                        "script": {
                            "type": "string",
                            "description": "JavaScript expression or IIFE to execute"
                        },
                        "wait_secs": {
                            "type": "integer",
                            "description": "Seconds to wait after page load before executing"
                        }
                    },
                    "required": ["url", "script"]
                }),
            },
            // Legacy tool aliases — emulated via evaluate or return helpful error
            ToolDefinition {
                name: "browser_type".to_string(),
                description: "Type text into an input field (emulated via JS).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" },
                        "selector": { "type": "string", "description": "CSS selector of the input" },
                        "text": { "type": "string", "description": "Text to type" }
                    },
                    "required": ["url", "selector", "text"]
                }),
            },
            ToolDefinition {
                name: "browser_scroll".to_string(),
                description: "Scroll the page (emulated via JS).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" },
                        "direction": { "type": "string", "enum": ["up", "down"], "description": "Scroll direction" },
                        "amount": { "type": "integer", "description": "Pixels to scroll. Default: 500" }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "browser_back".to_string(),
                description: "Go back to the previous page (emulated via JS history.back())."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Current page URL (for context)" }
                    }
                }),
            },
            ToolDefinition {
                name: "browser_screenshot".to_string(),
                description:
                    "Capture a screenshot. NOTE: AginxBrowser does not support screenshots. \
Use browser_navigate to extract page content instead."
                        .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" }
                    }
                }),
            },
            ToolDefinition {
                name: "browser_read_page".to_string(),
                description:
                    "Extract page content as text. Alias for browser_navigate with format=text."
                        .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" },
                        "selector": { "type": "string" }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "browser_wait".to_string(),
                description: "Wait for a condition or element (emulated via JS).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string" },
                        "selector": { "type": "string", "description": "CSS selector to wait for" },
                        "timeout_ms": { "type": "integer", "description": "Max wait time in ms. Default: 5000" }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "browser_close".to_string(),
                description:
                    "Close the browser session. NOTE: AginxBrowser is stateless; this is a no-op."
                        .to_string(),
                input_schema: serde_json::json!({ "type": "object", "properties": {} }),
            },
            ToolDefinition {
                name: "web_search".to_string(),
                description: "Search the web using AginxBrowser (native search aggregation). \
                    Returns results with title, URL, and snippet. \
                    Set fetch_top>0 to auto-fetch full content for top N results (one-step search+read)."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "q": {
                            "type": "string",
                            "description": "Search query"
                        },
                        "fetch_top": {
                            "type": "integer",
                            "description": "Auto-fetch full content for top N results. Default: 0 (snippet only, fast)"
                        },
                        "categories": {
                            "type": "string",
                            "description": "Search category: general, news, images, etc. Default: general"
                        },
                        "language": {
                            "type": "string",
                            "description": "Language code. Default: zh-CN"
                        },
                        "max_results": {
                            "type": "integer",
                            "description": "Max number of results. Default: 10"
                        },
                        "max_chars_per": {
                            "type": "integer",
                            "description": "Truncate each fetched content to N chars. Default: 4000. 0 = no limit"
                        },
                    },
                    "required": ["q"]
                }),
            },
            ToolDefinition {
                name: "web_fetch".to_string(),
                description: "Fetch a URL with SSRF protection. Supports GET/POST/PUT/PATCH/DELETE. \
                    For GET, HTML is converted to Markdown. For other methods, returns raw response body."
                    .to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "The URL to fetch (http/https only)" },
                        "method": { "type": "string", "enum": ["GET","POST","PUT","PATCH","DELETE"], "description": "HTTP method (default: GET)" },
                        "headers": { "type": "object", "description": "Custom HTTP headers as key-value pairs" },
                        "body": { "type": "string", "description": "Request body for POST/PUT/PATCH" }
                    },
                    "required": ["url"]
                }),
            },
        ]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        _ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        if !BRIDGE_TOOL_NAMES.contains(&name) {
            return None;
        }
        Some(run_web_tool(name, input).await)
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "browser_navigate" | "browser_read_page" | "browser_click" | "browser_evaluate"
            | "browser_type" | "browser_scroll" | "browser_back" | "browser_wait"
            | "browser_screenshot" | "browser_close" | "web_search" | "web_fetch" => {
                PermissionLevel::ReadOnly
            }
            _ => PermissionLevel::Dangerous,
        }
    }
}

/// Spawn `aginx-web tool <name>`（stdin=入参 JSON，stdout=D1 信封）并解信封。
///
/// sandbox 同 shell.rs 直接执行路径：env_clear 后只回 PATH/HOME 等
/// SAFE_ENV_VARS —— aginx-web 启动时自己 load_dotenv()，故 ~/.aginx/carrier/.env
/// 的 AGINXBROWSER_URL 在子进程内仍生效。kill_on_drop：超时/取消不留孤儿。
async fn run_web_tool(name: &str, input: &Value) -> CarrierResult<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new("aginx-web");
    cmd.arg("tool").arg(name);
    crate::subprocess_sandbox::sandbox_command(&mut cmd, &[]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        CarrierError::Internal(format!(
            "aginx-web CLI not available ({e}) — browser/web tools live in the `aginx-web` \
             package. Install it (`ag pkg install aginx-web`) or check PATH."
        ))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort write; aginx-web reads stdin to EOF before executing.
        let payload = serde_json::to_vec(input).unwrap_or_default();
        let _ = stdin.write_all(&payload).await;
        let _ = stdin.shutdown().await;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| CarrierError::Internal(format!("aginx-web tool {name} subprocess failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let envelope: Value = serde_json::from_str(&stdout).map_err(|_| {
        let tail = if stdout.is_empty() { stderr } else { stdout };
        let preview = crate::str_utils::safe_truncate_str(&tail, 300);
        CarrierError::Internal(format!(
            "aginx-web tool {name} returned a non-JSON response (exit {:?}): {preview}",
            output.status.code()
        ))
    })?;

    if envelope["ok"].as_bool().unwrap_or(false) {
        envelope["data"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                CarrierError::Serialization("aginx-web envelope missing string data field".to_string())
            })
    } else {
        let msg = envelope["error"].as_str().unwrap_or("unknown aginx-web error");
        Err(CarrierError::Internal(msg.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_match_bridge_names() {
        let bridge = WebBridge;
        let defs = bridge.definitions();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        let mut expected = BRIDGE_TOOL_NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn all_definitions_read_only() {
        let bridge = WebBridge;
        for name in BRIDGE_TOOL_NAMES {
            assert!(
                matches!(bridge.permission_level(name), PermissionLevel::ReadOnly),
                "{name} should stay ReadOnly"
            );
        }
    }

    #[tokio::test]
    async fn unknown_name_is_none() {
        let bridge = WebBridge;
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
        assert!(
            bridge
                .execute("tool_search", &serde_json::json!({}), &ctx)
                .await
                .is_none(),
            "tool_search is retired — must not be claimed by the bridge"
        );
        assert!(bridge
            .execute("nope", &serde_json::json!({}), &ctx)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn spawn_without_web_cli_reports_clean_error() {
        // PATH scrubbed → spawn must fail with the "aginx-web CLI not
        // available" hint, never a panic and never a hang.
        let r = run_web_tool("browser_close", &serde_json::json!({})).await;
        // On dev hosts aginx-web may actually be on PATH (built earlier); then
        // the call succeeds. Either way the contract is: Ok(string) or Err(hint).
        match r {
            Ok(s) => assert!(s.contains("stateless")),
            Err(e) => assert!(
                e.to_string().contains("aginx-web CLI not available"),
                "unexpected error: {e}"
            ),
        }
    }
}
