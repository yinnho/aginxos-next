//! agf 桥 — 文件面工具的外置实现桥（M32 D3 批2）。
//!
//! 实现已整体搬到 `agf` CLI（crates/agf，单真源）：file_read / file_write /
//! file_list / file_convert 四件来自 tools/filesystem.rs，image_analyze 来自
//! tools/media.rs。本模块只留：
//! - definitions()：与被删模块**逐字节相同**的 ToolDefinition（名字/schema/
//!   description 不动——flow `tools:` 加载期冻结、CORE_TOOL_NAMES、教学文本
//!   全部依赖这批名字）。
//! - 路径解析：沙箱与用户数据目录路由留在 kernel 侧（单真源在此，§9 计划
//!   随末模块退役），解析结果经 stdin JSON 保留键 `_ctx` 注入 CLI。
//! - execute()：spawn `agf tool <name>`，stdin 喂入参 JSON（含 `_ctx`），
//!   stdout 收 D1 信封（{"ok":true,"data":…} / {"ok":false,"error":…}）。
//!
//! 语义：定义恒广播；执行在 agf 未安装时干净报错（包在场门执行，不门
//! 广告——flow 冻结不因少包漂移；与 M31 web 桥同款）。
//!
//! 截断策略不变：file_read 的 50k 结果帽在 tool_meta（按工具名），随桥
//! 保留——信封只运字符串，不重复截一次。

use super::ToolModule;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub struct AgfBridge;

/// 桥承载的全部工具名（与 agf::TOOL_NAMES ���一对应）。
pub const BRIDGE_TOOL_NAMES: &[&str] = &[
    "file_read",
    "file_write",
    "file_list",
    "file_convert",
    "image_analyze",
];

#[async_trait]
impl ToolModule for AgfBridge {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "file_read".to_string(),
                description: "Read the contents of a file. Paths are relative to the agent workspace.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "The file path to read" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "file_write".to_string(),
                description: "Write content to a file. Use 'output/' prefix for user-specific task outputs (articles, reports, drafts, generated content). Use 'memory/' prefix for user-specific private notes. Paths are sandboxed per-user automatically. On success the result includes view_url — paste that link so the user can open the file in a browser.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "The file path to write to" },
                        "content": { "type": "string", "description": "The content to write to the file" }
                    },
                    "required": ["path", "content"]
                }),
            },
            ToolDefinition {
                name: "file_list".to_string(),
                description: "List files in a directory. Paths are relative to the agent workspace.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "The directory path to list" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "file_convert".to_string(),
                description: "Convert a document between formats using Pandoc. Supported formats: markdown, html, docx, pdf, rst, latex, etc.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "input_path": { "type": "string", "description": "Path to the input file" },
                        "output_format": { "type": "string", "description": "Target format (e.g. 'pdf', 'docx', 'html')" },
                        "output_path": { "type": "string", "description": "Optional output path. Auto-generated if not provided." }
                    },
                    "required": ["input_path", "output_format"]
                }),
            },
            ToolDefinition {
                name: "image_analyze".to_string(),
                description: "Analyze an image file — returns format, dimensions, file size, and a base64 preview. For vision-model analysis, include a prompt.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the image file" },
                        "prompt": { "type": "string", "description": "Optional prompt for vision analysis (e.g., 'Describe what you see')" }
                    },
                    "required": ["path"]
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
        if !BRIDGE_TOOL_NAMES.contains(&name) {
            return None;
        }
        Some(run_agf_tool(name, input, ctx).await)
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "file_read" | "file_list" | "file_convert" | "image_analyze" => {
                PermissionLevel::ReadOnly
            }
            "file_write" => PermissionLevel::Write,
            _ => PermissionLevel::Dangerous,
        }
    }
}

// ---------------------------------------------------------------------------
// 路径解析（自被删的 tools/filessystem.rs / media.rs 原样搬来）
// ---------------------------------------------------------------------------

/// Resolve output/memory (and catch-all) paths to the top-level senders directory.
///
/// Returns `None` if the path is a workspace-internal path (knowledge/, flows/, etc.)
/// that should be handled by the sandbox instead.
pub(crate) fn resolve_user_data_path(
    raw_path: &str,
    home_dir: &Path,
    sender_id: &str,
    owner_id: Option<&str>,
    agent_name: &str,
) -> Option<CarrierResult<PathBuf>> {
    // Absolute paths — delegate to the workspace sandbox, which strips the
    // workspace_root prefix and canonicalizes.  We MUST NOT strip the leading
    // slash ourselves (that would turn "/home/…/output/file.md" into
    // "home/…/output/file.md" and join it under the sender's output dir,
    // creating a malformed nested path).
    let normalized = raw_path.replace('\\', "/");
    if normalized.starts_with('/') {
        return None;
    }
    let rel = normalized.trim_start_matches('/');

    // Determine subdirectory and rest-of-path from the user's input
    let (subdir, rest) = if rel.starts_with("output/") || rel == "output" {
        let rest = rel.strip_prefix("output").unwrap_or("");
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        ("output", rest)
    } else if rel.starts_with("memory/") || rel == "memory" {
        let rest = rel.strip_prefix("memory").unwrap_or("");
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        ("memory", rest)
    } else if rel.starts_with("input/") || rel == "input" {
        // input/ holds files the user sent to the agent (saved by the channel
        // bridge into senders/{sender}/input/). Route there so file_read /
        // file_list / file_convert can read received attachments. Writes to
        // input/ are blocked in agf's file_write to protect received files.
        let rest = rel.strip_prefix("input").unwrap_or("");
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        ("input", rest)
    } else if crate::workspace_sandbox::is_internal_path(rel) {
        // Internal paths go through sandbox
        return None;
    } else {
        // Catch-all: non-internal paths go to output/
        ("output", rel)
    };

    // Validate no path traversal
    if let Err(e) = super::validate_path(rel) {
        return Some(Err(e));
    }

    let oid = owner_id.unwrap_or(sender_id);
    let base = carrier_types::config::sender_data_dir(home_dir, oid, agent_name, Some(sender_id));
    let target = if rest.is_empty() {
        base.join(subdir)
    } else {
        base.join(subdir).join(rest)
    };

    Some(Ok(target))
}

/// file_read / file_list 的解析（用户数据路由 → 读沙箱兜底）。
fn resolve_read_path(raw_path: &str, ctx: &ToolContext<'_>) -> CarrierResult<PathBuf> {
    if let (Some(hd), Some(sid), Some(an)) = (ctx.home_dir, ctx.sender_id, ctx.agent_name) {
        match resolve_user_data_path(raw_path, hd, sid, ctx.owner_id, an) {
            Some(Ok(path)) => Ok(path),
            Some(Err(e)) => Err(e),
            None => {
                // Internal path — go through sandbox
                super::resolve_file_path_for_read(
                    raw_path,
                    ctx.workspace_root,
                    ctx.sender_id,
                    ctx.agent_name,
                )
            }
        }
    } else {
        super::resolve_file_path_for_read(
            raw_path,
            ctx.workspace_root,
            ctx.sender_id,
            ctx.agent_name,
        )
    }
}

/// file_write 的解析（用户数据路由 → 写沙箱兜底，含 is_clone_admin）。
fn resolve_write_path(raw_path: &str, ctx: &ToolContext<'_>) -> CarrierResult<PathBuf> {
    if let (Some(hd), Some(sid), Some(an)) = (ctx.home_dir, ctx.sender_id, ctx.agent_name) {
        match resolve_user_data_path(raw_path, hd, sid, ctx.owner_id, an) {
            Some(Ok(path)) => Ok(path),
            Some(Err(e)) => Err(e),
            None => {
                // Internal path — go through sandbox
                if let Some(root) = ctx.workspace_root {
                    crate::workspace_sandbox::resolve_sandbox_path_for_write(
                        raw_path,
                        root,
                        ctx.sender_id,
                        ctx.agent_name,
                        ctx.is_clone_admin,
                    )
                } else {
                    let _ = super::validate_path(raw_path)?;
                    Ok(PathBuf::from(raw_path))
                }
            }
        }
    } else if let Some(root) = ctx.workspace_root {
        crate::workspace_sandbox::resolve_sandbox_path_for_write(
            raw_path,
            root,
            ctx.sender_id,
            ctx.agent_name,
            ctx.is_clone_admin,
        )
    } else {
        let _ = super::validate_path(raw_path)?;
        Ok(PathBuf::from(raw_path))
    }
}

/// image_analyze 的解析（绝对路径直接用；用户数据前缀/子串 → sender 目录；
/// 其余读沙箱）。逐字节搬自 media.rs 的 tool_image_analyze 解析块。
fn resolve_analyze_path(path: &str, ctx: &ToolContext<'_>) -> CarrierResult<PathBuf> {
    if path.starts_with("/tmp/") || path.starts_with('/') {
        Ok(PathBuf::from(path))
    } else if let (Some(hd), Some(sid), Some(an)) = (ctx.home_dir, ctx.sender_id, ctx.agent_name)
    {
        let rel = path.replace('\\', "/");
        let rel = rel.trim_start_matches('/');
        if rel.starts_with("input/") || rel.starts_with("output/") || rel.starts_with("memory/") {
            let oid = ctx.owner_id.unwrap_or(sid);
            Ok(carrier_types::config::sender_data_dir(hd, oid, an, Some(sid)).join(rel))
        } else if let Some(idx) = rel.find("input/") {
            let oid = ctx.owner_id.unwrap_or(sid);
            Ok(carrier_types::config::sender_data_dir(hd, oid, an, Some(sid)).join(&rel[idx..]))
        } else {
            super::resolve_file_path_for_read(
                path,
                ctx.workspace_root,
                ctx.sender_id,
                ctx.agent_name,
            )
        }
    } else {
        super::resolve_file_path_for_read(
            path,
            ctx.workspace_root,
            ctx.sender_id,
            ctx.agent_name,
        )
    }
}

// ---------------------------------------------------------------------------
// spawn + 信封解包（web 桥同款）
// ---------------------------------------------------------------------------

/// Spawn `agf tool <name>`（stdin=入参 JSON 含 `_ctx`，stdout=D1 信封）。
///
/// sandbox 同 web 桥：env_clear 后只回 PATH/HOME 等 SAFE_ENV_VARS —— agf
/// 的 markitdown/pandoc 子进程靠 PATH。kill_on_drop：超时/取消不留孤儿。
/// 相对路径依赖 CWD 继承（与 runtime 进程同 CWD，语义同旧模块）。
async fn run_agf_tool(name: &str, input: &Value, ctx: &ToolContext<'_>) -> CarrierResult<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    // 入参克隆 + `_ctx` 注入：身份（view_url / 自动输出目录）+ 预解析路径
    // （沙箱权威在本模块；CLI 只认 `_ctx.resolved.<param>`）。
    let mut payload = input.clone();
    {
        let c = payload.as_object_mut().unwrap().entry("_ctx").or_insert_with(|| serde_json::json!({}));
        let c = c.as_object_mut().unwrap();
        if let Some(hd) = ctx.home_dir {
            c.insert("home_dir".into(), serde_json::json!(hd.display().to_string()));
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
        if let Some(eu) = ctx.external_url {
            c.insert("external_url".into(), serde_json::json!(eu));
        }
        let resolved = c
            .entry("resolved")
            .or_insert_with(|| serde_json::json!({}));
        let resolved = resolved.as_object_mut().unwrap();
        match name {
            "file_read" | "file_list" | "image_analyze" => {
                if let Some(p) = input["path"].as_str() {
                    let r = if name == "image_analyze" {
                        resolve_analyze_path(p, ctx)
                    } else {
                        resolve_read_path(p, ctx)
                    }?;
                    resolved.insert("path".into(), serde_json::json!(r.display().to_string()));
                }
            }
            "file_write" => {
                if let Some(p) = input["path"].as_str() {
                    let r = resolve_write_path(p, ctx)?;
                    resolved.insert("path".into(), serde_json::json!(r.display().to_string()));
                }
            }
            "file_convert" => {
                if let Some(p) = input["input_path"].as_str() {
                    // 输入走工作区沙箱（与旧 file_convert 的 resolve_file_path
                    // 一致——不做 sender 改写）。
                    let r = super::resolve_file_path(p, ctx.workspace_root)?;
                    resolved.insert("input_path".into(), serde_json::json!(r.display().to_string()));
                }
                if let Some(op) = input["output_path"].as_str() {
                    let r = resolve_write_path(op, ctx)?;
                    resolved.insert("output_path".into(), serde_json::json!(r.display().to_string()));
                }
            }
            _ => {}
        }
    }

    let mut cmd = tokio::process::Command::new("agf");
    cmd.arg("tool").arg(name);
    crate::subprocess_sandbox::sandbox_command(&mut cmd, &[]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        CarrierError::Internal(format!(
            "agf CLI not available ({e}) — file tools live in the `agf` package. \
             Install it (`ag pkg install agf`) or check PATH."
        ))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort write; agf reads stdin to EOF before executing.
        let bytes = serde_json::to_vec(&payload).unwrap_or_default();
        let _ = stdin.write_all(&bytes).await;
        let _ = stdin.shutdown().await;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| CarrierError::Internal(format!("agf tool {name} subprocess failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let envelope: Value = serde_json::from_str(&stdout).map_err(|_| {
        let tail = if stdout.is_empty() { stderr } else { stdout };
        let preview = crate::str_utils::safe_truncate_str(&tail, 300);
        CarrierError::Internal(format!(
            "agf tool {name} returned a non-JSON response (exit {:?}): {preview}",
            output.status.code()
        ))
    })?;

    if envelope["ok"].as_bool().unwrap_or(false) {
        envelope["data"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                CarrierError::Serialization("agf envelope missing string data field".to_string())
            })
    } else {
        let msg = envelope["error"].as_str().unwrap_or("unknown agf error");
        Err(CarrierError::Internal(msg.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_match_bridge_names() {
        let bridge = AgfBridge;
        let defs = bridge.definitions();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        let mut expected = BRIDGE_TOOL_NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
    }

    #[test]
    fn permission_levels_match_legacy() {
        let bridge = AgfBridge;
        assert!(matches!(bridge.permission_level("file_read"), PermissionLevel::ReadOnly));
        assert!(matches!(bridge.permission_level("file_list"), PermissionLevel::ReadOnly));
        assert!(matches!(bridge.permission_level("file_convert"), PermissionLevel::ReadOnly));
        assert!(matches!(bridge.permission_level("image_analyze"), PermissionLevel::ReadOnly));
        assert!(matches!(bridge.permission_level("file_write"), PermissionLevel::Write));
    }

    #[test]
    fn input_path_resolves_to_sender_input_dir() {
        // Files the user sent are saved by the bridge into
        // senders/{sender}/input/. file_read/file_list must resolve input/
        // there (not into output/input/ as the old catch-all did).
        let h = PathBuf::from("/tmp/oc-fs-test-home");
        let sender = "u1@im.wechat";
        let p = resolve_user_data_path("input/为.md", &h, sender, None, "mo-catering-ops")
            .expect("input/ should resolve (not internal)")
            .expect("path should be ok");
        let expected = h
            .join("workspaces")
            .join("mo-catering-ops")
            .join("senders")
            .join(sender)
            .join("input")
            .join("为.md");
        assert_eq!(p, expected);
    }

    #[test]
    fn output_memory_and_catchall_unchanged() {
        // Regression: existing output/ / memory/ / catch-all routing must not
        // change when moving to the bridge.
        let h = PathBuf::from("/tmp/oc-fs-test-home");
        let p_out = resolve_user_data_path("output/r.md", &h, "u1", None, "ag")
            .unwrap()
            .unwrap();
        assert_eq!(p_out, h.join("workspaces/ag/senders/u1/output/r.md"));

        let p_mem = resolve_user_data_path("memory/n.md", &h, "u1", None, "ag")
            .unwrap()
            .unwrap();
        assert_eq!(p_mem, h.join("workspaces/ag/senders/u1/memory/n.md"));

        // catch-all (no recognized prefix) still goes to output/
        let p_catch = resolve_user_data_path("foo.md", &h, "u1", None, "ag")
            .unwrap()
            .unwrap();
        assert_eq!(p_catch, h.join("workspaces/ag/senders/u1/output/foo.md"));
    }

    #[tokio::test]
    async fn unknown_name_is_none() {
        let bridge = AgfBridge;
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
