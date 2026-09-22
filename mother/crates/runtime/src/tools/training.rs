//! Cross-workspace training tools (for trainer agents).

use super::ToolModule;
use crate::kernel_handle::KernelHandle;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};

// ---------------------------------------------------------------------------
// Cross-workspace training tools (for trainer agents)
// ---------------------------------------------------------------------------

async fn tool_train_read(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let target_root = crate::tools::resolve_target_workspace(input, kernel)?;
    let path = input["path"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'path' parameter".to_string(),
    ))?;
    let full_path = crate::workspace_sandbox::resolve_sandbox_path(path, &target_root)?;
    tokio::fs::read_to_string(&full_path)
        .await
        .map_err(CarrierError::Io)
}

async fn tool_train_write(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let target_root = crate::tools::resolve_target_workspace(input, kernel)?;
    let path = input["path"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'path' parameter".to_string(),
    ))?;
    let content = input["content"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'content' parameter".to_string(),
    ))?;
    let full_path = crate::workspace_sandbox::resolve_sandbox_path(path, &target_root)?;
    if let Some(parent) = full_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(CarrierError::Io)?;
    }
    tokio::fs::write(&full_path, content)
        .await
        .map_err(CarrierError::Io)?;
    Ok(format!(
        "Successfully wrote {} bytes to {}",
        content.len(),
        path
    ))
}

async fn tool_train_list(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let target_root = crate::tools::resolve_target_workspace(input, kernel)?;
    let sub_path = input["path"].as_str().unwrap_or(".");
    let full_path = crate::workspace_sandbox::resolve_sandbox_path(sub_path, &target_root)?;
    let mut entries = tokio::fs::read_dir(&full_path)
        .await
        .map_err(CarrierError::Io)?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(CarrierError::Io)? {
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata().await;
        let suffix = match metadata {
            Ok(m) if m.is_dir() => "/",
            _ => "",
        };
        files.push(format!("{name}{suffix}"));
    }
    files.sort();
    Ok(files.join("\n"))
}

async fn tool_train_evaluate(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let target_root = crate::tools::resolve_target_workspace(input, kernel)?;
    // clone_evaluate 的实现已搬进 agmem CLI（M35）；train 面解析的是
    // 目标分身的 workspace（kernel 侧），所以这里直调 lifecycle 的
    // 确定性打分——输出格式与 agmem clone_evaluate 逐字一致。
    let metrics = carrier_lifecycle::evaluate::compute_deterministic_metrics(&target_root);
    Ok(format!(
        "Quality Score: {}/100 ({})\nKnowledge: {} files, {} bytes\nSkills: {}\nIdentity: SOUL={}, SP={}, MEMORY={}",
        metrics.score,
        metrics.grade,
        metrics.knowledge_files,
        metrics.knowledge_total_bytes,
        metrics.flow_count,
        metrics.has_soul,
        metrics.has_system_prompt,
        metrics.has_memory,
    ))
}

// ---------------------------------------------------------------------------
// Clone lifecycle tools (for clone-creator flow): publish / export
//
// `clone_install` was RETIRED (2026-08-25): forcing the whole definition layer
// through one giant tool payload was the root cause of generation-turn
// failures (timeout → regenerate → compaction loss). Installation now happens
// kernel-side: the clone-creator writes files incrementally into its own
// `staging/<name>/` and emits a `[CLONE_INSTALL:<name>]` reply marker; the
// kernel messaging layer executes the install at turn end
// (kernel/src/clone_marker.rs).
// ---------------------------------------------------------------------------

/// `clone_publish`: push an installed clone's definition layer to DupHub.
///
/// Input `{ name }`. Resolves the clone's workspace, collects its
/// definition-layer files (excluding runtime dirs and `.dup/`), and pushes via
/// the file-level dup endpoint using the configured Hub url + api_key.
async fn tool_clone_publish(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let name = input["name"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'name' parameter".to_string()))?;
    crate::tools::validate_clone_name(name)?;

    let ws_str = kh.resolve_agent_workspace(name).ok_or_else(|| {
        CarrierError::InvalidInput(format!("Clone '{name}' not found or has no workspace"))
    })?;
    let ws = std::path::PathBuf::from(&ws_str);

    let mut files = carrier_clone::manifest::collect_definition_files(&ws)
        .map_err(|e| CarrierError::Internal(format!("Failed to collect definition files: {e}")))?;
    // Ensure template.json has a `version` field - DupHub requires it to extract
    // listing metadata (description/display_name/category). Safe for clones
    // installed before the clone_install safeguard added it.
    let added_version = carrier_clone::manifest::ensure_template_version(&mut files);
    // Compute the manifest hash from the (possibly modified) files so it matches
    // exactly what we push - build_manifest reads the workspace, which may not
    // yet reflect the version we just injected, so derive the hash here.
    let file_hashes: std::collections::BTreeMap<String, String> = files
        .iter()
        .map(|(p, b)| (p.clone(), carrier_clone::manifest::sha256_hex(b)))
        .collect();
    let hash = carrier_clone::manifest::manifest_hash(&file_hashes);

    let (hub_url, api_key) = kh.clone_hub_config().ok_or_else(|| {
        CarrierError::Internal("Hub not configured (hub.url / api_key missing)".into())
    })?;

    let template_name =
        carrier_clone::hub::push_dup_files(&hub_url, &api_key, name, &files, &hash, None, None)
            .await
            .map_err(|e| CarrierError::Network(format!("Failed to push to Hub: {e}")))?;

    let short_hash = &hash[..hash.len().min(12)];
    Ok(format!(
        "已推送到 DupHub: {template_name}（{} 文件，hash={short_hash}{}）",
        files.len(),
        if added_version {
            "，已补 version 字段"
        } else {
            ""
        }
    ))
}

/// `clone_export`: list an installed clone's definition-layer manifest.
///
/// Input `{ name }`. Returns the manifest (file listing + state hash + sizes)
/// for the clone's definition layer — the file-level equivalent of an archive
/// summary.
async fn tool_clone_export(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let name = input["name"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'name' parameter".to_string()))?;
    crate::tools::validate_clone_name(name)?;

    let ws_str = kh.resolve_agent_workspace(name).ok_or_else(|| {
        CarrierError::InvalidInput(format!("Clone '{name}' not found or has no workspace"))
    })?;
    let ws = std::path::PathBuf::from(&ws_str);

    let files = carrier_clone::manifest::collect_definition_files(&ws)
        .map_err(|e| CarrierError::Internal(format!("Failed to collect definition files: {e}")))?;
    let manifest = carrier_clone::manifest::build_manifest(&ws)
        .map_err(|e| CarrierError::Internal(format!("Failed to build manifest: {e}")))?;
    let total_bytes: usize = files.values().map(|b| b.len()).sum();

    let mut paths: Vec<&String> = manifest.files.keys().collect();
    paths.sort();
    Ok(format!(
        "分身 '{name}' 定义层清单：{} 文件，{} 字节，hash={}\n{}",
        files.len(),
        total_bytes,
        manifest.hash,
        paths
            .iter()
            .map(|p| format!("- {p}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

// ---------------------------------------------------------------------------
// ToolModule implementation
// ---------------------------------------------------------------------------

/// Cross-workspace training tools (for trainer agents).
pub struct TrainingTools;

#[async_trait]
impl ToolModule for TrainingTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "train_read".to_string(),
                description: "Read a file from a target clone's workspace. Used by trainer agents to inspect other clones.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {"type": "string", "description": "Name of the target clone to read from"},
                        "path": {"type": "string", "description": "File path relative to the target clone's workspace root"},
                    },
                    "required": ["target", "path"],
                }),
            },
            ToolDefinition {
                name: "train_write".to_string(),
                description: "Write a file to a target clone's workspace. Can modify any file including SOUL.md, system_prompt.md, agent.toml, and flows. Used by trainer agents to train other clones.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {"type": "string", "description": "Name of the target clone to write to"},
                        "path": {"type": "string", "description": "File path relative to the target clone's workspace root"},
                        "content": {"type": "string", "description": "File content to write"},
                    },
                    "required": ["target", "path", "content"],
                }),
            },
            ToolDefinition {
                name: "train_list".to_string(),
                description: "List files in a target clone's workspace directory. Used by trainer agents to explore other clones.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {"type": "string", "description": "Name of the target clone"},
                        "path": {"type": "string", "description": "Directory path relative to the target clone's workspace root (default: '.')"},
                    },
                    "required": ["target"],
                }),
            },
            ToolDefinition {
                name: "train_evaluate".to_string(),
                description: "Evaluate a target clone's quality with deterministic metrics. Returns score (0-100), knowledge stats, flow count, and identity completeness.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": {"type": "string", "description": "Name of the target clone to evaluate"},
                    },
                    "required": ["target"],
                }),
            },
            ToolDefinition {
                name: "clone_publish".to_string(),
                description: "Push an installed clone's definition-layer files to DupHub via the file-level dup endpoint. Requires hub.url + api_key configured in config.toml. Returns the template name and state hash.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Name of the installed clone to publish"},
                    },
                    "required": ["name"],
                }),
            },
            ToolDefinition {
                name: "clone_export".to_string(),
                description: "List an installed clone's definition-layer manifest (file paths, total size, state hash). Read-only — the file-level equivalent of an archive summary.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Name of the installed clone to export"},
                    },
                    "required": ["name"],
                }),
            },
        ]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        let kernel = ctx.kernel;
        let caller_agent_id = ctx.caller_agent_id;

        match name {
            "train_read" => Some(tool_train_read(input, kernel, caller_agent_id).await),
            "train_write" => Some(tool_train_write(input, kernel, caller_agent_id).await),
            "train_list" => Some(tool_train_list(input, kernel, caller_agent_id).await),
            "train_evaluate" => Some(tool_train_evaluate(input, kernel, caller_agent_id).await),
            "clone_publish" => Some(tool_clone_publish(input, kernel, caller_agent_id).await),
            "clone_export" => Some(tool_clone_export(input, kernel, caller_agent_id).await),
            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "train_read" | "train_list" | "train_evaluate" => PermissionLevel::None,
            "train_write" => PermissionLevel::Write,
            "clone_publish" => PermissionLevel::Write,
            "clone_export" => PermissionLevel::None,
            _ => PermissionLevel::Dangerous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_lifecycle_tools_registered() {
        let tools = TrainingTools;
        let names: Vec<String> = tools.definitions().into_iter().map(|d| d.name).collect();
        // clone_install was RETIRED (kernel-side [CLONE_INSTALL] marker takes
        // over); asserting its absence guards against accidental re-registration.
        assert!(!names.contains(&"clone_install".to_string()));
        assert!(names.contains(&"clone_publish".to_string()));
        assert!(names.contains(&"clone_export".to_string()));
    }

    #[test]
    fn clone_lifecycle_permissions() {
        let tools = TrainingTools;
        assert_eq!(
            tools.permission_level("clone_publish"),
            PermissionLevel::Write
        );
        assert_eq!(
            tools.permission_level("clone_export"),
            PermissionLevel::None
        );
        // Unknown tools still fail-safe to Dangerous.
        assert_eq!(
            tools.permission_level("clone_unknown"),
            PermissionLevel::Dangerous
        );
    }

    #[tokio::test]
    async fn clone_publish_requires_kernel() {
        let input = serde_json::json!({"name": "liang-shuming"});
        let res = tool_clone_publish(&input, None, None).await;
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("Kernel handle not available"), "got: {msg}");
    }

    #[tokio::test]
    async fn clone_export_requires_kernel() {
        let input = serde_json::json!({"name": "liang-shuming"});
        let res = tool_clone_export(&input, None, None).await;
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("Kernel handle not available"), "got: {msg}");
    }
}
