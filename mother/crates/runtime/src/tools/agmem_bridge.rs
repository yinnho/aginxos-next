//! agmem 桥 — 记忆面工具的外置实现桥（M35c）。
//!
//! 实现已整体搬到 `agmem` CLI（crates/agmem，单真源）：kv_get / kv_set /
//! kv_list 来自 tools/kv.rs，memory_tree 来自 tools/memory.rs，
//! knowledge_* / clone_evaluate / flow_* 来自 tools/knowledge.rs。本模块
//! 只留：
//! - definitions()：与被删模块**逐字节相同**的 ToolDefinition（名字/schema/
//!   description 不动——flow `tools:` 加载期冻结、CORE_TOOL_NAMES、教学
//!   文本全部依赖这批名字）。
//! - 身份与库定位：`(agent, owner, user)` 三元组与 substrate 库路径的
//!   单真源在 kernel 侧（daemon 的 config.data_dir/carrier.db），CLI 直开
//!   同一 sqlite（WAL 并发安全，M35a spike 2026-09-03；notify.rs DB 直读
//!   同款先例）。身份经 stdin JSON 保留键 `_ctx` 注入；owner/user 的
//!   None 以显式 null 传（CLI 侧还原成 Option，各面回落与上游逐字一致：
//!   kv → ""，tree → "default"/不过滤）。
//! - execute()：spawn `agmem tool <name>`，stdin 喂入参 JSON（含 `_ctx`），
//!   stdout 收 D1 信封（{"ok":true,"data":…} / {"ok":false,"error":…}）。
//!
//! 留守 tools/knowledge.rs 的两个内核耦合面不经本桥：apply_patch（走
//! crate::apply_patch + agf_bridge ��� sender 路径路由）、session_summarize
//! （吃轮次身份与守护内记忆句柄）。
//!
//! 语义：定义恒广播；执行在 agmem 未安装时干净报错（包在场门执行，不门
//! 广告——flow 冻结不因少包漂移；与 M31 web / M32 agf 桥同款）。

use super::ToolModule;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;

pub struct AgmemBridge;

/// 桥承载的全部工具名（被删三模块的定义全集；apply_patch /
/// session_summarize 留守 knowledge.rs，不在此表。agmem CLI 的
/// TOOL_NAMES 是本表 + kv_delete 补位——CLI 面更宽，桥只广播旧名）。
pub const BRIDGE_TOOL_NAMES: &[&str] = &[
    "kv_get",
    "kv_set",
    "kv_list",
    "memory_tree",
    "knowledge_list",
    "knowledge_read",
    "knowledge_lint",
    "knowledge_heal",
    "knowledge_add",
    "knowledge_update",
    "knowledge_remove",
    "knowledge_import",
    "clone_evaluate",
    "knowledge_extract",
    "knowledge_index",
    "flow_create",
    "flow_update",
    "flow_load",
];

#[async_trait]
impl ToolModule for AgmemBridge {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "kv_get".to_string(),
                description: "Retrieve a value from your private key-value store by key. Your data is isolated per-agent and per-user.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "The key to look up"
                        }
                    },
                    "required": ["key"]
                }),
            },
            ToolDefinition {
                name: "kv_set".to_string(),
                description: "Store a key-value pair in your private key-value store. Overwrites any existing value for the same key. Your data is isolated per-agent and per-user.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "key": {
                            "type": "string",
                            "description": "The key to store under"
                        },
                        "value": {
                            "description": "The value to store (any JSON type)"
                        }
                    },
                    "required": ["key", "value"]
                }),
            },
            ToolDefinition {
                name: "kv_list".to_string(),
                description: "List all keys in your private key-value store, optionally filtered by prefix. Your data is isolated per-agent and per-user.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "prefix": {
                            "type": "string",
                            "description": "Optional prefix to filter keys (e.g. 'entity.' to list only entity keys)"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "memory_tree".to_string(),
                description: "Query the user's hierarchical memory tree. This is a retrospective index of already-ingested conversations, emails, and documents — NOT a live API for connected services.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "mode": {
                            "type": "string",
                            "enum": [
                                "search_entities",
                                "query_topic",
                                "query_source",
                                "query_global",
                                "drill_down",
                                "fetch_leaves",
                            ],
                            "description": "Which retrieval operation to run",
                        },
                        "query": {
                            "type": "string",
                            "description": "Search term (used by search_entities, query_topic, query_source, query_global)",
                        },
                        "entity_id": {
                            "type": "string",
                            "description": "Canonical entity ID from search_entities (used by query_topic)",
                        },
                        "source_id": {
                            "type": "string",
                            "description": "Source identifier to filter by (used by query_source)",
                        },
                        "source_kind": {
                            "type": "string",
                            "enum": ["chat", "email", "document"],
                            "description": "Source type filter (used by query_source)",
                        },
                        "kinds": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Entity kind filter (used by search_entities), e.g. [\"person\", \"email\"]",
                        },
                        "time_window_days": {
                            "type": "integer",
                            "description": "Only return memories from the last N days (used by query_source, query_global, query_topic)",
                        },
                        "node_id": {
                            "type": "string",
                            "description": "Summary node ID to expand (used by drill_down)",
                        },
                        "max_depth": {
                            "type": "integer",
                            "description": "Levels to walk in drill_down (default: 1, max: 3)",
                        },
                        "chunk_ids": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Chunk IDs to hydrate (used by fetch_leaves, max 20)",
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Max results to return (default varies by mode)",
                        },
                    },
                    "required": ["mode"],
                }),
            },
            ToolDefinition {
                name: "knowledge_list".to_string(),
                description: "List available knowledge files in the agent's knowledge base. Returns filenames with descriptions.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "knowledge_read".to_string(),
                description: "Read a specific knowledge file from the agent's knowledge base. Only files in knowledge/ are accessible.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "filename": { "type": "string", "description": "The knowledge file name (e.g., 'refund-policy.md')" }
                    },
                    "required": ["filename"]
                }),
            },
            ToolDefinition {
                name: "knowledge_lint".to_string(),
                description: "Check the health of the clone's knowledge base. Reports missing frontmatter, empty files, placeholder content, and other issues.".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDefinition {
                name: "knowledge_heal".to_string(),
                description: "Automatically fix knowledge base issues: remove empty files, rebuild MEMORY.md index, add missing frontmatter templates.".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDefinition {
                name: "knowledge_add".to_string(),
                description: "Save a long-term knowledge entry that ALL users share (e.g. policies, reference docs, how-to guides, facts). Do NOT use this for user-specific content like article drafts, reports, outlines, or task outputs — use file_write with an output/ path for those.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Short title for the knowledge entry"},
                        "content": {"type": "string", "description": "The knowledge content (markdown)"},
                    },
                    "required": ["title", "content"],
                }),
            },
            ToolDefinition {
                name: "knowledge_update".to_string(),
                description: "Update an EXISTING knowledge file in place (full content replacement). Use this to correct or supersede statements in old knowledge files instead of piling up new entries. First knowledge_read the file, edit, then pass the complete new content (must keep the --- frontmatter with name/description). Filename is fuzzy-matched like knowledge_read.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "filename": {"type": "string", "description": "Knowledge file to update (e.g. 'business-info.md'; fuzzy matched)"},
                        "content": {"type": "string", "description": "Complete new file content, starting with the --- frontmatter block"},
                    },
                    "required": ["filename", "content"],
                }),
            },
            ToolDefinition {
                name: "knowledge_remove".to_string(),
                description: "Remove a knowledge entry by filename (supports fuzzy matching).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "filename": {"type": "string", "description": "Filename or title to remove (fuzzy matched)"},
                    },
                    "required": ["filename"],
                }),
            },
            ToolDefinition {
                name: "knowledge_import".to_string(),
                description: "Import data into the clone's knowledge base. Supports FAQ (CSV/TSV), chat logs (JSON), and document text.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "data": {"type": "string", "description": "Raw data content to import"},
                        "data_type": {"type": "string", "description": "Data format: 'faq', 'chat', 'document', or 'auto' (default: auto)"},
                    },
                    "required": ["data"],
                }),
            },
            ToolDefinition {
                name: "clone_evaluate".to_string(),
                description: "Evaluate the clone's quality with deterministic metrics. Returns a score (0-100) based on identity completeness, knowledge richness, skills, and knowledge quality.".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDefinition {
                name: "knowledge_extract".to_string(),
                description: "Extract new knowledge from a conversation and save it to the knowledge base. Uses dual-layer format with timeline tracking and rebuilds MEMORY.md index. Use when you discover facts, rules, or preferences worth remembering.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": {"type": "string", "description": "Short title for the knowledge (English or pinyin preferred, used as filename)"},
                        "content": {"type": "string", "description": "The knowledge content to save (markdown)"},
                    },
                    "required": ["title", "content"],
                }),
            },
            ToolDefinition {
                name: "knowledge_index".to_string(),
                description: "Rebuild the knowledge index file (MEMORY.md) by scanning all knowledge files in knowledge/. Use after manually adding or removing knowledge files.".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDefinition {
                name: "flow_create".to_string(),
                description: "Create a new flow in the workspace flows/ directory. Flows are tool prescriptions: frontmatter tools: are auto-injected when the flow matches; body is the hard workflow. Prefer declaring concrete tool names in tools (the flow injection is the only discovery path).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Flow name (used as filename)"},
                        "description": {"type": "string", "description": "Brief description of when to activate this flow"},
                        "tools": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Tool names this flow needs (e.g. [\"file_read\", \"file_write\", \"web_search\"]). Injected automatically when the flow matches — these are guaranteed present, no discovery needed."
                        },
                        "toolsets": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Deprecated alias for tools. Prefer tools."
                        },
                        "body": {"type": "string", "description": "The flow content: hard rules, workflow steps, instructions (markdown)"},
                    },
                    "required": ["name", "body"],
                }),
            },
            ToolDefinition {
                name: "flow_update".to_string(),
                description: "Update an existing PRIVATE flow (body and/or tools: frontmatter) to固化 a proven tool path so next runs inject it without trial-and-error. Only workspace-private flows can be updated; shared system flows are READ-ONLY (no copy-on-write) — request a human to update shared flows, or use flow_create for a clone-specific variant.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Flow name to update"},
                        "body": {"type": "string", "description": "New flow body (replaces existing body; omit to keep body)"},
                        "tools": {
                            "type": "array",
                            "items": {"type": "string"},
                            "description": "Replace frontmatter tools: list with these names (proven tools to inject next time)"
                        },
                        "description": {"type": "string", "description": "Optional new frontmatter description"},
                    },
                    "required": ["name"],
                }),
            },
            ToolDefinition {
                name: "flow_load".to_string(),
                description: "Load the full content of a flow by name. Returns the complete flow file including frontmatter and body.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "Flow name to load"},
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
        if !BRIDGE_TOOL_NAMES.contains(&name) {
            return None;
        }
        // kv 面上游纪律：无 caller_agent 即拒（CLI 面不知道身份缺席，
        // 这个闸必须在桥侧）。tree/knowledge 面不吃 agent 身份。
        if matches!(name, "kv_get" | "kv_set" | "kv_list") && ctx.caller_agent_id.is_none() {
            return Some(Err(CarrierError::Internal(
                "No agent context".to_string(),
            )));
        }
        Some(run_agmem_tool(name, input, ctx).await)
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "kv_get" | "kv_list" | "memory_tree" | "knowledge_list" | "knowledge_read"
            | "flow_load" | "clone_evaluate" => PermissionLevel::None,
            "knowledge_lint" | "knowledge_index" | "knowledge_extract" => {
                PermissionLevel::ReadOnly
            }
            "kv_set" | "knowledge_add" | "knowledge_update" | "knowledge_remove"
            | "knowledge_import" | "knowledge_heal" | "flow_create" | "flow_update" => {
                PermissionLevel::Write
            }
            _ => PermissionLevel::Dangerous,
        }
    }
}

// ---------------------------------------------------------------------------
// spawn + 信封解包（web/agf 桥同款）
// ---------------------------------------------------------------------------

/// 组装入参 + `_ctx`（身份三元组 + 库/workspace 定位）。抽出来单测：
/// owner/user 的 None 必须以显式 null 下发（缺席会被 CLI 当人面默认）。
fn build_payload(name: &str, input: &Value, ctx: &ToolContext<'_>) -> Value {
    let _ = name;
    let mut payload = input.clone();
    let obj = payload.as_object_mut().unwrap();
    let mut c = serde_json::Map::new();
    if let Some(a) = ctx.caller_agent_id {
        c.insert("agent_id".into(), serde_json::json!(a));
    }
    c.insert(
        "owner_id".into(),
        ctx.owner_id
            .map(|s| serde_json::json!(s))
            .unwrap_or(Value::Null),
    );
    c.insert(
        "user_id".into(),
        ctx.sender_id
            .map(|s| serde_json::json!(s))
            .unwrap_or(Value::Null),
    );
    if let Some(hd) = ctx.home_dir {
        c.insert("home_dir".into(), serde_json::json!(hd.display().to_string()));
    }
    if let Some(ws) = ctx.workspace_root {
        c.insert(
            "workspace_root".into(),
            serde_json::json!(ws.display().to_string()),
        );
    }
    obj.insert("_ctx".into(), Value::Object(c));
    payload
}

/// Spawn `agmem tool <name>`（stdin=入参 JSON 含 `_ctx`，stdout=D1 信封）。
///
/// sandbox 同 web/agf 桥：env_clear 后只回 PATH/HOME 等 SAFE_ENV_VARS。
/// kill_on_drop：超时/取消不留孤儿。
async fn run_agmem_tool(name: &str, input: &Value, ctx: &ToolContext<'_>) -> CarrierResult<String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let payload = build_payload(name, input, ctx);

    let mut cmd = tokio::process::Command::new("agmem");
    cmd.arg("tool").arg(name);
    crate::subprocess_sandbox::sandbox_command(&mut cmd, &[]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| {
        CarrierError::Internal(format!(
            "agmem CLI not available ({e}) — memory tools live in the `agmem` package. \
             Install it (`ag pkg install agmem`) or check PATH."
        ))
    })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort write; agmem reads stdin to EOF before executing.
        let bytes = serde_json::to_vec(&payload).unwrap_or_default();
        let _ = stdin.write_all(&bytes).await;
        let _ = stdin.shutdown().await;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| CarrierError::Internal(format!("agmem tool {name} subprocess failed: {e}")))?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    let envelope: Value = serde_json::from_str(&stdout).map_err(|_| {
        let tail = if stdout.is_empty() { stderr } else { stdout };
        let preview = crate::str_utils::safe_truncate_str(&tail, 300);
        CarrierError::Internal(format!(
            "agmem tool {name} returned a non-JSON response (exit {:?}): {preview}",
            output.status.code()
        ))
    })?;

    if envelope["ok"].as_bool().unwrap_or(false) {
        envelope["data"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| {
                CarrierError::Serialization("agmem envelope missing string data field".to_string())
            })
    } else {
        let msg = envelope["error"].as_str().unwrap_or("unknown agmem error");
        Err(CarrierError::Internal(msg.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare_ctx<'a>() -> ToolContext<'a> {
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
            max_tool_level: PermissionLevel::Write,
            is_clone_admin: false,
            external_url: None,
            flow_elevated_tools: None,
            flow_shell_allow: None,
            flow_deny_tools: None,
            flow_allowed_tools: None,
        }
    }

    #[test]
    fn definitions_match_bridge_names() {
        let bridge = AgmemBridge;
        let defs = bridge.definitions();
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        names.sort_unstable();
        let mut expected = BRIDGE_TOOL_NAMES.to_vec();
        expected.sort_unstable();
        assert_eq!(names, expected);
        // 留守面不在桥上
        assert!(!expected.contains(&"apply_patch"));
        assert!(!expected.contains(&"session_summarize"));
    }

    #[test]
    fn permission_levels_match_legacy() {
        let bridge = AgmemBridge;
        for n in ["kv_get", "kv_list", "memory_tree", "knowledge_list", "knowledge_read", "flow_load", "clone_evaluate"] {
            assert!(matches!(bridge.permission_level(n), PermissionLevel::None), "{n}");
        }
        for n in ["knowledge_lint", "knowledge_index", "knowledge_extract"] {
            assert!(matches!(bridge.permission_level(n), PermissionLevel::ReadOnly), "{n}");
        }
        for n in [
            "kv_set", "knowledge_add", "knowledge_update", "knowledge_remove",
            "knowledge_import", "knowledge_heal", "flow_create", "flow_update",
        ] {
            assert!(matches!(bridge.permission_level(n), PermissionLevel::Write), "{n}");
        }
    }

    #[test]
    fn payload_null_owner_user_round_trips() {
        // 上游 None 路径：owner/user 必须显式 null，缺席键才会被 CLI
        // 当人面默认（default/local）——串了就是跨身份读库。
        let ctx = bare_ctx();
        let p = build_payload("kv_get", &serde_json::json!({"key": "k"}), &ctx);
        assert!(p["_ctx"]["owner_id"].is_null());
        assert!(p["_ctx"]["user_id"].is_null());
        assert!(p["_ctx"].get("agent_id").is_none());

        let mut ctx = bare_ctx();
        ctx.caller_agent_id = Some("mo");
        ctx.owner_id = Some("o1");
        ctx.sender_id = Some("u@im");
        ctx.home_dir = Some(std::path::Path::new("/home"));
        ctx.workspace_root = Some(std::path::Path::new("/var/lib/ws/mo"));
        let p = build_payload("kv_set", &serde_json::json!({"key": "k"}), &ctx);
        assert_eq!(p["_ctx"]["agent_id"], "mo");
        assert_eq!(p["_ctx"]["owner_id"], "o1");
        assert_eq!(p["_ctx"]["user_id"], "u@im");
        assert_eq!(p["_ctx"]["home_dir"], "/home");
        assert_eq!(p["_ctx"]["workspace_root"], "/var/lib/ws/mo");
        // 原入参不动
        assert_eq!(p["key"], "k");
    }

    #[tokio::test]
    async fn unknown_name_is_none() {
        let bridge = AgmemBridge;
        let ctx = bare_ctx();
        assert!(bridge
            .execute("nope", &serde_json::json!({}), &ctx)
            .await
            .is_none());
        // 留守面不经桥
        assert!(bridge
            .execute("apply_patch", &serde_json::json!({}), &ctx)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn kv_without_agent_context_errors_before_spawn() {
        // 上游 kv.rs 的闸：无 caller_agent_id → "No agent context"。
        // 必须在 spawn 前拦（CLI 无法区分身份缺席与人面默认）。
        let bridge = AgmemBridge;
        let ctx = bare_ctx();
        let r = bridge
            .execute("kv_get", &serde_json::json!({"key": "k"}), &ctx)
            .await
            .unwrap();
        let msg = match r {
            Err(CarrierError::Internal(m)) => m,
            other => panic!("expected Internal error, got {other:?}"),
        };
        assert_eq!(msg, "No agent context");
    }

    #[tokio::test]
    async fn spawn_without_agmem_reports_clean_error() {
        // PATH 里没有 agmem（本测试进程）→ 干净的 Internal 报错带安装
        // 提示，不是 panic。
        let bridge = AgmemBridge;
        let mut ctx = bare_ctx();
        ctx.caller_agent_id = Some("mo");
        let r = bridge
            .execute("kv_list", &serde_json::json!({}), &ctx)
            .await
            .unwrap();
        let msg = match r {
            Err(CarrierError::Internal(m)) => m,
            other => panic!("expected Internal error, got {other:?}"),
        };
        assert!(msg.contains("agmem CLI not available"), "{msg}");
        assert!(msg.contains("ag pkg install agmem"), "{msg}");
    }
}
