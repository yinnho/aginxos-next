//! Built-in tool execution.
//!
//! Provides web tools and tool dispatch. Most tools are now in the `tools` module.

use crate::mcp;
use crate::tool_context::ToolContext;
use carrier_types::tool::{ToolDefinition, ToolResult};
use carrier_types::tool_compat::normalize_tool_name;
use tracing::{debug, warn};

tokio::task_local! {
    /// Tracks the current inter-agent call depth within a task.
    pub(crate) static AGENT_CALL_DEPTH: std::cell::Cell<u32>;
    /// Canvas max HTML size in bytes (set from kernel config at loop start).
    pub(crate) static CANVAS_MAX_BYTES: usize;
}

/// Maximum inter-agent call depth (used by agent tools).
pub(crate) const MAX_AGENT_CALL_DEPTH: u32 = 5;

/// Current inter-agent call depth of this task (0 outside an agent turn) —
/// pub wrapper for the carrier CLI `tool` face (M33): the bridge reads it
/// before spawning the child so the guard survives the process boundary.
pub fn current_agent_call_depth() -> u32 {
    AGENT_CALL_DEPTH.try_with(|d| d.get()).unwrap_or(0)
}

/// Run `fut` with the inter-agent call depth set to `depth` — pub wrapper
/// used by the carrier CLI `agent send`/`tool agent_send` face to seed the
/// depth it received via `AGINX_AGENT_DEPTH` env (bridge passes depth+1).
pub fn scope_agent_call_depth<F: std::future::Future>(
    depth: u32,
    fut: F,
) -> impl std::future::Future<Output = F::Output> {
    AGENT_CALL_DEPTH.scope(std::cell::Cell::new(depth), fut)
}

/// Max inter-agent call depth as a pub constant (CLI-side guard parity).
pub fn max_agent_call_depth() -> u32 {
    MAX_AGENT_CALL_DEPTH
}

/// Strip a toolset prefix ("filesystem__shell_exec" → "shell_exec") for
/// base-name comparison in the flow `tools:` hard sandbox.
pub(crate) fn base_tool_name(n: &str) -> &str {
    n.rsplit_once("__").map(|(_, b)| b).unwrap_or(n)
}

/// Whether `tool_name` may appear in the LLM tool list / be discovered under a
/// flow `tools:` hard sandbox. `None` or empty allow-list = no sandbox.
/// `mcp_*` is always permitted (same rule as execute-time).
pub(crate) fn tool_permitted_in_flow(tool_name: &str, allowed: Option<&[String]>) -> bool {
    let Some(allowed) = allowed.filter(|a| !a.is_empty()) else {
        return true;
    };
    if tool_name.starts_with("mcp_") {
        return true;
    }
    let called = base_tool_name(tool_name);
    allowed.iter().any(|a| base_tool_name(a) == called)
}

/// Execute a tool by name with the given input, returning a ToolResult.
///
/// The optional `kernel` handle enables inter-agent tools. If `None`,
/// agent tools will return an error indicating the kernel is not available.
///
/// `max_tool_level` enforces permission-based security: tools above the
/// agent's maximum permission level are not available.
pub async fn execute_tool(
    tool_use_id: &str,
    tool_name: &str,
    input: &serde_json::Value,
    ctx: &ToolContext<'_>,
) -> ToolResult {
    // Unpack context into local bindings matching the old parameter names.
    let ToolContext {
        kernel,
        memory: _,
        caller_agent_id,
        mcp_connections,
        allowed_env_vars: _,
        workspace_root,
        brain: _,
        exec_policy: _,
        process_manager: _,
        sender_id,
        owner_id: _,
        home_dir: _,
        agent_name: _,
        subagent_configs: _,
        channel_type,
        max_tool_level,
        cli_exec_config: _,
        is_clone_admin,
        external_url: _,
        flow_elevated_tools,
        flow_shell_allow,
        flow_deny_tools,
        flow_allowed_tools,
    } = *ctx;

    // Normalize the tool name through compat mappings so LLM-hallucinated aliases
    // (e.g. "fs-write" → "file_write") resolve to the canonical Carrier name.
    let tool_name = normalize_tool_name(tool_name);

    let input_ref = input;

    let flow_elevated = flow_elevated_tools
        .map(|names| names.iter().any(|n| n == tool_name))
        .unwrap_or(false);

    // Flow deny_tools (e.g. image_generate on template poster flows).
    if let Some(denied) = flow_deny_tools {
        if denied.iter().any(|d| d == tool_name) {
            warn!(
                tool_name,
                "Permission denied: tool blocked by flow deny_tools"
            );
            return ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: format!(
                    "Permission denied: '{tool_name}' 被当前 flow 禁止（deny_tools）。请按 flow 指定的脚本/模板路径执行，不要用此工具。"
                ),
                is_error: true,
            };
        }
    }

    // Flow `tools:` hard sandbox (flow_allowed_tools): when the matched flow
    // declares a non-empty tool set, only tools in that frozen allow-list may
    // run. This stops the agent wandering to out-of-flow catalog tools (e.g.
    // clone-creator reaching train_write instead of the declared file_write).
    // Both sides are normalized to base names ("filesystem__x" → "x") so
    // toolset-prefixed names compare equal.
    if !tool_permitted_in_flow(tool_name, flow_allowed_tools) {
        warn!(
            tool_name,
            allowed_count = flow_allowed_tools.map(|a| a.len()).unwrap_or(0),
            "Permission denied: tool not in flow's declared tool set"
        );
        return ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: format!(
                "Permission denied: '{tool_name}' 不在当前 flow 声明的工具集中（flow tools 硬沙箱）。请只使用 flow `tools:` 声明的工具来完成本任务——不要调用 flow 未声明的工具。"
            ),
            is_error: true,
        };
    }

    // Admin gate — orthogonal to max_tool_level. A small set of irreversible /
    // brand-affecting tools (shell execution, publishing to a public account)
    // require the caller to be a clone admin (creator or approved admin, per
    // admins.json). System-flow elevation may bypass this for tools listed in
    // the shared flow's `tools:` (e.g. office-xlsx shell_exec).
    // See docs/ADMIN-MECHANISM.md and docs/OFFICE-SYSTEM-FLOWS.md.
    if !is_clone_admin && carrier_types::tool::is_admin_gated(tool_name) && !flow_elevated {
        warn!(
            tool_name,
            "Permission denied: admin-gated tool, caller is not a clone admin"
        );
        return ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: format!(
                "Permission denied: '{tool_name}' 需要管理员权限，仅分身管理员可执行。"
            ),
            is_error: true,
        };
    }

    // System-flow shell_allow: elevated shell_exec must match declared patterns.
    if tool_name == "shell_exec" {
        if let Some(patterns) = flow_shell_allow {
            if !patterns.is_empty() {
                let command = input_ref["command"].as_str().unwrap_or("");
                if !carrier_types::flow::command_matches_flow_shell_allow(
                    command,
                    patterns,
                    workspace_root,
                ) {
                    warn!(
                        tool_name,
                        command = %crate::str_utils::safe_truncate_str(command, 80),
                        "Permission denied: shell command not in flow shell_allow"
                    );
                    return ToolResult {
                        tool_use_id: tool_use_id.to_string(),
                        content: format!(
                            "权限拒绝：shell 命令不匹配本流程的 shell_allow 白名单。\
                             命令：{}。允许的 pattern：{}。\
                             修复：脚本路径写 workspace 相对或绝对都认（系统自动归一化）；\
                             确需执行请把该脚本目录加进 flow 的 shell_allow 后 dup push 提交",
                            crate::str_utils::safe_truncate_str(command, 120),
                            patterns.join(", ")
                        ),
                        is_error: true,
                    };
                }
            }
        }
    }

    // Permission enforcement: reject tools above max_tool_level or Dangerous
    let cli_exec_config = ctx.cli_exec_config.cloned().unwrap_or_default();
    let modules = crate::tools::builtin_modules(cli_exec_config);
    let mut permission_checked = false;
    for module in &modules {
        if module.definitions().iter().any(|d| d.name == tool_name) {
            let level = module.permission_level(tool_name);
            if level > max_tool_level && !flow_elevated {
                warn!(
                    tool_name,
                    ?level,
                    ?max_tool_level,
                    "Permission denied: tool exceeds max level"
                );
                return ToolResult {
                    tool_use_id: tool_use_id.to_string(),
                    content: format!(
                        "Permission denied: tool '{tool_name}' requires {:?} level but agent is limited to {:?}",
                        level, max_tool_level
                    ),
                    is_error: true,
                };
            }
            permission_checked = true;
            break;
        }
    }

    // For tools not in any builtin module (e.g. MCP tools), use the
    // centralized PermissionLevel::for_tool() for permission checks.
    if !permission_checked {
        let level = carrier_types::tool::PermissionLevel::for_tool(tool_name);
        if level > max_tool_level && !flow_elevated {
            // Suggest likely intended tool names when the LLM hallucinates
            // a toolset name (e.g. "filesystem") instead of a real tool name.
            let suggestion = match tool_name {
                "filesystem" => " Did you mean file_write, file_read, or file_list?",
                "knowledge" => " Did you mean knowledge_add, knowledge_read, or knowledge_list?",
                "shell" => " Did you mean shell_exec?",
                "media" => " Did you mean image_generate or text_to_speech?",
                _ => "",
            };
            warn!(
                tool_name,
                ?level,
                ?max_tool_level,
                "Permission denied: non-builtin tool exceeds max level"
            );
            return ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: format!(
                    "Permission denied: '{tool_name}' is not a callable tool.{suggestion} Available tools are listed in your tool definitions.",
                ),
                is_error: true,
            };
        }
    }

    debug!(tool_name, "Executing tool");

    // Phase 1: Try extracted tool modules (filesystem, shell, misc, ...)
    let cli_exec_config = ctx.cli_exec_config.cloned().unwrap_or_default();
    let modules = crate::tools::builtin_modules(cli_exec_config);
    for module in &modules {
        if let Some(result) = module.execute(tool_name, input_ref, ctx).await {
            return match result {
                Ok(content) => ToolResult {
                    tool_use_id: tool_use_id.to_string(),
                    content: truncate_tool_result(tool_name, content),
                    is_error: false,
                },
                Err(err) => {
                    warn!(tool_name, error = %err, "Tool execution failed");
                    ToolResult {
                        tool_use_id: tool_use_id.to_string(),
                        content: format!("Error: {err}"),
                        is_error: true,
                    }
                }
            };
        }
    }

    // Phase 1.5: Plugin tool dispatcher — remaining channel tools (e.g.
    // charter_create_order, weixin_oa_publish_article) registered as ToolProvider
    // instances. Rich content delivery uses the unified Channel::deliver path
    // and [DELIVER:key] markers instead of channel-specific send tools.
    // Run on a blocking thread: plugin tools internally block_on a fresh
    // runtime, which would panic inside this async tokio context.
    if let Some(kernel) = kernel {
        let kernel = kernel.clone();
        let tool_name_owned = tool_name.to_string();
        let args_owned = input_ref.clone();
        let plugin_ctx = carrier_types::plugin::PluginToolContext {
            // bot_id (OA app_id): single-OA deployments resolve via the tool's
            // WEIXIN_OA_STATE fallback when invoked without an inbound context.
            bot_id: String::new(),
            sender_id: sender_id.unwrap_or("").to_string(),
            agent_id: caller_agent_id.unwrap_or("").to_string(),
            channel_type: channel_type.unwrap_or("").to_string(),
        };
        let join = tokio::task::spawn_blocking(move || {
            kernel.execute_plugin_tool(&tool_name_owned, &args_owned, &plugin_ctx)
        })
        .await;
        if let Ok(exec_result) = join {
            match exec_result {
                Ok(Some(content)) => {
                    return ToolResult {
                        tool_use_id: tool_use_id.to_string(),
                        content: truncate_tool_result(tool_name, content),
                        is_error: false,
                    };
                }
                Ok(None) => { /* no plugin handles it — fall through to MCP/other dispatch */ }
                Err(err) => {
                    warn!(tool_name = %tool_name, error = %err, "Plugin tool execution failed");
                    return ToolResult {
                        tool_use_id: tool_use_id.to_string(),
                        content: format!("Error: {err}"),
                        is_error: true,
                    };
                }
            }
        }
    }

    // Phase 2: MCP fallback (all built-in tools now handled by ToolModules in Phase 1)
    let result = {
        // Fallback 1: MCP tools (mcp_{server}_{tool} prefix)
        // Permission already enforced by max_tool_level check above
        if mcp::is_mcp_tool(tool_name) {
            if let Some(mcp_conns) = mcp_connections {
                // Collect known server keys from DashMap for name resolution
                let known_keys: Vec<String> = mcp_conns.iter().map(|e| e.key().clone()).collect();
                let known_refs: Vec<&str> = known_keys.iter().map(|s| s.as_str()).collect();
                if let Some(server_key) = mcp::extract_mcp_server_from_known(tool_name, &known_refs)
                {
                    // O(1) lookup by normalized server name — no global lock
                    if let Some(mut conn) = mcp_conns.get_mut(&server_key.to_string()) {
                        debug!(
                            tool = tool_name,
                            server = server_key,
                            "Dispatching to MCP server"
                        );
                        match conn.call_tool(tool_name, input_ref).await {
                            Ok(content) => Ok(content),
                            Err(e) => Err(format!("MCP tool call failed: {e}")),
                        }
                    } else {
                        Err(format!("MCP server '{server_key}' not connected"))
                    }
                } else {
                    Err(format!("Invalid MCP tool name: {tool_name}"))
                }
            } else {
                Err(format!("MCP not available for tool: {tool_name}"))
            }
        } else {
            Err(format!("Unknown tool: {tool_name}"))
        }
    };

    match result {
        Ok(content) => ToolResult {
            tool_use_id: tool_use_id.to_string(),
            content: truncate_tool_result(tool_name, content),
            is_error: false,
        },
        Err(err) => {
            warn!(tool_name, error = %err, "Tool execution failed");
            ToolResult {
                tool_use_id: tool_use_id.to_string(),
                content: format!("Error: {err}"),
                is_error: true,
            }
        }
    }
}

/// Per-tool maximum result size in characters.
/// Tools returning more than this will be truncated with a marker.
/// None means no per-tool limit (dynamic context truncation still applies).
///
/// Thin wrapper over [`crate::tool_meta::tool_meta`] — the single source of
/// truth for tool metadata.
fn tool_max_result_chars(name: &str) -> Option<usize> {
    crate::tool_meta::tool_meta(name).max_result_chars
}

/// Truncate a tool result if it exceeds the per-tool max size.
/// Two-stage compression: collapse duplicate lines, then keep head + tail.
fn truncate_tool_result(tool_name: &str, content: String) -> String {
    let max = match tool_max_result_chars(tool_name) {
        Some(m) => m,
        None => return content,
    };
    if content.len() <= max {
        return content;
    }

    let original_len = content.len();

    // Stage 1: collapse consecutive duplicate lines (3+ → keep first + marker)
    let deduped = dedup_lines(&content);
    if deduped.len() <= max {
        let saved = original_len.saturating_sub(deduped.len());
        if saved > 0 {
            return format!(
                "{deduped}\n\n[compressed: {:.1} KB → {:.1} KB]",
                original_len as f64 / 1024.0,
                deduped.len() as f64 / 1024.0
            );
        }
        return deduped;
    }

    // Stage 2: keep head + tail lines
    let result = smart_truncate(&deduped, max);
    format!(
        "{result}\n\n[compressed: {:.1} KB → {:.1} KB]",
        original_len as f64 / 1024.0,
        result.len() as f64 / 1024.0,
    )
}

/// Collapse consecutive duplicate lines. Runs of 3+ identical lines keep only
/// the first occurrence. Runs of 2 are preserved as-is.
fn dedup_lines(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < 5 {
        return content.to_string();
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let mut run = 1;
        while i + run < lines.len() && lines[i + run] == line {
            run += 1;
        }
        out.push(line.to_string());
        if run >= 3 {
            out.push(format!("  ... ({} duplicate lines)", run - 1));
        } else if run == 2 {
            out.push(line.to_string());
        }
        i += run;
    }
    out.join("\n")
}

/// Keep HEAD_LINES from the top and TAIL_LINES from the bottom, drop the middle.
/// Falls back to char-boundary truncation for content too short for head/tail.
fn smart_truncate(content: &str, max_chars: usize) -> String {
    const HEAD_LINES: usize = 120;
    const TAIL_LINES: usize = 60;

    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= HEAD_LINES + TAIL_LINES + 10 {
        if content.len() <= max_chars {
            return content.to_string();
        }
        let mut bp = carrier_types::floor_char_boundary(content, max_chars);
        let search_start = carrier_types::floor_char_boundary(content, bp.saturating_sub(200));
        if let Some(nl_pos) = content[search_start..bp].rfind('\n') {
            bp = search_start + nl_pos;
        }
        return content[..bp].to_string();
    }

    let head: Vec<&str> = lines.iter().take(HEAD_LINES).copied().collect();
    let tail: Vec<&str> = lines.iter().rev().take(TAIL_LINES).copied().rev().collect();
    let cut = lines.len() - head.len() - tail.len();
    format!(
        "{}\n\n... +{cut} lines\n\n{}",
        head.join("\n"),
        tail.join("\n"),
    )
}

/// Get definitions for all built-in tools.
pub fn builtin_tool_definitions(
    cli_exec_config: carrier_types::config::CliExecConfig,
) -> Vec<ToolDefinition> {
    // All built-in tool definitions come from ToolModules now.
    crate::tools::builtin_modules(cli_exec_config)
        .into_iter()
        .flat_map(|m| m.definitions())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: empty ToolContext for tests that don't need any services.
    fn noop_ctx() -> ToolContext<'static> {
        ToolContext {
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
        }
    }

    #[tokio::test]
    async fn test_flow_elevated_shell_exec_bypasses_admin_and_level() {
        // Without elevation: shell_exec denied (Write level + non-admin).
        let denied = execute_tool(
            "t1",
            "shell_exec",
            &serde_json::json!({"command": "python3 output/scripts/gen.py"}),
            &noop_ctx(),
        )
        .await;
        assert!(denied.is_error);
        assert!(denied.content.contains("Permission denied"));

        // With elevation + matching shell_allow: admin + level gates pass.
        // Command may still fail at execution (no workspace), but not permission.
        let elevated = ["shell_exec".to_string()];
        let allow = ["python3 output/scripts/*".to_string()];
        let mut ctx = noop_ctx();
        ctx.max_tool_level = carrier_types::tool::PermissionLevel::Dangerous;
        ctx.flow_elevated_tools = Some(&elevated);
        ctx.flow_shell_allow = Some(&allow);
        let result = execute_tool(
            "t2",
            "shell_exec",
            &serde_json::json!({"command": "python3 output/scripts/gen.py"}),
            &ctx,
        )
        .await;
        // Not a permission deny (admin/level/shell_allow). May fail on spawn.
        assert!(
            !result.content.contains("需要管理员权限")
                && !result
                    .content
                    .contains("not allowed by system flow shell_allow")
                && !result.content.contains("requires Dangerous level"),
            "unexpected permission deny: {}",
            result.content
        );
    }

    #[test]
    fn test_tool_permitted_in_flow_filters_catalog() {
        let allowed = ["file_read".to_string(), "clone_install".to_string()];
        assert!(tool_permitted_in_flow("file_read", Some(&allowed)));
        assert!(tool_permitted_in_flow("mcp_wecom_x", Some(&allowed)));
        assert!(!tool_permitted_in_flow("train_write", Some(&allowed)));
        assert!(tool_permitted_in_flow("train_write", None));
        assert!(tool_permitted_in_flow("train_write", Some(&[])));
    }

    #[tokio::test]
    async fn test_flow_allowed_tools_blocks_out_of_set_tool() {
        // Flow declares clone_install + file_read; calling train_write (not in
        // the frozen allow-list) is denied by the hard sandbox.
        let allowed = ["clone_install".to_string(), "file_read".to_string()];
        let mut ctx = noop_ctx();
        ctx.flow_allowed_tools = Some(&allowed);
        let result = execute_tool(
            "t4",
            "train_write",
            &serde_json::json!({"target": "x", "path": "p", "content": "c"}),
            &ctx,
        )
        .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("不在当前 flow 声明的工具集"),
            "expected flow sandbox deny, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_flow_allowed_tools_permits_declared_tool() {
        // clone_install IS in the allow-list -> sandbox check passes. It fails
        // downstream (no kernel handle) but NOT with the sandbox deny message.
        let allowed = ["clone_install".to_string(), "file_read".to_string()];
        let mut ctx = noop_ctx();
        ctx.flow_allowed_tools = Some(&allowed);
        let result = execute_tool(
            "t5",
            "clone_install",
            &serde_json::json!({"name": "x", "files": {"SOUL.md": "s"}}),
            &ctx,
        )
        .await;
        assert!(
            !result.content.contains("不在当前 flow 声明的工具集"),
            "declared tool should not be sandbox-denied, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_flow_allowed_tools_base_name_normalization() {
        // Toolset-prefixed out-of-set tool normalizes to base "train_write" and
        // is still denied; a prefixed in-set tool normalizes to "clone_install"
        // and passes the sandbox (fails downstream, not on the sandbox).
        let allowed = ["clone_install".to_string()];
        let mut ctx = noop_ctx();
        ctx.flow_allowed_tools = Some(&allowed);
        let denied = execute_tool(
            "t6",
            "training__train_write",
            &serde_json::json!({"target": "x"}),
            &ctx,
        )
        .await;
        assert!(denied.is_error);
        assert!(
            denied.content.contains("不在当前 flow 声明的工具集"),
            "prefixed out-of-set tool should be denied, got: {}",
            denied.content
        );
        let permitted = execute_tool(
            "t7",
            "training__clone_install",
            &serde_json::json!({"name": "x", "files": {"SOUL.md": "s"}}),
            &ctx,
        )
        .await;
        assert!(
            !permitted.content.contains("不在当前 flow 声明的工具集"),
            "prefixed in-set tool should pass sandbox, got: {}",
            permitted.content
        );
    }

    #[tokio::test]
    async fn test_flow_allowed_tools_exempts_mcp_tools() {
        // MCP tools (mcp_*) are exempt from the sandbox even when not declared:
        // flows call them in their body without listing each in `tools:`.
        let allowed = ["clone_install".to_string()];
        let mut ctx = noop_ctx();
        ctx.flow_allowed_tools = Some(&allowed);
        let result = execute_tool(
            "t8",
            "mcp_wechat_oa_create_draft",
            &serde_json::json!({"title": "t", "content": "c"}),
            &ctx,
        )
        .await;
        assert!(
            !result.content.contains("不在当前 flow 声明的工具集"),
            "mcp tool should be exempt from sandbox, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_flow_shell_allow_blocks_unmatched_command() {
        let elevated = ["shell_exec".to_string()];
        let allow = ["python3 output/scripts/*".to_string()];
        let mut ctx = noop_ctx();
        ctx.max_tool_level = carrier_types::tool::PermissionLevel::Dangerous;
        ctx.flow_elevated_tools = Some(&elevated);
        ctx.flow_shell_allow = Some(&allow);
        let result = execute_tool(
            "t3",
            "shell_exec",
            &serde_json::json!({"command": "rm -rf /tmp/foo"}),
            &ctx,
        )
        .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("shell_allow") || result.content.contains("not allowed"),
            "got: {}",
            result.content
        );
    }

    #[test]
    fn test_builtin_tool_definitions() {
        let tools = builtin_tool_definitions(carrier_types::config::CliExecConfig::default());
        assert!(
            tools.len() >= 25,
            "Expected at least 25 tools, got {}",
            tools.len()
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        // Training tools (cross-workspace)
        assert!(
            names.contains(&"train_read"),
            "Missing train_read in: {:?}",
            names
        );
        assert!(names.contains(&"train_write"), "Missing train_write");
        assert!(names.contains(&"train_list"), "Missing train_list");
        assert!(names.contains(&"train_evaluate"), "Missing train_evaluate");
        // Original 12
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"shell_exec"));
        assert!(names.contains(&"agent_send"));
        assert!(names.contains(&"agent_spawn"));
        assert!(names.contains(&"agent_list"));
        assert!(names.contains(&"agent_kill"));
        assert!(names.contains(&"agent_send"));
        assert!(names.contains(&"agent_list"));
        // 6 collaboration tools
        assert!(names.contains(&"agent_find"));
        assert!(names.contains(&"task_post"));
        assert!(names.contains(&"task_claim"));
        assert!(names.contains(&"task_complete"));
        assert!(names.contains(&"task_list"));
        assert!(names.contains(&"event_publish"));
        // 5 new Phase 3 tools
        assert!(names.contains(&"schedule_create"));
        assert!(names.contains(&"schedule_list"));
        assert!(names.contains(&"schedule_delete"));
        assert!(names.contains(&"image_analyze"));
        assert!(names.contains(&"location_get"));
        assert!(names.contains(&"system_time"));
        // Browser tools are now provided by browser-mcp (standalone MCP server)
        // 3 media/image generation tools
        assert!(names.contains(&"media_describe"));
        assert!(names.contains(&"media_transcribe"));
        assert!(names.contains(&"image_generate"));
        // 3 cron tools
        assert!(names.contains(&"cron_create"));
        assert!(names.contains(&"cron_list"));
        assert!(names.contains(&"cron_cancel"));
        // Voice tools
        assert!(names.contains(&"text_to_speech"));
        assert!(names.contains(&"speech_to_text"));
        // Canvas tool
        assert!(names.contains(&"canvas_present"));
    }

    #[test]
    fn test_collaboration_tool_schemas() {
        let tools = builtin_tool_definitions(carrier_types::config::CliExecConfig::default());
        let collab_tools = [
            "agent_find",
            "task_post",
            "task_claim",
            "task_complete",
            "task_list",
            "event_publish",
        ];
        for name in &collab_tools {
            let tool = tools
                .iter()
                .find(|t| t.name == *name)
                .unwrap_or_else(|| panic!("Tool '{}' not found", name));
            // Verify each has a valid JSON schema
            assert!(
                tool.input_schema.is_object(),
                "Tool '{}' schema should be an object",
                name
            );
            assert_eq!(
                tool.input_schema["type"], "object",
                "Tool '{}' should have type=object",
                name
            );
        }
    }

    #[tokio::test]
    async fn test_file_read_missing() {
        let bad_path = std::env::temp_dir()
            .join("carrier_test_nonexistent_99999")
            .join("file.txt");
        let result = execute_tool(
            "test-id",
            "file_read",
            &serde_json::json!({"path": bad_path.to_str().unwrap()}),
            &noop_ctx(),
        )
        .await;
        assert!(
            result.is_error,
            "Expected error but got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_file_read_path_traversal_blocked() {
        let result = execute_tool(
            "test-id",
            "file_read",
            &serde_json::json!({"path": "../../etc/passwd"}),
            &noop_ctx(),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("traversal"));
    }

    #[tokio::test]
    async fn test_file_write_path_traversal_blocked() {
        let result = execute_tool(
            "test-id",
            "file_write",
            &serde_json::json!({"path": "../../../tmp/evil.txt", "content": "pwned"}),
            &noop_ctx(),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("traversal"));
    }

    #[tokio::test]
    async fn test_file_list_path_traversal_blocked() {
        let result = execute_tool(
            "test-id",
            "file_list",
            &serde_json::json!({"path": "/foo/../../etc"}),
            &noop_ctx(),
        )
        .await;
        assert!(result.is_error);
        assert!(
            result.content.contains("traversal") || result.content.contains("Absolute"),
            "Expected path rejection, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let result = execute_tool(
            "test-id",
            "nonexistent_tool",
            &serde_json::json!({}),
            &noop_ctx(),
        )
        .await;
        assert!(result.is_error);
        // Unknown tools are rejected by permission check (for_tool defaults to Dangerous)
        // before reaching the "Unknown tool" error path. Both outcomes are correct.
        assert!(
            result.content.contains("Permission denied") || result.content.contains("Unknown tool")
        );
    }

    #[tokio::test]
    async fn test_agent_tools_without_kernel() {
        // M33：agent_* 走 carrier 桥——无内核态可报错，执行经
        // `aginx-carrier tool` 子进程。host 测试机 PATH 无该 CLI →
        // 干净报错（bridge 不门广播，只门执行）。
        let result =
            execute_tool("test-id", "agent_list", &serde_json::json!({}), &noop_ctx()).await;
        assert!(result.is_error, "expected error, got: {}", result.content);
        assert!(
            result.content.contains("aginx-carrier")
                || result.content.contains("Kernel handle not available"),
            "expected bridge/CLI error, got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_permission_level_denied() {
        // shell_exec is Dangerous level, noop_ctx has Write level — should be denied
        let result = execute_tool(
            "test-id",
            "shell_exec",
            &serde_json::json!({"command": "ls"}),
            &noop_ctx(),
        )
        .await;
        assert!(result.is_error);
        assert!(result.content.contains("Permission denied"));
    }

    #[tokio::test]
    async fn test_permission_level_allowed() {
        // file_read is ReadOnly level, noop_ctx has Write level — should pass permission check
        let result = execute_tool(
            "test-id",
            "file_read",
            &serde_json::json!({"path": "carrier_test_nonexistent_12345/file.txt"}),
            &noop_ctx(),
        )
        .await;
        assert!(
            result.is_error,
            "Expected error but got: {}",
            result.content
        );
        assert!(
            result.content.contains("Failed to read")
                || result.content.contains("not found")
                || result.content.contains("No such file"),
            "Unexpected error: {}",
            result.content
        );
    }

    #[test]
    fn test_depth_limit_constant() {
        assert_eq!(MAX_AGENT_CALL_DEPTH, 5);
    }

    #[test]
    fn test_depth_limit_first_call_succeeds() {
        // Default depth is 0, which is < MAX_AGENT_CALL_DEPTH
        let default_depth = AGENT_CALL_DEPTH.try_with(|d| d.get()).unwrap_or(0);
        assert!(default_depth < MAX_AGENT_CALL_DEPTH);
    }

    #[test]
    fn test_task_local_compiles() {
        // Verify task_local macro works — just ensure the type exists
        let cell = std::cell::Cell::new(0u32);
        assert_eq!(cell.get(), 0);
    }

    #[tokio::test]
    async fn test_schedule_tools_without_kernel() {
        // M33：schedule_* 走 carrier 桥（同 test_agent_tools_without_kernel）。
        let result = execute_tool(
            "test-id",
            "schedule_list",
            &serde_json::json!({}),
            &noop_ctx(),
        )
        .await;
        assert!(result.is_error, "expected error, got: {}", result.content);
        assert!(
            result.content.contains("aginx-carrier")
                || result.content.contains("memory")
                || result.content.contains("Kernel"),
            "expected bridge/CLI error, got: {}",
            result.content
        );
    }

    // ------------------------------------------------------------------
    // dedup_lines
    // ------------------------------------------------------------------

    #[test]
    fn test_dedup_lines_collapses_long_runs() {
        let input = "line1\nline1\nline1\nline2\nline3\nline3\nline3\nline3";
        let out = dedup_lines(input);
        assert!(out.contains("... (2 duplicate lines)"));
        assert!(out.contains("... (3 duplicate lines)"));
        // line2 preserved
        assert!(out.contains("\nline2\n"));
    }

    #[test]
    fn test_dedup_lines_preserves_pairs() {
        let input = "a\na\nb\nb";
        let out = dedup_lines(input);
        assert!(!out.contains("duplicate"));
        assert_eq!(out.matches('\n').count(), input.matches('\n').count());
    }

    #[test]
    fn test_dedup_lines_skips_short_content() {
        let input = "one\ntwo";
        let out = dedup_lines(input);
        assert_eq!(out, input);
    }

    // ------------------------------------------------------------------
    // smart_truncate
    // ------------------------------------------------------------------

    #[test]
    fn test_smart_truncate_keeps_head_and_tail() {
        let mut lines: Vec<String> = Vec::new();
        for i in 0..300 {
            lines.push(format!("line {i}"));
        }
        let content = lines.join("\n");
        let out = smart_truncate(&content, 4096);
        // Should contain early lines
        assert!(out.contains("line 0"));
        assert!(out.contains("line 10"));
        // Should contain late lines
        assert!(out.contains("line 299"));
        assert!(out.contains("line 290"));
        // Should have truncation marker
        assert!(out.contains("... +"));
        assert!(out.contains("lines"));
    }

    #[test]
    fn test_smart_truncate_short_content_unchanged() {
        let content = "line1\nline2\nline3";
        let out = smart_truncate(content, 4096);
        assert_eq!(out, content);
    }
}
