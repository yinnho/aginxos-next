//! Tool module framework.
//!
//! Each tool category implements `ToolModule` and provides both definitions
//! (for LLM tool schemas) and execution (the actual logic).

pub mod a2a;
pub mod agf_bridge;
pub mod agmem_bridge;
pub mod carrier_bridge;
pub mod agent;
pub mod automation;
pub mod collaboration;
pub mod data_analyze;
pub mod document;
pub mod gateway_hub;
pub mod knowledge;
pub mod media;
pub mod shell;
pub mod sqlite;
pub mod training;
pub mod web;
pub mod web_bridge;

use crate::kernel_handle::KernelHandle;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// web 工具（browser_* / web_search / web_fetch）：M31 曾外置 `aginx-web`
// CLI，2026-09-26 回迁进程内（tools/web/，aginxbrowser HTTP 客户端 +
// web_fetch 引擎）——设备缺包会让整线工具死亡的形态退役。工具面见
// web_bridge.rs；配置键仍是 AGINXBROWSER_URL（默认
// http://127.0.0.1:8089，设备上 aginxbrowser 恒在）。
//
// 文件面工具（file_* + image_analyze）：spawn `aginx-file` CLI
// （M32 D3 批2，原名 `agf`，D13 改姓 2026-09-26）。工具面见
// agf_bridge.rs；路径解析/沙箱留在桥内。
//
// 记忆面工具（kv_* / memory_tree / knowledge_* / flow_* /
// clone_evaluate）：spawn `aginx-mem` CLI（M35，原名 `agmem`，D13 改姓
// 2026-09-26）。工具面见 agmem_bridge.rs；knowledge.rs 只剩 apply_patch +
// session_summarize 两个内核耦合留守面。
// ---------------------------------------------------------------------------

/// A category of related tools.
///
/// Modules are tried in order; the first one returning `Some` handles the tool.
#[async_trait]
pub trait ToolModule: Send + Sync {
    /// Tool definitions exposed to the LLM.
    fn definitions(&self) -> Vec<ToolDefinition>;

    /// Try to execute a tool by name.
    ///
    /// Returns `Some(Ok(content))` if handled successfully,
    /// `Some(Err(message))` if handled but failed,
    /// `None` if this module doesn't handle the tool.
    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>>;

    /// Return the permission level for a tool in this module.
    ///
    /// Default: `Dangerous` (fail-safe — unknown tools require maximum trust).
    fn permission_level(&self, _tool_name: &str) -> PermissionLevel {
        PermissionLevel::Dangerous
    }

    /// Return the maximum result size in chars for a tool in this module.
    ///
    /// Default: `None` (no per-tool limit — dynamic context truncation applies).
    fn max_result_size_chars(&self, _tool_name: &str) -> Option<usize> {
        None
    }
}

/// All built-in tool modules in dispatch order.
pub fn builtin_modules(
    cli_exec_config: carrier_types::config::CliExecConfig,
) -> Vec<Box<dyn ToolModule>> {
    let mut modules: Vec<Box<dyn ToolModule>> = vec![
        Box::new(agf_bridge::AgfBridge),
        Box::new(agmem_bridge::AgmemBridge),
        Box::new(carrier_bridge::CarrierBridge),
        Box::new(document::DocumentTools),
        Box::new(sqlite::SqliteTools),
        Box::new(shell::ShellTools),
        Box::new(web_bridge::WebBridge),
        Box::new(knowledge::KnowledgeTools),
        Box::new(media::MediaTools),
        Box::new(agent::DelegationTools),
        Box::new(training::TrainingTools),
        Box::new(collaboration::CollaborationTools),
        Box::new(automation::AutomationRulesTools),
        Box::new(a2a::A2aTools),
        Box::new(gateway_hub::GatewayHubTools),
        Box::new(data_analyze::DataAnalyzeTools),
        Box::new(crate::api_tools::ApiToolRegisterModule),
    ];
    // Only register cli_exec if there are whitelisted commands configured.
    if !cli_exec_config.commands.is_empty() {
        modules.push(Box::new(shell::CliExecTools::new(cli_exec_config)));
    }

    // Load declarative API tools from api_tools.toml: global + every workspace's.
    // Each tool becomes a ToolDefinition visible to agents — no Rust code,
    // no CORE_TOOL_NAMES entry needed.
    //
    // Workspace tools MUST be loaded here (into the global module) so they have
    // executable ToolModule instances. messaging.rs surfaces tool *names* per
    // agent workspace, but matches them against builtin_tool_definitions() — so
    // without loading workspaces here, per-agent declarative tools (e.g.
    // quant-scout's quant_*) are named but never runnable, and agents fall back
    // to raw web_fetch on API URLs. Per-agent visibility is still gated by the
    // skill/manifest tools list downstream.
    let home_dir = carrier_types::config::home_dir();
    let mut api_tool_configs = crate::api_tools::loader::load_all_api_tools(&home_dir, None);
    let ws_root = home_dir.join("workflows");
    if let Ok(entries) = std::fs::read_dir(&ws_root) {
        for entry in entries.flatten() {
            let ws_toml = entry.path().join("api_tools.toml");
            if ws_toml.is_file() {
                let ws_tools = crate::api_tools::loader::load_api_tools_file(&ws_toml);
                let existing: std::collections::HashSet<String> =
                    api_tool_configs.iter().map(|t| t.name.clone()).collect();
                for tool in ws_tools {
                    if !existing.contains(&tool.name) {
                        api_tool_configs.push(tool);
                    }
                }
            }
        }
    }
    // Merge in dynamically registered tools (from api_tool_register)
    let dynamic = crate::api_tools::register::dynamic_tools();
    let static_names: std::collections::HashSet<String> =
        api_tool_configs.iter().map(|t| t.name.clone()).collect();
    for dt in dynamic {
        if !static_names.contains(&dt.name) {
            api_tool_configs.push(dt);
        }
    }
    if !api_tool_configs.is_empty() {
        modules.push(Box::new(crate::api_tools::DeclarativeApiModule::new(
            api_tool_configs,
        )));
    }

    modules
}

// ---------------------------------------------------------------------------
// Shared kernel helpers (used by multiple tool modules)
// ---------------------------------------------------------------------------

/// Require a kernel handle, returning an error if none is available.
pub(crate) fn require_kernel(
    kernel: Option<&Arc<dyn KernelHandle>>,
) -> CarrierResult<&Arc<dyn KernelHandle>> {
    kernel.ok_or_else(|| {
        CarrierError::Internal(
            "Kernel handle not available. Inter-agent tools require a running kernel.".to_string(),
        )
    })
}

/// Check that the inter-agent call depth has not exceeded the maximum.
pub(crate) fn check_call_depth() -> CarrierResult<()> {
    let current = crate::tool_runner::AGENT_CALL_DEPTH
        .try_with(|d| d.get())
        .unwrap_or(0);
    if current >= crate::tool_runner::MAX_AGENT_CALL_DEPTH {
        Err(CarrierError::Internal(format!(
            "Agent call depth exceeded (max {}). Use the task queue instead.",
            crate::tool_runner::MAX_AGENT_CALL_DEPTH
        )))
    } else {
        Ok(())
    }
}

/// Resolve a target clone's workspace root via kernel.
pub(crate) fn resolve_target_workspace(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
) -> CarrierResult<PathBuf> {
    let kh = kernel.ok_or(CarrierError::Internal(
        "train_* tools require kernel access".to_string(),
    ))?;
    let target = input["target"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'target' parameter (target clone name)".to_string(),
    ))?;

    let target_workspace = kh.resolve_agent_workspace(target).ok_or_else(|| {
        CarrierError::InvalidInput(format!("Agent '{}' not found or has no workspace", target))
    })?;

    let path = PathBuf::from(&target_workspace);
    if !path.exists() {
        return Err(CarrierError::InvalidInput(format!(
            "Workspace for '{}' does not exist: {}",
            target, target_workspace
        )));
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Shared path validation utilities (used by multiple tool modules)
// ---------------------------------------------------------------------------

/// Reject path traversal attempts and absolute paths.
pub fn validate_path(path: &str) -> CarrierResult<&str> {
    for component in std::path::Path::new(path).components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(CarrierError::InvalidInput(
                    "Path traversal denied: '..' components are forbidden".to_string(),
                ));
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(CarrierError::InvalidInput(
                    "Absolute paths are forbidden".to_string(),
                ));
            }
            _ => {}
        }
    }
    Ok(path)
}

/// Sanitize a string before using it as a single path component.
pub fn sanitize_path_component(name: &str) -> CarrierResult<&str> {
    if name.is_empty() {
        return Err(CarrierError::InvalidInput(
            "Empty path component".to_string(),
        ));
    }
    if name.contains('/') || name.contains('\\') || name == ".." || name.contains("..") {
        return Err(CarrierError::InvalidInput(format!(
            "Invalid path component: {:?}",
            name
        )));
    }
    for component in std::path::Path::new(name).components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(CarrierError::InvalidInput(format!(
                    "Path traversal denied in component: {:?}",
                    name
                )));
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(CarrierError::InvalidInput(format!(
                    "Absolute path denied in component: {:?}",
                    name
                )));
            }
            _ => {}
        }
    }
    Ok(name)
}

/// Validate a clone name: only lowercase alphanumeric and hyphens allowed.
pub fn validate_clone_name(name: &str) -> CarrierResult<&str> {
    if name.is_empty() {
        return Err(CarrierError::InvalidInput(
            "Clone name cannot be empty".to_string(),
        ));
    }
    if name.len() > 64 {
        return Err(CarrierError::InvalidInput(
            "Clone name too long (max 64 characters)".to_string(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(CarrierError::InvalidInput(
            "Clone name cannot start or end with a hyphen".to_string(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(CarrierError::InvalidInput(
            "Clone name must contain only lowercase letters, digits, and hyphens (e.g. 'customer-support')".to_string()
        ));
    }
    Ok(name)
}

/// Resolve a file path through the workspace sandbox (if available) or legacy validation.
pub fn resolve_file_path(raw_path: &str, workspace_root: Option<&Path>) -> CarrierResult<PathBuf> {
    if let Some(root) = workspace_root {
        crate::workspace_sandbox::resolve_sandbox_path(raw_path, root)
    } else {
        let _ = validate_path(raw_path)?;
        Ok(PathBuf::from(raw_path))
    }
}

/// Resolve a file read path through the workspace sandbox with sender_id-aware rewriting.
pub fn resolve_file_path_for_read(
    raw_path: &str,
    workspace_root: Option<&Path>,
    sender_id: Option<&str>,
    agent_name: Option<&str>,
) -> CarrierResult<PathBuf> {
    if let Some(root) = workspace_root {
        crate::workspace_sandbox::resolve_sandbox_path_for_read(
            raw_path, root, sender_id, agent_name,
        )
    } else {
        let _ = validate_path(raw_path)?;
        Ok(PathBuf::from(raw_path))
    }
}
