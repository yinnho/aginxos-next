//! Knowledge module — 内核耦合留守面（M35c 之后只剩两个工具）。
//!
//! knowledge_list/read/add/update/remove/import/lint/heal/extract/index、
//! clone_evaluate、flow_create/update/load 已整体搬到 `agmem` CLI（单真源，
//! 见 tools/agmem_bridge.rs 与 crates/agmem）。留守本模块的是两个吃
//! ToolContext 内核状态、无法下沉到子进程的工具：
//! - apply_patch：补丁路径要过 agf_bridge 的 sender 域路由（与
//!   file_write 同一解析，2026-08-21 86bus 路径分叉事故的修复面）。
//! - session_summarize：吃轮次身份（caller_agent/sender）与守护内记忆
//!   句柄，落 kv 的 session_summary 键。

use super::ToolModule;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

/// Patch 与 session 工具（内核耦合留守面）。
pub struct KnowledgeTools;

#[async_trait]
impl ToolModule for KnowledgeTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "apply_patch".to_string(),
                description: "Apply a multi-hunk diff patch to add, update, move, or delete files. Use this for targeted edits instead of full file overwrites.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "patch": {
                            "type": "string",
                            "description": "The patch in *** Begin Patch / *** End Patch format. Use *** Add File:, *** Update File:, *** Delete File: markers. Hunks use @@ headers with space (context), - (remove), + (add) prefixed lines."
                        }
                    },
                    "required": ["patch"]
                }),
            },
            ToolDefinition {
                name: "session_summarize".to_string(),
                description: "Save a summary of the current conversation for future recall. Use after long or important conversations to preserve key points, decisions, and outcomes.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "summary": {"type": "string", "description": "Key points, decisions, and outcomes from this conversation"},
                    },
                    "required": ["summary"],
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
        match name {
            "apply_patch" => Some(tool_apply_patch(input, ctx.workspace_root, ctx).await),
            "session_summarize" => Some(
                tool_session_summarize(input, ctx.memory, ctx.caller_agent_id, ctx.sender_id)
                    .await,
            ),
            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "session_summarize" => PermissionLevel::None,
            "apply_patch" => PermissionLevel::Write,
            _ => PermissionLevel::Dangerous,
        }
    }
}

async fn tool_apply_patch(
    input: &serde_json::Value,
    workspace_root: Option<&Path>,
    ctx: &ToolContext<'_>,
) -> CarrierResult<String> {
    let patch_str = input["patch"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'patch' parameter".to_string(),
    ))?;
    let root = workspace_root.ok_or(CarrierError::Internal(
        "apply_patch requires a workspace root".to_string(),
    ))?;
    let mut ops = crate::apply_patch::parse_patch(patch_str)?;

    // Route user-data paths (output/, memory/, input/, catch-all) through the
    // same sender-scoped resolver as file_read/file_write, so a file written by
    // file_write can be read/patched/deleted by apply_patch in the same turn.
    // Without this, file_write lands in senders/<sender>/output/... while
    // apply_patch looks in <workspace>/output/... - a path fork the agent
    // cannot reconcile (2026-08-21 86bus stuck-turn incident).
    for op in &mut ops {
        let (path, move_to): (&mut String, Option<&mut String>) = match op {
            crate::apply_patch::PatchOp::AddFile { path, .. } => (path, None),
            crate::apply_patch::PatchOp::UpdateFile { path, move_to, .. } => {
                (path, move_to.as_mut())
            }
            crate::apply_patch::PatchOp::DeleteFile { path } => (path, None),
        };
        for p in std::iter::once(path).chain(move_to) {
            if p.contains('\u{FFFD}') {
                return Err(CarrierError::InvalidInput(format!(
                    "路径 '{p}' 含损坏字符（U+FFFD），无法寻址。请用干净的文件名（中文名或 ASCII 名）重写补丁。"
                )));
            }
            let normalized = p.replace('\\', "/");
            if normalized == "input" || normalized.starts_with("input/") {
                return Err(CarrierError::InvalidInput(
                    "input/ 是用户发来的文件收件箱（只读），请改用 output/ 前缀写文件。"
                        .to_string(),
                ));
            }
            if let (Some(hd), Some(sid), Some(an)) = (ctx.home_dir, ctx.sender_id, ctx.agent_name) {
                if let Some(res) =
                    super::agf_bridge::resolve_user_data_path(p, hd, sid, ctx.owner_id, an)
                {
                    *p = res?.to_string_lossy().into_owned();
                }
            }
        }
    }

    let result = crate::apply_patch::apply_patch(&ops, root).await;
    if result.is_ok() {
        Ok(result.summary())
    } else {
        Err(CarrierError::Internal(format!(
            "Patch partially applied: {}. Errors: {}",
            result.summary(),
            result.errors.join("; ")
        )))
    }
}

async fn tool_session_summarize(
    input: &serde_json::Value,
    memory: Option<&Arc<dyn crate::memory_handle::MemoryHandle>>,
    caller_agent_id: Option<&str>,
    sender_id: Option<&str>,
) -> CarrierResult<String> {
    let mem = memory.ok_or(CarrierError::Internal(
        "session_summarize requires memory access".to_string(),
    ))?;
    let agent_id = caller_agent_id.ok_or(CarrierError::Internal(
        "session_summarize requires caller agent ID".to_string(),
    ))?;
    let sid = sender_id.unwrap_or("");
    let summary = input["summary"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'summary' parameter".to_string(),
    ))?;

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let key = format!("session_summary:{date}");

    mem.kv_set(
        agent_id,
        sid,
        sid,
        &key,
        serde_json::Value::String(summary.to_string()),
    )?;

    Ok(format!("Session summary stored for {date}."))
}

#[cfg(test)]
mod apply_patch_routing_tests {
    use super::*;
    use crate::tool_context::ToolContext;

    /// ToolContext with sender routing fields set, mirroring a sender-scoped
    /// chat turn (home_dir + sender_id + agent_name + workspace_root).
    fn sender_ctx<'a>(
        home: &'a std::path::Path,
        workspace: &'a std::path::Path,
    ) -> ToolContext<'a> {
        ToolContext {
            kernel: None,
            memory: None,
            caller_agent_id: None,
            mcp_connections: None,
            allowed_env_vars: None,
            workspace_root: Some(workspace),
            brain: None,
            exec_policy: None,
            cli_exec_config: None,
            process_manager: None,
            sender_id: Some("u1"),
            owner_id: None,
            home_dir: Some(home),
            agent_name: Some("ag"),
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
    async fn apply_patch_routes_output_to_sender_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let workspace = home.join("workspaces/ag");
        std::fs::create_dir_all(&workspace).unwrap();

        // AddFile via patch -> must land in senders/u1/output/, not workspace/output/
        let ctx = sender_ctx(home, &workspace);
        let add = serde_json::json!({
            "patch": "*** Begin Patch\n*** Add File: output/p1/material.md\n+hello\n*** End Patch\n"
        });
        let out = tool_apply_patch(&add, Some(&workspace), &ctx)
            .await
            .unwrap();
        assert!(out.contains("added"), "{out}");
        let sender_file = home.join("workspaces/ag/senders/u1/output/p1/material.md");
        assert!(sender_file.exists(), "file must land in sender output dir");
        assert!(!workspace.join("output/p1/material.md").exists());

        // DeleteFile via patch on the same logical path must find it there too
        // (2026-08-21 86bus incident: delete failed with "No such file").
        let del = serde_json::json!({
            "patch": "*** Begin Patch\n*** Delete File: output/p1/material.md\n*** End Patch\n"
        });
        tool_apply_patch(&del, Some(&workspace), &ctx)
            .await
            .unwrap();
        assert!(
            !sender_file.exists(),
            "delete must remove the sender-scoped file"
        );
    }

    #[tokio::test]
    async fn apply_patch_rejects_replacement_char_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let workspace = home.join("workspaces/ag");
        std::fs::create_dir_all(&workspace).unwrap();
        let ctx = sender_ctx(home, &workspace);
        let bad = serde_json::json!({
            "patch": "*** Begin Patch\n*** Add File: output/\u{FFFD}\u{FFFD}材.md\n+x\n*** End Patch\n"
        });
        let err = tool_apply_patch(&bad, Some(&workspace), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("U+FFFD"), "{err}");
    }

    #[tokio::test]
    async fn apply_patch_rejects_input_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let workspace = home.join("workspaces/ag");
        std::fs::create_dir_all(&workspace).unwrap();
        let ctx = sender_ctx(home, &workspace);
        let bad = serde_json::json!({
            "patch": "*** Begin Patch\n*** Add File: input/recvd.md\n+x\n*** End Patch\n"
        });
        let err = tool_apply_patch(&bad, Some(&workspace), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("只读"), "{err}");
    }

    #[tokio::test]
    async fn apply_patch_internal_paths_stay_in_workspace() {
        // knowledge/ is an internal path - must NOT be rerouted to the sender dir.
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let workspace = home.join("workspaces/ag");
        std::fs::create_dir_all(workspace.join("knowledge")).unwrap();
        let ctx = sender_ctx(home, &workspace);
        let add = serde_json::json!({
            "patch": "*** Begin Patch\n*** Add File: knowledge/new.md\n+x\n*** End Patch\n"
        });
        tool_apply_patch(&add, Some(&workspace), &ctx)
            .await
            .unwrap();
        assert!(workspace.join("knowledge/new.md").exists());
        assert!(!home
            .join("workspaces/ag/senders/u1/output/knowledge/new.md")
            .exists());
    }
}
