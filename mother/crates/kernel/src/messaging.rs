//! Agent message dispatch and execution — send, stream, WASM, Python, LLM.
//!
//! Handles the core agent communication paths: plain send, streaming send,
//! and module-type dispatch (WASM sandbox, Python subprocess, LLM agent loop).

use carrier_runtime::agent_loop::{run_agent_loop, run_agent_loop_streaming, AgentLoopResult};
use carrier_runtime::kernel_handle::KernelHandle;
use carrier_runtime::llm_driver::LlmDriver;
use carrier_runtime::llm_driver::StreamEvent;
use carrier_runtime::python_runtime::{self, PythonConfig};
use carrier_runtime::sandbox::SandboxConfig;
use carrier_types::agent::*;
use carrier_types::error::CarrierError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

use crate::capabilities::manifest_to_capabilities;
use crate::error::{KernelError, KernelResult};
use crate::kernel::CarrierKernel;
use crate::prompt_sources::touch_user_profile;
use crate::workspace::append_daily_memory_log;

/// A session whose last activity is older than this is treated as stale: the
/// next inbound message starts a fresh session instead of appending to the old
/// one. Stops a single sender's unrelated tasks from piling up across days
/// into one bloated, cross-contaminated session (the o9cq80yV failure mode:
/// one openid, two months, a dozen unrelated tasks in one 45-message session).
const SESSION_STALE_SECS: i64 = 12 * 60 * 60; // 12 hours

/// Rollover threshold for chained-pipeline sessions (explicit `session_label`).
/// Above this the session is rolled to a fresh suffixed one: observed at
/// 300K+ chars the LLM degrades (empty / text-only responses) AND the
/// summarizer itself fails, so compaction cannot rescue it — the longer it
/// gets the dumber it gets, a self-reinforcing loop (jiakao-20260815). User
/// chat sessions are NOT rolled (history matters there; compaction + the
/// staleness window govern them). Chain steps are safe to roll: pipeline
/// state lives on disk (`output/<pipeline_id>/`) and each step's cron message
/// is self-contained.
const SESSION_ROLLOVER_CHARS: usize = 150_000;

/// Approximate serialized size of a session's messages (chars). Cheap upper
/// bound via serde; the cost is negligible next to the LLM call it protects.
fn session_chars(session: &carrier_memory::session::Session) -> usize {
    session
        .messages
        .iter()
        .map(|m| serde_json::to_string(m).map(|s| s.len()).unwrap_or(0))
        .sum()
}

/// List files under `output_dir` as relative paths (`foo.md`, `subdir/a.png`).
fn list_output_rel_paths(output_dir: &Path) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    if !output_dir.is_dir() {
        return out;
    }
    fn walk(dir: &Path, prefix: &str, out: &mut std::collections::HashSet<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            let path = entry.path();
            if path.is_dir() {
                walk(&path, &rel, out);
            } else if path.is_file() {
                out.insert(rel);
            }
        }
    }
    walk(output_dir, "", &mut out);
    out
}

/// Shared preparation context for LLM agent execution.
///
/// Both `send_message_streaming` and `execute_llm_agent` perform the same
/// session loading, compaction check, tool assembly, flow/subagent matching,
/// and manifest mutation steps before diverging at the actual LLM call.
/// This struct holds the results of that shared preparation.
struct PreparedContext {
    session: carrier_memory::session::Session,
    needs_compact: bool,
    tools: Vec<carrier_types::tool::ToolDefinition>,
    manifest: AgentManifest,
    driver: Arc<dyn LlmDriver>,
    ctx_window: Option<usize>,
    /// The auto-matched flow (if any), carrying the full parsed `FlowDef`.
    /// `flow_def.steps` non-empty => multi-step flow for `run_flow`.
    flow: Option<crate::prompt_sources::FlowMatch>,
}

/// 借用轮结果 — 响应文本 + 更新后的会话票据（交还用户侧持久化）。
///
/// 主人服务器对借用者无状态：会话的唯一真源是这张票据，由用户侧保管、下一轮
/// 再提交。`ticket` 已含本轮 user/assistant 消息与最新 turn_summaries。
#[derive(Debug)]
pub struct BorrowedTurnOutcome {
    /// 本轮最终文本响应。
    pub response: String,
    /// 是否为有意不回复的静默轮。
    pub silent: bool,
    /// 更新后的会话票据（用户侧持久化）。
    pub ticket: carrier_memory::session::SessionTicket,
    /// 本轮全部 LLM 调用的累计 token 用量。
    pub total_usage: carrier_types::message::TokenUsage,
    /// agent loop 实际迭代轮数。
    pub iterations: u32,
    /// 本轮产物文件（3.3 回传）：agent 写进 borrow/<uuid>/output/ 的内容，
    /// 轮末随响应回流用户侧后，主机上即销毁。
    pub files: Vec<BorrowedOutputFile>,
}

/// 单个回传产物文件。
#[derive(Debug, Clone, serde::Serialize)]
pub struct BorrowedOutputFile {
    /// workspace 相对路径（borrow/<uuid>/output/ 之后的部分）。
    pub name: String,
    #[serde(rename = "contentBase64")]
    pub content_base64: String,
}

/// 借用轮素材（3.2）——用户侧随 prompt 提交、仅本轮有效的文件。
///
/// 落在 `<workspace>/borrow/<uuid>/materials/<name>`，走 agent 自己的 file
/// 工具沙箱路径；轮末整目录销毁，开轮先清扫残留（借道轮不跨进程存活）。
#[derive(Debug, Clone)]
pub struct BorrowedMaterial {
    /// workspace 相对路径（拒绝 `..`/绝对路径/反斜杠）。
    pub name: String,
    /// 解码后的文件内容。
    pub content: Vec<u8>,
}

/// 单个借用素材的大小上限（decode 后）。
pub const BORROWED_MATERIAL_MAX_FILE: usize = 8 * 1024 * 1024;
/// 单轮借用素材总量上限（decode 后）。
pub const BORROWED_MATERIAL_MAX_TOTAL: usize = 32 * 1024 * 1024;
/// 单个回传产物上限（3.3）。
pub const BORROWED_OUTPUT_MAX_FILE: usize = 16 * 1024 * 1024;
/// 单轮回传产物总量预算——超预算的文件跳过并计数。
pub const BORROWED_OUTPUT_MAX_TOTAL: usize = 64 * 1024 * 1024;

impl CarrierKernel {
    /// Inject flow-declared tools into the turn's tool list.
    ///
    /// When `elevate` is true (shared system flow with `privilege: system`), tools
    /// are resolved by **exact catalog lookup** including Dangerous tools like
    /// `shell_exec` (bypassing `search_tools`' level filter). Otherwise tools are
    /// resolved via `search_tools` under the agent's `max_tool_level`.
    fn inject_flow_tools(
        &self,
        tools: &mut Vec<carrier_types::tool::ToolDefinition>,
        flow: &crate::prompt_sources::FlowMatch,
        max_tool_level: carrier_types::tool::PermissionLevel,
        elevate: bool,
        cli_exec: carrier_types::config::CliExecConfig,
    ) -> Vec<String> {
        let mut warnings: Vec<String> = Vec::new();
        if flow.tools.is_empty() {
            warnings.push(format!(
                "Flow '{}' has no declared tools in its frontmatter. \
                 If this flow requires tools, use flow_update to add a tools: [\"tool1\", \"tool2\"] field.",
                flow.name
            ));
            return warnings;
        }

        let lookup_level = if elevate {
            carrier_types::tool::PermissionLevel::Dangerous
        } else {
            max_tool_level
        };

        for t in &flow.tools {
            if tools.iter().any(|d| d.name == *t) {
                continue;
            }

            // Exact builtin / plugin match first (honors Dangerous when elevating).
            if let Some(def) = self.lookup_tool_definition_exact(t, cli_exec.clone(), elevate) {
                let level = carrier_types::tool::PermissionLevel::for_tool(t);
                if elevate || level <= max_tool_level {
                    tools.push(def);
                    continue;
                }
            }

            // Fallback: scored search (MCP tools, fuzzy names) under lookup_level.
            if let Some((_, def)) = self.search_tools(t, 1, lookup_level).into_iter().next() {
                tools.push(def);
            } else {
                warnings.push(format!(
                    "Flow '{}' declared tool '{}' but it was not found in the tool catalog. \
                     Use flow_update to remove or correct this tool declaration.",
                    flow.name, t
                ));
            }
        }
        warnings
    }

    /// Exact-name tool definition from builtin modules or plugin dispatcher.
    /// When `allow_dangerous` is false, Dangerous tools are skipped.
    fn lookup_tool_definition_exact(
        &self,
        name: &str,
        cli_exec: carrier_types::config::CliExecConfig,
        allow_dangerous: bool,
    ) -> Option<carrier_types::tool::ToolDefinition> {
        let level = carrier_types::tool::PermissionLevel::for_tool(name);
        if !allow_dangerous && level == carrier_types::tool::PermissionLevel::Dangerous {
            return None;
        }
        if let Some(def) = carrier_runtime::tool_runner::builtin_tool_definitions(cli_exec)
            .into_iter()
            .find(|d| d.name == name)
        {
            return Some(def);
        }
        // Plugin tool dispatcher (channel tools registered as ToolProvider).
        if let Some(dispatcher) = self
            .plugins
            .plugin_tool_dispatcher
            .lock()
            .ok()
            .and_then(|g| g.clone())
        {
            if let Some(def) = dispatcher
                .definitions()
                .into_iter()
                .find(|d| d.name == name)
            {
                return Some(def);
            }
        }
        None
    }

    /// Shared preparation for LLM agent execution: session loading, compaction
    /// check, core tool set assembly, flow/subagent classification, and manifest
    /// mutation. Returns a `PreparedContext` that both streaming and non-streaming
    /// paths consume before diverging at the actual LLM invocation.
    /// Load a named flow and prepare its turn injection: inject the flow's
    /// tools into `tools`, build the flow prompt string, and read its
    /// `max_iterations`. Shared by the resume path (user replying to a
    /// suspended flow) and the explicit `active_flow` path — both bypass the
    /// non-deterministic LLM classifier and load by name. Returns `None` when
    /// no flow definition matches `flow_name`.
    /// Load (and parse) a named flow from disk — the cacheable half of flow
    /// loading. This is the expensive part: `load_flow_by_name` does a
    /// `read_dir` + parse of every flow file in the workspace. Callers that
    /// load the same flow repeatedly (e.g. `execute_plan`, one flow per step)
    /// should memoize this so each file is parsed at most once per plan.
    fn load_flow_match(
        &self,
        entry: &AgentEntry,
        flow_name: &str,
    ) -> Option<crate::prompt_sources::FlowMatch> {
        let ws = entry.manifest.workspace.as_ref()?;
        crate::prompt_sources::load_flow_by_name(ws, flow_name)
    }

    /// Apply a parsed flow to a turn: scope `tools` to the flow's declared set
    /// and build the flow prompt. Cheap (no disk I/O) — safe to call once per
    /// turn even when the flow was loaded from a cache.
    fn apply_flow_to_turn(
        &self,
        flow: &crate::prompt_sources::FlowMatch,
        tools: &mut Vec<carrier_types::tool::ToolDefinition>,
        entry: &AgentEntry,
    ) -> (String, Option<u32>) {
        let flow_name_owned = flow.name.clone();
        let flow_max_iter = flow.max_iterations;
        let elevate = flow.elevates();
        let flow_warnings = self.inject_flow_tools(
            tools,
            flow,
            entry.manifest.max_tool_level,
            elevate,
            entry.manifest.cli_exec.clone().unwrap_or_default(),
        );
        let mut flow_prompt = format!("**{}**\n{}", flow_name_owned, flow.body.clone());
        if !flow_warnings.is_empty() {
            flow_prompt.push_str(&format!(
                "\n\n⚠️ **Flow Tool Warnings:**\n{}",
                flow_warnings
                    .iter()
                    .map(|w| format!("- {}", w))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        (flow_prompt, flow_max_iter)
    }

    fn load_named_flow_for_turn(
        &self,
        entry: &AgentEntry,
        tools: &mut Vec<carrier_types::tool::ToolDefinition>,
        flow_name: &str,
    ) -> Option<(String, Option<u32>, crate::prompt_sources::FlowMatch)> {
        let flow = self.load_flow_match(entry, flow_name)?;
        let (flow_prompt, flow_max_iter) = self.apply_flow_to_turn(&flow, tools, entry);
        Some((flow_prompt, flow_max_iter, flow))
    }

    /// Read `default_flow` from the clone's definition-layer `template.json`
    /// (single field via serde_json::Value — a full-struct parse fails on
    /// drifted template shapes like mcp_servers object-arrays). Used as the
    /// classifier-miss fallback source when the agent.toml override is unset.
    fn read_template_default_flow(entry: &AgentEntry) -> Option<String> {
        let ws = entry.manifest.workspace.as_ref()?;
        let text = std::fs::read_to_string(ws.join("template.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&text).ok()?;
        v.get("default_flow")
            .and_then(|d| d.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_agent_context(
        &self,
        agent_id: AgentId,
        message: &str,
        entry: &AgentEntry,
        sender_id: &Option<String>,
        sender_name: Option<String>,
        owner_id: &Option<String>,
        channel_type: &Option<String>,
        task_id: Option<&str>,
        resume_flow: Option<&carrier_memory::FlowRunRow>,
        active_flow: Option<&str>,
        session_label: Option<&str>,
        chain_id: Option<String>,
    ) -> KernelResult<PreparedContext> {
        let agent_name = self
            .registry
            .get(agent_id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| agent_id.to_string());
        // Session isolation: every session MUST carry a traceable label.
        // Priority: sender_id (user:<openid>) > task_id > owner_id > channel_type.
        // There is NO silent fallback to an unlabeled "default" session — that
        // fallback was the source of orphan sessions: calls with no sender (cron
        // jobs without sender_id, background ticks, webhooks) all landed on the
        // agent's label=None default session, piling up invisible, untraceable
        // rows (wechat-writer had 48 of them). If none of the identifiers is
        // present, hard-error so the call site is forced to pass an explicit
        // label rather than silently corrupting session isolation.
        let session = {
            let label = Self::resolve_session_label(
                agent_id,
                session_label,
                sender_id,
                task_id,
                owner_id,
                channel_type,
            )?;
            // Windowed lookup: only resume a session updated within the
            // staleness window. A sender returning after the window starts a
            // fresh session (the old one stays archived-in-place, out of the
            // active window) — see SESSION_STALE_SECS.
            let mut session = match self
                .memory
                .find_active_session_by_label_async(&agent_name, &label, SESSION_STALE_SECS)
                .await
                .map_err(KernelError::Carrier)?
            {
                Some(s) => s,
                None => self
                    .memory
                    .create_session_with_label(agent_name.clone(), Some(&label))
                    .map_err(KernelError::Carrier)?,
            };
            // P1-C authority flip (config `session_event_source`): session
            // history loads from the append-only event log; the sessions DB
            // row stays as cache/identity. A fold-vs-cache length mismatch is
            // warn-logged — the runtime form of the "model-visible ⟺
            // logged" assertion (every mismatch is a divergence to inspect,
            // never silently ignored). Missing log for an old session falls
            // back to the DB row (pre-event-log sessions).
            if self.config.session_event_source {
                match self
                    .memory
                    .session_events_fold(&agent_name, &session.id.0.to_string())
                {
                    Ok(folded) if !folded.is_empty() => {
                        if folded.len() != session.messages.len() {
                            warn!(
                                agent = %agent_name,
                                session = %session.id.0,
                                db_msgs = session.messages.len(),
                                log_msgs = folded.len(),
                                "session event log vs DB cache length mismatch (event log wins; inspect divergence)"
                            );
                        }
                        session.messages = folded;
                    }
                    Ok(_) => {} // empty log: pre-event-log session, DB cache stands
                    Err(e) => {
                        warn!(
                            agent = %agent_name,
                            error = %e,
                            "session event fold failed — falling back to DB cache"
                        );
                    }
                }
            }
            // Session-too-long rollover (degeneration guard): a chained
            // pipeline session (explicit `session_label`) that has ballooned is
            // a model-degeneration trigger — the summarizer itself fails
            // ("summarization unavailable") and the LLM degrades to empty /
            // text-only responses. 300K-char sessions were observed doing this
            // (jiakao-20260815). Roll THIS turn to a fresh suffixed session:
            // pipeline state lives on disk (`output/<pipeline_id>/`) and each
            // chain step's cron message is self-contained, so no context is
            // lost. Only explicit-label (chained) sessions roll — user chat
            // sessions keep their history (compaction + staleness window handle
            // those).
            if session_label.is_some_and(|l| !l.trim().is_empty()) {
                let total_chars = session_chars(&session);
                if total_chars > SESSION_ROLLOVER_CHARS {
                    let mut suffix = 2u32;
                    let new_label = loop {
                        let cand = format!("{label}-r{suffix}");
                        match self
                            .memory
                            .find_active_session_by_label_async(
                                &agent_name,
                                &cand,
                                SESSION_STALE_SECS,
                            )
                            .await
                            .map_err(KernelError::Carrier)?
                        {
                            Some(_) => suffix += 1,
                            None => break cand,
                        }
                    };
                    tracing::warn!(
                        agent = %agent_name,
                        old_label = %label,
                        new_label = %new_label,
                        session_chars = total_chars,
                        threshold = SESSION_ROLLOVER_CHARS,
                        "Chained-pipeline session too large — rolling to a fresh session to avoid model degeneration"
                    );
                    session = self
                        .memory
                        .create_session_with_label(agent_name.clone(), Some(&new_label))
                        .map_err(KernelError::Carrier)?;
                }
            }
            session
        };

        // Check if auto-compaction is needed
        let needs_compact =
            self.check_compaction_needed(&session, &entry.manifest.model.system_prompt, agent_id);

        // Build agent's core tool set (bootstrap tools + delegate tools)
        let mut tools = self.resolve_tools(entry);

        // Auto-match flow for prompt injection
        let brain_ref: Option<Arc<dyn carrier_runtime::llm_driver::Brain>> =
            Some(Arc::clone(&*self.brain.brain.read().unwrap_or_else(|e| {
                warn!("Brain RwLock poisoned, recovering");
                e.into_inner()
            }))
                as Arc<dyn carrier_runtime::llm_driver::Brain>);

        // Flow resolution priority: resume > explicit active_flow > LLM classify.
        // resume and active_flow both load a named flow directly (skipping the
        // non-deterministic classifier); classify is the fallback. A classify
        // miss runs a BARE turn — no default_flow fallback (operator ruling
        // 2026-08-18: silent fallbacks hide configuration problems; an
        // explicit active_flow is the sanctioned way to pin a flow).
        let (auto_matched_flow, flow_max_iterations, matched_flow) = self
            .resolve_matched_flow(
                entry,
                message,
                &mut tools,
                &brain_ref,
                resume_flow,
                active_flow,
                &session,
            )
            .await;

        // Auto-match subagent trigger (only when no flow matched) + subagent
        // delegation from channel_type.
        let (auto_matched_subagent, subagent_config) = Self::resolve_subagent(
            message,
            &entry.manifest.subagents,
            channel_type,
            &auto_matched_flow,
            &entry.name,
        );

        let driver = self.resolve_driver(&entry.manifest)?;
        let ctx_window: Option<usize> = None;

        let mut manifest = entry.manifest.clone();

        // Flow turn elevation (shared system OR private skill with shell_allow):
        // raise max_tool_level and stamp elevated tool names + shell_allow for tool_runner.
        // Also enforce deny_tools: strip from LLM tool list for this turn.
        // Every flow reaching here carries intent (resume/active_flow/classify
        // hit) — full authority, no cage mode since the fallback removal.
        if let Some(ref flow) = matched_flow {
            Self::apply_flow_elevation(&mut tools, &mut manifest, flow, &entry.name);
        }

        // Apply flow's then subagent's max_iterations override (subagent wins).
        Self::apply_manifest_overrides(
            &mut manifest,
            flow_max_iterations,
            subagent_config.as_ref(),
            &entry.name,
        );

        // Combine flow and subagent auto-match for prompt injection
        let prompt_auto_match = auto_matched_flow.or_else(|| {
            auto_matched_subagent.map(|name| format!("**Auto-delegation: {}**\nThe user message matches the '{}' subagent. Call delegate_{} to handle this task.", name, name, name))
        });

        // L0 turn summaries from session
        let turn_summaries = session.turn_summaries.clone();

        // Drawer + compaction-recall from kv memory — canonical partition
        // (agent_name, owner or "", sender or ""), same as every writer.
        let (drawer_entries, recalled_memories) = self.prefetch_kv_memories(
            &manifest.name,
            owner_id.as_deref().unwrap_or(""),
            sender_id.as_deref().unwrap_or(""),
        );

        self.build_and_apply_prompt(
            &mut manifest,
            &tools,
            sender_id,
            sender_name,
            owner_id,
            prompt_auto_match.clone(),
            turn_summaries,
            drawer_entries,
            recalled_memories,
            task_id.map(|s| s.to_string()),
            chain_id,
        );

        Ok(PreparedContext {
            session,
            needs_compact,
            tools,
            manifest,
            driver,
            ctx_window,
            flow: matched_flow,
        })
    }

    /// Resolve the session label from the available identifiers.
    ///
    /// Priority: explicit override (chained pipelines) > sender_id
    /// (user:<openid>) > task_id > owner_id > channel_type.
    /// There is NO silent fallback to an unlabeled "default" session — that
    /// fallback was the source of orphan sessions: calls with no sender (cron
    /// jobs without sender_id, background ticks, webhooks) all landed on the
    /// agent's label=None default session, piling up invisible, untraceable
    /// rows (wechat-writer had 48 of them). If none of the identifiers is
    /// present, hard-error so the call site is forced to pass an explicit
    /// label rather than silently corrupting session isolation.
    fn resolve_session_label(
        agent_id: AgentId,
        session_label: Option<&str>,
        sender_id: &Option<String>,
        task_id: Option<&str>,
        owner_id: &Option<String>,
        channel_type: &Option<String>,
    ) -> KernelResult<String> {
        let label = if let Some(explicit) = session_label.filter(|l| !l.trim().is_empty()) {
            // Chained-pipeline isolation: cron AgentTurn turns with an
            // explicit `session_label` run in their OWN session, so user
            // chat interleaving mid-chain cannot pollute pipeline steps
            // (and vice versa). The sender_id still routes file paths and
            // delivery — only the session identity is overridden.
            explicit.trim().to_string()
        } else if let Some(ref sid) = sender_id {
            format!("user:{}", sid)
        } else if let Some(t) = task_id {
            format!("task:{}", t)
        } else if let Some(ref o) = owner_id {
            format!("owner:{}", o)
        } else if let Some(ref c) = channel_type {
            format!("channel:{}", c)
        } else {
            warn!(
                agent_id = %agent_id,
                "Session isolation: sender_id/task_id/owner_id/channel_type all None — refusing to create unlabeled (orphan) session"
            );
            return Err(KernelError::Carrier(CarrierError::InvalidInput(
                "cannot determine session label: sender_id/task_id/owner_id/channel_type all missing — pass an explicit label at the call site".into(),
            )));
        };
        Ok(label)
    }

    /// Check whether the session needs auto-compaction, on three criteria:
    /// message-count threshold, estimated-token threshold, and quota headroom.
    fn check_compaction_needed(
        &self,
        session: &carrier_memory::session::Session,
        system_prompt: &str,
        agent_id: AgentId,
    ) -> bool {
        use carrier_runtime::compactor::{
            estimate_token_count, needs_compaction as check_compact, needs_compaction_by_tokens,
            CompactionConfig,
        };
        let config = CompactionConfig::default();
        let by_messages = check_compact(session, &config);
        let estimated = estimate_token_count(&session.messages, Some(system_prompt), None);
        let by_tokens = needs_compaction_by_tokens(estimated, &config);
        if by_tokens && !by_messages {
            info!(
                agent_id = %agent_id,
                estimated_tokens = estimated,
                messages = session.messages.len(),
                "Token-based compaction triggered (messages below threshold but tokens above)"
            );
        }
        let by_quota = if let Some(headroom) = self.runtime.scheduler.token_headroom(agent_id) {
            let threshold = (headroom as f64 * 0.8) as u64;
            if estimated as u64 > threshold && session.messages.len() > 4 {
                info!(
                    agent_id = %agent_id,
                    estimated_tokens = estimated,
                    quota_headroom = headroom,
                    "Quota-headroom compaction triggered (session would consume >80% of remaining quota)"
                );
                true
            } else {
                false
            }
        } else {
            false
        };
        by_messages || by_tokens || by_quota
    }

    /// Build the agent's core tool set for this turn: bootstrap CORE_TOOL_NAMES,
    /// declarative API tools, and subagent delegate tools.
    fn resolve_tools(&self, entry: &AgentEntry) -> Vec<carrier_types::tool::ToolDefinition> {
        let mut tools: Vec<carrier_types::tool::ToolDefinition> =
            carrier_runtime::tool_runner::builtin_tool_definitions(self.config.cli_exec.clone())
                .into_iter()
                .filter(|t| carrier_types::tool::CORE_TOOL_NAMES.contains(&t.name.as_str()))
                .collect();

        // Also include declarative API tools — they are always available to all
        // agents (registered via builtin_modules), but not in CORE_TOOL_NAMES.
        // Capabilities.tools filtering still applies downstream.
        let home_dir = carrier_types::config::home_dir();
        let api_tool_names: std::collections::HashSet<String> = {
            let mut names: std::collections::HashSet<String> =
                carrier_runtime::api_tools::loader::load_all_api_tools(
                    &home_dir,
                    entry.manifest.workspace.as_deref(),
                )
                .into_iter()
                .map(|t| t.name)
                .collect();
            // Include dynamically registered tools
            for dt in carrier_runtime::api_tools::register::dynamic_tools() {
                names.insert(dt.name);
            }
            names
        };
        if !api_tool_names.is_empty() {
            let all_builtins = carrier_runtime::tool_runner::builtin_tool_definitions(
                self.config.cli_exec.clone(),
            );
            for t in &all_builtins {
                if api_tool_names.contains(&t.name) {
                    tools.push(t.clone());
                }
            }
        }

        if !entry.manifest.subagents.is_empty() {
            tools.extend(carrier_types::agent::build_subagent_tool_definitions(
                &entry.manifest.subagents,
            ));
        }

        info!(
            agent = %entry.name,
            tool_count = tools.len(),
            "Agent core tool set assembled"
        );

        tools
    }

    /// Resolve the flow to inject this turn. Priority: resume > explicit
    /// active_flow > LLM classify. resume and active_flow both load a named flow
    /// directly (skipping the non-deterministic classifier); classify is the
    /// fallback. A silent classify-None previously left the agent with no flow
    /// prompt AND no max_iterations override (ad-occ3 root cause) — now logged.
    /// Mutates `tools` to inject the matched flow's declared tools.
    /// Returns (flow_prompt, max_iterations, matched_flow). A classifier miss
    /// returns NO flow — there is no default_flow fallback (operator ruling
    /// 2026-08-18: a silent guessed flow hides configuration problems behind
    /// a caged, half-empowered turn; run bare and let the gap be visible).
    #[allow(clippy::too_many_arguments)]
    async fn resolve_matched_flow(
        &self,
        entry: &AgentEntry,
        message: &str,
        tools: &mut Vec<carrier_types::tool::ToolDefinition>,
        brain_ref: &Option<Arc<dyn carrier_runtime::llm_driver::Brain>>,
        resume_flow: Option<&carrier_memory::FlowRunRow>,
        active_flow: Option<&str>,
        session: &carrier_memory::session::Session,
    ) -> (
        Option<String>,
        Option<u32>,
        Option<crate::prompt_sources::FlowMatch>,
    ) {
        let from_resume = if let Some(rf) = resume_flow {
            // Resume: load by name WITHOUT an LLM classify call -- the user's
            // reply continues an already-matched flow, so re-classifying would
            // be wrong (and might match a different flow).
            match self.load_named_flow_for_turn(entry, tools, &rf.flow_name) {
                Some(loaded) => {
                    info!(
                        agent = %entry.name,
                        flow = %rf.flow_name,
                        "Flow loaded for resume"
                    );
                    Some(loaded)
                }
                None => {
                    warn!(agent = %entry.name, flow = %rf.flow_name, "resume: flow def not found, falling back to normal handling");
                    None
                }
            }
        } else {
            None
        };

        // Explicit active_flow (HTTP/cron caller): also bypasses the
        // classifier. If the named flow is missing, fall through to classify
        // rather than giving up silently. All flows are workspace flows now
        // ("全进分身"), so always allowed (no system-flow allowlist gate).
        let from_active = if from_resume.is_none() {
            if let Some(name) = active_flow {
                match self.load_named_flow_for_turn(entry, tools, name) {
                    Some((prompt, max_iter, flow)) => {
                        info!(agent = %entry.name, flow = %name, "Flow loaded by active_flow (explicit)");
                        Some((prompt, max_iter, flow))
                    }
                    None => {
                        warn!(agent = %entry.name, flow = %name, "active_flow not found — falling back to classifier");
                        None
                    }
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some((prompt, max_iter, flow)) = from_resume.or(from_active) {
            (Some(prompt), max_iter, Some(flow))
        } else if let (Some(ws), Some(brain)) =
            (entry.manifest.workspace.as_ref(), brain_ref.as_ref())
        {
            // Give the classifier recent conversation context so it can
            // match follow-up messages in multi-turn workflows (e.g.
            // charter-quoter after the user sends their phone in turn 2).
            let recent_turns: Vec<(String, String)> = session
                .turn_summaries
                .iter()
                .rev()
                .take(2)
                .rev()
                .map(|t| {
                    let intent = if t.user_intent.is_empty() {
                        "(no intent)".to_string()
                    } else {
                        t.user_intent.clone()
                    };
                    let outcome = if t.assistant_outcome.is_empty() {
                        "(no outcome)".to_string()
                    } else {
                        t.assistant_outcome.clone()
                    };
                    (intent, outcome)
                })
                .collect();
            match crate::prompt_sources::classify_flow_with_llm(
                message,
                ws,
                brain,
                &entry.manifest.flows,
                &recent_turns,
                entry.manifest.clone_source.is_some(),
            )
            .await
            {
                Some(flow) => {
                    let flow_name = flow.name.clone();
                    let flow_body = flow.body.clone();
                    let flow_max_iter = flow.max_iterations;
                    let elevate = flow.elevates();
                    let flow_warnings = self.inject_flow_tools(
                        tools,
                        &flow,
                        entry.manifest.max_tool_level,
                        elevate,
                        entry.manifest.cli_exec.clone().unwrap_or_default(),
                    );

                    info!(
                        agent = %entry.name,
                        flow = %flow_name,
                        elevate,
                        "Flow classified by LLM"
                    );

                    let mut flow_prompt = format!("**{}**\n{}", flow_name, flow_body);
                    if !flow_warnings.is_empty() {
                        flow_prompt.push_str(&format!(
                            "\n\n⚠️ **Flow Tool Warnings:**\n{}",
                            flow_warnings
                                .iter()
                                .map(|w| format!("- {}", w))
                                .collect::<Vec<_>>()
                                .join("\n")
                        ));
                    }

                    // The flow body is injected into the base system prompt for
                    // BOTH single- and multi-step flows. Multi-step execution
                    // (run_flow) receives this base prompt and adds per-step
                    // directives on top; the streaming path falls back to
                    // guided single-step execution if run_flow isn't wired there.
                    (Some(flow_prompt), flow_max_iter, Some(flow))
                }
                None => {
                    // Classifier miss — try a SAFE default_flow fallback. The
                    // 08-16 cage (load a guessed flow but withhold authority)
                    // silently degraded turns; the 08-18 revert ran fully bare.
                    // Here we load the clone's declared default_flow ONLY when
                    // it is a pure consultation flow that cannot elevate (no
                    // shell_exec/shell_allow) — so a casual message can never
                    // be lifted to privileged shell access (the
                    // article-formatter regression). An elevating fallback is
                    // skipped and the turn stays bare, visibly.
                    let fallback_name = entry
                        .manifest
                        .default_flow
                        .clone()
                        .or_else(|| Self::read_template_default_flow(entry));
                    let fallback_flow = fallback_name
                        .as_deref()
                        .and_then(|n| self.load_flow_match(entry, n));
                    match fallback_flow {
                        Some(flow) if !flow.elevates() => {
                            let (flow_prompt, flow_max_iter) =
                                self.apply_flow_to_turn(&flow, tools, entry);
                            info!(
                                agent = %entry.name,
                                flow = %flow.name,
                                "Classifier miss — loaded safe default_flow fallback"
                            );
                            (Some(flow_prompt), flow_max_iter, Some(flow))
                        }
                        Some(flow) => {
                            warn!(
                                agent = %entry.name,
                                flow = %flow.name,
                                "default_flow fallback skipped — flow elevates (would grant shell access to a casual turn); running bare"
                            );
                            (None, None, None)
                        }
                        None => {
                            warn!(
                                agent = %entry.name,
                                "Flow classifier returned no match — bare turn (no safe default_flow fallback; pass active_flow to pin a flow)"
                            );
                            (None, None, None)
                        }
                    }
                }
            }
        } else {
            // Nothing happens invisibly: record WHY the classifier could not
            // even run (silent (None, None, None) here leaves the turn with
            // no flow prompt and no explanation in the logs).
            match (entry.manifest.workspace.as_ref(), brain_ref.as_ref()) {
                (None, _) => {
                    warn!(agent = %entry.name, "Flow classification skipped — no workspace (no flows to match)")
                }
                (Some(_), None) => {
                    warn!(agent = %entry.name, "Flow classification skipped — no brain configured")
                }
                _ => {}
            }
            (None, None, None)
        }
    }

    /// Auto-match a subagent trigger (only when no flow matched) and resolve the
    /// subagent config from channel_type ("subagent:<name>"). Returns
    /// (auto_matched_subagent_name, subagent_config).
    fn resolve_subagent(
        message: &str,
        subagents: &[SubagentConfig],
        channel_type: &Option<String>,
        auto_matched_flow: &Option<String>,
        agent_name: &str,
    ) -> (Option<String>, Option<SubagentConfig>) {
        // Auto-match subagent trigger (only when no flow matched)
        let auto_matched_subagent = if auto_matched_flow.is_none() && !subagents.is_empty() {
            if let Some(sa_match) =
                crate::prompt_sources::match_subagent_for_message(message, subagents)
            {
                info!(
                    agent = %agent_name,
                    subagent = %sa_match.name,
                    "Subagent trigger matched"
                );
                Some(sa_match.name.clone())
            } else {
                None
            }
        } else {
            None
        };

        // Subagent delegation from channel_type
        let subagent_config = if let Some(ref ct) = channel_type {
            if let Some(sa_name) = ct.strip_prefix("subagent:") {
                subagents.iter().find(|s| s.name == sa_name).cloned()
            } else {
                None
            }
        } else {
            None
        };

        (auto_matched_subagent, subagent_config)
    }

    /// Apply flow turn elevation: enforce deny_tools (strip from the LLM tool
    /// list for this turn), and for elevating flows raise max_tool_level and
    /// stamp elevated tool names + shell_allow into manifest metadata.
    /// Full authority: every flow reaching here was selected with intent
    /// (resume / explicit active_flow / classifier hit) — the caged
    /// default_flow fallback mode was removed 2026-08-18.
    fn apply_flow_elevation(
        tools: &mut Vec<carrier_types::tool::ToolDefinition>,
        manifest: &mut AgentManifest,
        flow: &crate::prompt_sources::FlowMatch,
        agent_name: &str,
    ) {
        if !flow.flow_def.deny_tools.is_empty() {
            let before = tools.len();
            tools.retain(|t| {
                !flow
                    .flow_def
                    .deny_tools
                    .iter()
                    .any(|d| d == &t.name || t.name.ends_with(&format!("__{d}")))
            });
            manifest.metadata.insert(
                carrier_types::flow::META_FLOW_DENY_TOOLS.to_string(),
                serde_json::json!(flow.flow_def.deny_tools),
            );
            info!(
                agent = %agent_name,
                flow = %flow.name,
                denied = ?flow.flow_def.deny_tools,
                removed = before.saturating_sub(tools.len()),
                "Flow deny_tools applied for this turn"
            );
        }
        // Flow `tools:` hard sandbox: when the matched flow declares a non-empty
        // tool set, freeze the assembled toolset (base + flow tools, post-deny)
        // as the turn's allow-list. Flow-injected tools are filtered to this set and
        // tool_runner denies calls outside it — so the agent can't wander to
        // out-of-flow catalog tools (e.g. clone-creator reaching train_write
        // instead of the flow's declared clone_install). Only stamped when the
        // flow declares tools; a flow with no declared tools imposes no sandbox.
        if !flow.tools.is_empty() {
            let allowed: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
            manifest.metadata.insert(
                carrier_types::flow::META_FLOW_ALLOWED_TOOLS.to_string(),
                serde_json::json!(allowed),
            );
            info!(
                agent = %agent_name,
                flow = %flow.name,
                allowed_count = allowed.len(),
                "Flow tools hard sandbox stamped for this turn"
            );
        }
        // Flow-level `output: report`: the turn's FINAL message must carry a
        // valid Ralph report (carrier_types::flow::validate_step_report). end_turn
        // enforces it as a hard gate — chained pipeline steps (writing chain)
        // can no longer end on free-form prose with no quality assertion.
        // Only intentionally-selected flows reach here (the classifier-miss
        // default_flow fallback that had to skip this gate is gone), so a
        // casual chat turn only sees the gate if the classifier matched a
        // report flow for it.
        if flow.flow_def.output.as_deref().is_some_and(|o| {
            carrier_types::flow::StepOutputMode::parse(o.trim())
                == carrier_types::flow::StepOutputMode::Report
        }) {
            manifest.metadata.insert(
                carrier_types::flow::META_OUTPUT_REPORT.to_string(),
                serde_json::json!(true),
            );
            info!(
                agent = %agent_name,
                flow = %flow.name,
                "Flow output:report gate stamped for this turn"
            );
        }
        if flow.elevates() {
            let required = flow.flow_def.required_max_tool_level();
            if required > manifest.max_tool_level {
                info!(
                    agent = %agent_name,
                    flow = %flow.name,
                    from = ?manifest.max_tool_level,
                    to = ?required,
                    "Flow elevates max_tool_level for this turn"
                );
                manifest.max_tool_level = required;
            }
            manifest.metadata.insert(
                carrier_types::flow::META_FLOW_ELEVATED_TOOLS.to_string(),
                serde_json::json!(flow.tools),
            );
            if !flow.flow_def.shell_allow.is_empty() {
                manifest.metadata.insert(
                    carrier_types::flow::META_FLOW_SHELL_ALLOW.to_string(),
                    serde_json::json!(flow.flow_def.shell_allow),
                );
            }
        }
    }

    /// Apply max_iterations overrides: flow first, then subagent (subagent wins).
    fn apply_manifest_overrides(
        manifest: &mut AgentManifest,
        flow_max_iterations: Option<u32>,
        subagent_config: Option<&SubagentConfig>,
        agent_name: &str,
    ) {
        // Apply flow's max_iterations override
        if let Some(max_iter) = flow_max_iterations {
            manifest
                .autonomous
                .get_or_insert_with(Default::default)
                .max_iterations = max_iter;
            manifest.metadata.insert(
                carrier_types::flow::META_MAX_ITERATIONS_DECLARED.to_string(),
                serde_json::json!(max_iter),
            );
            info!(
                agent = %agent_name,
                max_iterations = max_iter,
                "Flow overrides max_iterations"
            );
        }

        // Apply subagent's max_iterations override
        if let Some(sa) = subagent_config {
            manifest
                .autonomous
                .get_or_insert_with(Default::default)
                .max_iterations = sa.max_iterations;
            manifest
                .metadata
                .insert("is_subagent".to_string(), serde_json::json!(true));
            manifest.metadata.insert(
                carrier_types::flow::META_MAX_ITERATIONS_DECLARED.to_string(),
                serde_json::json!(sa.max_iterations),
            );
            info!(
                agent = %agent_name,
                subagent = %sa.name,
                max_iterations = sa.max_iterations,
                "Subagent overrides max_iterations"
            );
        }
    }

    /// Send a message to an agent and get a response.
    ///
    /// Automatically upgrades the kernel handle from `self_handle` so that
    /// agent turns triggered by cron, channels, events, or inter-agent calls
    /// have full access to kernel tools (cron_create, agent_send, etc.).
    pub async fn send_message(
        &self,
        agent_id: AgentId,
        message: &str,
    ) -> KernelResult<AgentLoopResult> {
        let handle: Option<Arc<dyn KernelHandle>> = self
            .coordination
            .self_handle
            .get()
            .and_then(|w| w.upgrade())
            .map(|arc| arc as Arc<dyn KernelHandle>);
        self.send_message_with_handle(
            agent_id, message, handle, None, None, None, None, None, None,
        )
        .await
    }

    /// Send a multimodal message (text + images) to an agent and get a response.
    ///
    /// Send a message with an optional kernel handle for inter-agent tools.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_with_handle(
        &self,
        agent_id: AgentId,
        message: &str,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<String>,
        sender_name: Option<String>,
        owner_id: Option<String>,
        channel_type: Option<String>,
        task_id: Option<String>,
        active_flow: Option<&str>,
    ) -> KernelResult<AgentLoopResult> {
        self.send_message_with_handle_and_blocks(
            agent_id,
            message,
            kernel_handle,
            None,
            sender_id,
            sender_name,
            owner_id,
            channel_type,
            task_id,
            active_flow,
            None,
            None,
        )
        .await
    }

    /// Send a message with optional content blocks and an optional kernel handle.
    ///
    /// When `content_blocks` is `Some`, the LLM agent loop receives structured
    /// multimodal content (text + images) instead of just a text string. This
    /// enables vision models to process images sent from channels like Telegram.
    ///
    /// Per-agent locking ensures that concurrent messages for the same agent
    /// are serialized (preventing session corruption), while messages for
    /// different agents run in parallel.
    /// Bound an agent-turn future with a wall-clock backstop. All trigger paths
    /// (HTTP /send, channel inbound, cron, inter-agent) funnel through
    /// `send_message_with_handle_and_blocks`, which wraps each executor branch
    /// in this so every path is bounded consistently. Cron may also wrap in its
    /// own per-job timeout (tighter wins).
    ///
    /// This is a daemon-hang BACKSTOP only - the turn itself is governed by
    /// progress/stuck detection, not a time budget. `secs == 0` disables the
    /// backstop entirely (run unbounded, rely solely on stuck detection + the
    /// per-LLM-call stall timeout).
    async fn bounded_turn<F>(fut: F, secs: u64, agent_id: &str) -> KernelResult<AgentLoopResult>
    where
        F: std::future::Future<Output = KernelResult<AgentLoopResult>>,
    {
        if secs == 0 {
            return fut.await;
        }
        match tokio::time::timeout(std::time::Duration::from_secs(secs), fut).await {
            Ok(r) => r,
            Err(_) => Err(KernelError::Carrier(CarrierError::Internal(format!(
                "agent {agent_id} turn exceeded {secs}s backstop"
            )))),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_with_handle_and_blocks(
        &self,
        agent_id: AgentId,
        message: &str,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        content_blocks: Option<Vec<carrier_types::message::ContentBlock>>,
        sender_id: Option<String>,
        sender_name: Option<String>,
        owner_id: Option<String>,
        channel_type: Option<String>,
        task_id: Option<String>,
        active_flow: Option<&str>,
        session_label: Option<&str>,
        chain_id: Option<&str>,
    ) -> KernelResult<AgentLoopResult> {
        // NOTE: The per-owner execution lock has been removed. Concurrent messages
        // for the same agent+owner now run in parallel (like nginx). Session
        // consistency is maintained by `save_session_append_async` which uses
        // per-session write locks and merge-writes.

        // LLM concurrency is now enforced per-call inside the agent loop
        // (call_with_retry), not at the agent-loop level. This means a stuck
        // agent only holds a semaphore slot for the duration of a single LLM
        // call (~180-300s), not the entire loop.

        // Enforce quota before running the agent loop
        self.runtime
            .scheduler
            .check_quota(agent_id)
            .map_err(KernelError::Carrier)?;

        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        // Dispatch based on module type, bounded by the configurable turn
        // timeout (all trigger paths funnel through here). Cron may also wrap
        // per-job (tighter wins).
        let turn_secs = self.config.agent_turn_timeout_secs;
        let agent_id_str = agent_id.to_string();
        let result = if entry.manifest.module.starts_with("wasm:") {
            Self::bounded_turn(
                self.execute_wasm_agent(&entry, message, kernel_handle),
                turn_secs,
                &agent_id_str,
            )
            .await
        } else if entry.manifest.module.starts_with("python:") {
            Self::bounded_turn(
                self.execute_python_agent(&entry, agent_id, message),
                turn_secs,
                &agent_id_str,
            )
            .await
        } else {
            // Resume detection: if this sender has a suspended (waiting) flow
            // run for this agent, the message is the `user_input` reply --
            // resume the flow instead of starting a new conversation.
            let resume_row: Option<carrier_memory::FlowRunRow> = sender_id
                .as_ref()
                .and_then(|sid| {
                    self.memory
                        .flow_runs()
                        .list_pending(sid, &agent_id.to_string())
                        .ok()
                        .and_then(|v| v.into_iter().next())
                })
                .filter(|r| {
                    r.expires_at
                        .as_deref()
                        .is_none_or(|exp| exp > chrono::Utc::now().to_rfc3339().as_str())
                });

            // Intent classifier: decide whether to continue the current session
            // or open a new one. Skips for empty sessions, when disabled, or
            // when resuming a suspended flow (the reply continues the flow's
            // session, so rotation would be wrong).
            if resume_row.is_none() && entry.manifest.intent_classifier_enabled.unwrap_or(true) {
                if let Err(e) = self
                    .maybe_rotate_session_by_intent(agent_id, &entry, message)
                    .await
                {
                    tracing::warn!(agent_id = %agent_id, error = %e, "Intent classifier failed; opening new session as fallback");
                    // Fallback: open new session on classifier error.
                    let agent_name = self
                        .registry
                        .get(agent_id)
                        .map(|e| e.name.clone())
                        .unwrap_or_else(|| agent_id.to_string());
                    if let Ok(new_session) = self.memory.create_session_async(agent_name).await {
                        if let Err(e) = self.registry.update_session_id(agent_id, new_session.id) {
                            tracing::warn!(agent_id = %agent_id, error = %e, "Failed to update session ID in registry");
                        }
                    }
                }
            }
            // Re-fetch entry — session_id may have changed
            let entry = self.registry.get(agent_id).ok_or_else(|| {
                KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
            })?;
            // Default: LLM agent loop (builtin:chat or any unrecognized module)
            //
            // Sender-gated path (dsh inbox+claim at turn granularity): a
            // channel sender's messages serialize per (agent, label) and
            // rapid-fire messages coalesce into ONE combined turn — the lock
            // holder claims the whole inbox. A caller whose message was
            // claimed by an earlier runner returns a synthetic empty/silent
            // result (the combined reply already went to the same recipient;
            // the bridge skips empty responses so nothing sends twice).
            // API turns without a sender bypass the gate entirely.
            if sender_id.is_some() {
                let label = Self::resolve_session_label(
                    agent_id,
                    session_label,
                    &sender_id,
                    task_id.as_deref(),
                    &owner_id,
                    &channel_type,
                )?;
                let gate_key = format!("{agent_id_str}:{label}");
                let turn_secs_g = turn_secs;
                match self
                    .sender_gate
                    .run::<_, _, KernelResult<AgentLoopResult>>(
                        &gate_key,
                        message.to_string(),
                        content_blocks.clone(),
                        |batch| async move {
                            let combined = batch.combined_message();
                            let blocks = if batch.blocks.is_empty() {
                                None
                            } else {
                                Some(batch.blocks)
                            };
                            if batch.len > 1 {
                                tracing::info!(
                                    agent = %agent_id_str,
                                    coalesced = batch.len,
                                    "Sender gate coalesced rapid-fire messages into one turn"
                                );
                            }
                            Self::bounded_turn(
                                self.execute_llm_agent(
                                    &entry,
                                    agent_id,
                                    &combined,
                                    kernel_handle,
                                    blocks,
                                    sender_id,
                                    sender_name,
                                    owner_id,
                                    channel_type.clone(),
                                    task_id,
                                    resume_row.as_ref(),
                                    active_flow,
                                    session_label,
                                    chain_id.map(|s| s.to_string()),
                                ),
                                turn_secs_g,
                                &agent_id_str,
                            )
                            .await
                        },
                    )
                    .await
                {
                    crate::sender_gate::GatedOutcome::Ran(result) => result,
                    crate::sender_gate::GatedOutcome::Merged => Ok(AgentLoopResult {
                        response: String::new(),
                        total_usage: Default::default(),
                        iterations: 0,
                        silent: true,
                        directives: Default::default(),
                        plan: None,
                    }),
                }
            } else {
                Self::bounded_turn(
                    self.execute_llm_agent(
                        &entry,
                        agent_id,
                        message,
                        kernel_handle,
                        content_blocks,
                        sender_id,
                        sender_name,
                        owner_id,
                        channel_type.clone(),
                        task_id,
                        resume_row.as_ref(),
                        active_flow,
                        session_label,
                        chain_id.map(|s| s.to_string()),
                    ),
                    turn_secs,
                    &agent_id_str,
                )
                .await
            }
        };

        match result {
            Ok(mut result) => {
                // [CLONE_INSTALL:<name>] markers: kernel-side clone install at
                // turn end (clone-creator stages files incrementally under its
                // own staging/; the marker hands the move to the kernel).
                // Channel-agnostic — every non-streaming turn lands here.
                if let Some(entry) = self.registry.get(agent_id) {
                    self.process_clone_install_markers(
                        &entry.name,
                        entry.manifest.workspace.as_deref(),
                        &mut result.response,
                    )
                    .await;
                }

                // Record token usage for quota tracking
                self.runtime
                    .scheduler
                    .record_usage(agent_id, &result.total_usage);

                // Update last active time
                if let Err(e) = self.registry.set_state(agent_id, AgentState::Running) {
                    tracing::warn!(agent_id = %agent_id, error = %e, "Failed to set agent state to Running");
                }

                // SECURITY: Record successful message in audit trail
                self.audit_log.record(
                    agent_id.to_string(),
                    carrier_runtime::audit::AuditAction::AgentMessage,
                    format!(
                        "tokens_in={}, tokens_out={}",
                        result.total_usage.input_tokens, result.total_usage.output_tokens
                    ),
                    "ok",
                );

                Ok(result)
            }
            Err(e) => {
                // SECURITY: Record failed message in audit trail
                self.audit_log.record(
                    agent_id.to_string(),
                    carrier_runtime::audit::AuditAction::AgentMessage,
                    "agent loop failed",
                    format!("error: {e}"),
                );

                // Record the failure in supervisor for health reporting
                self.runtime.supervisor.record_panic();
                warn!(agent_id = %agent_id, error = %e, "Agent loop failed — recorded in supervisor");
                Err(e)
            }
        }
    }

    /// Send a message to an agent with streaming responses.
    ///
    /// Returns a receiver for incremental `StreamEvent`s and a `JoinHandle`
    /// that resolves to the final `AgentLoopResult`. The caller reads stream
    /// events while the agent loop runs, then awaits the handle for final stats.
    ///
    /// WASM and Python agents don't support true streaming — they execute
    /// synchronously and emit a single `TextDelta` + `ContentComplete` pair.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_streaming(
        self: &Arc<Self>,
        agent_id: AgentId,
        message: &str,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<String>,
        sender_name: Option<String>,
        owner_id: Option<String>,
        channel_type: Option<String>,
        active_flow: Option<&str>,
    ) -> KernelResult<(
        tokio::sync::mpsc::Receiver<StreamEvent>,
        tokio::task::JoinHandle<KernelResult<AgentLoopResult>>,
    )> {
        // Enforce quota before spawning the streaming task
        self.runtime
            .scheduler
            .check_quota(agent_id)
            .map_err(KernelError::Carrier)?;

        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        let is_wasm = entry.manifest.module.starts_with("wasm:");
        let is_python = entry.manifest.module.starts_with("python:");

        // Non-LLM modules: execute non-streaming and emit results as stream events
        if is_wasm || is_python {
            let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);
            let kernel_clone = Arc::clone(self);
            let message_owned = message.to_string();
            let entry_clone = entry.clone();

            let handle = tokio::spawn(async move {
                let result = if is_wasm {
                    kernel_clone
                        .execute_wasm_agent(&entry_clone, &message_owned, kernel_handle)
                        .await
                } else {
                    kernel_clone
                        .execute_python_agent(&entry_clone, agent_id, &message_owned)
                        .await
                };

                match result {
                    Ok(result) => {
                        // Emit the complete response as a single text delta
                        let _ = tx
                            .send(StreamEvent::TextDelta {
                                text: result.response.clone(),
                            })
                            .await;
                        let _ = tx
                            .send(StreamEvent::ContentComplete {
                                stop_reason: carrier_types::message::StopReason::EndTurn,
                                usage: result.total_usage,
                            })
                            .await;
                        kernel_clone
                            .runtime
                            .scheduler
                            .record_usage(agent_id, &result.total_usage);
                        if let Err(e) = kernel_clone
                            .registry
                            .set_state(agent_id, AgentState::Running)
                        {
                            tracing::warn!(agent_id = %agent_id, error = %e, "Failed to set agent state to Running");
                        }
                        Ok(result)
                    }
                    Err(e) => {
                        kernel_clone.runtime.supervisor.record_panic();
                        warn!(agent_id = %agent_id, error = %e, "Non-LLM agent failed");
                        Err(e)
                    }
                }
            });

            return Ok((rx, handle));
        }

        // LLM agent: true streaming via agent loop
        let ctx = self
            .prepare_agent_context(
                agent_id,
                message,
                &entry,
                &sender_id,
                sender_name,
                &owner_id,
                &channel_type,
                None,
                None,
                active_flow,
                None,
                None,
            )
            .await?;
        let PreparedContext {
            mut session,
            needs_compact,
            tools,
            manifest,
            driver,
            ctx_window,
            ..
        } = ctx;

        let (tx, rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);

        let memory = Arc::clone(&self.memory);
        // Build link context from user message (auto-extract URLs for the agent)
        let message_owned = if let Some(link_ctx) =
            carrier_runtime::link_understanding::build_link_context(message, &self.config.links)
        {
            format!("{message}{link_ctx}")
        } else {
            message.to_string()
        };
        let kernel_clone = Arc::clone(self);

        let handle = tokio::spawn(async move {
            // Clone Brain Arc before any .await so the RwLockReadGuard is dropped (not Send).
            let brain_ref: Option<Arc<dyn carrier_runtime::llm_driver::Brain>> = Some(Arc::clone(
                &*kernel_clone.brain.brain.read().unwrap_or_else(|e| {
                    warn!("Brain RwLock poisoned, recovering");
                    e.into_inner()
                }),
            )
                as Arc<dyn carrier_runtime::llm_driver::Brain>);

            // Extract MemoryHandle from kernel.
            let memory_handle: Option<Arc<dyn carrier_runtime::memory_handle::MemoryHandle>> = Some(
                crate::handle::make_memory_handle(Arc::clone(&kernel_clone.memory)),
            );

            // Auto-compact if the session is large before running the loop
            if needs_compact {
                info!(agent_id = %agent_id, messages = session.messages.len(), "Auto-compacting session");
                match kernel_clone
                    .compact_agent_session(
                        agent_id,
                        session.id,
                        owner_id.as_deref(),
                        sender_id.as_deref(),
                    )
                    .await
                {
                    Ok(msg) => {
                        info!(agent_id = %agent_id, "{msg}");
                        // Reload the session after compaction
                        if let Ok(Some(reloaded)) = memory.get_session_async(session.id).await {
                            session = reloaded;
                        }
                    }
                    Err(e) => {
                        warn!(agent_id = %agent_id, "Auto-compaction failed: {e}");
                    }
                }
            }

            // Create a phase callback that emits PhaseChange events to WS/SSE clients
            let phase_tx = tx.clone();
            let phase_cb: carrier_runtime::agent_loop::PhaseCallback =
                std::sync::Arc::new(move |phase| {
                    use carrier_runtime::agent_loop::LoopPhase;
                    let (phase_str, detail) = match &phase {
                        LoopPhase::Thinking => ("thinking".to_string(), None),
                        LoopPhase::ToolUse { tool_name } => {
                            ("tool_use".to_string(), Some(tool_name.clone()))
                        }
                        LoopPhase::Streaming => ("streaming".to_string(), None),
                        LoopPhase::Done => ("done".to_string(), None),
                        LoopPhase::Error => ("error".to_string(), None),
                    };
                    let event = StreamEvent::PhaseChange {
                        phase: phase_str,
                        detail,
                    };
                    let _ = phase_tx.try_send(event);
                });

            let result = run_agent_loop_streaming(
                &manifest,
                &message_owned,
                &mut session,
                &memory,
                driver,
                &tools,
                kernel_handle,
                tx,
                Some(&kernel_clone.plugins.mcp_connections),
                manifest.workspace.as_deref(),
                Some(&phase_cb),
                Some(&kernel_clone.coordination.hooks),
                ctx_window,
                Some(&kernel_clone.coordination.process_manager),
                None,                  // content_blocks (streaming path uses text only for now)
                brain_ref.clone(),     // Brain for modality-based routing
                memory_handle.clone(), // Memory handle for kv/tree operations
                sender_id.as_deref(),
                owner_id.as_deref(),
                channel_type.as_deref(),
                Some(kernel_clone.runtime.llm_concurrency_limit.clone()),
            )
            .await;

            // Drop the phase callback immediately after the streaming loop
            // completes. It holds a clone of the stream sender (`tx`), which
            // keeps the mpsc channel alive. If we don't drop it here, the
            // WS/SSE stream_task won't see channel closure until this entire
            // spawned task exits (after all post-processing below). This was
            // causing 20-45s hangs where the client received phase:done but
            // never got the response event (the upstream WS would die from
            // ping timeout before post-processing finished).
            drop(phase_cb);

            match result {
                Ok(mut result) => {
                    // Clean up running_tasks entry
                    kernel_clone.runtime.running_tasks.remove(&agent_id);

                    // [CLONE_INSTALL:<name>] markers: kernel-side clone install
                    // at turn end (staging/<name>/ → real workspace → spawn).
                    // Runs before anything reads result.response downstream
                    // (webui SSE / evolution / mirrors all see the receipt).
                    kernel_clone
                        .process_clone_install_markers(
                            &manifest.name,
                            manifest.workspace.as_deref(),
                            &mut result.response,
                        )
                        .await;

                    // task_plan in streaming path: log warning, plan not auto-executed
                    // (streaming clients expect real-time output; plan execution is for
                    // non-streaming/cron paths)
                    if result.plan.is_some() {
                        warn!("task_plan produced in streaming path — plan execution skipped (not supported in streaming mode)");
                        result.plan = None;
                    }

                    // Evolution hook — post-conversation auto-learning for clones
                    kernel_clone.maybe_run_evolution(
                        &manifest,
                        &message_owned,
                        &result.response,
                        owner_id.as_deref(),
                        sender_id.as_deref(),
                    );

                    // Multi-tenancy: update user profile
                    if let Some(ref sid) = &sender_id {
                        touch_user_profile(
                            &kernel_clone.config.home_dir,
                            owner_id.as_deref().unwrap_or(sid),
                            &manifest.name,
                            Some(sid),
                        );
                    }

                    // Write JSONL session mirror to workspace
                    if let Some(ref workspace) = manifest.workspace {
                        if let Err(e) = memory.write_jsonl_mirror(
                            &session,
                            &workspace.join("sessions"),
                            owner_id.as_deref(),
                            sender_id.as_deref(),
                            Some(&kernel_clone.config.home_dir),
                            Some(&manifest.name),
                        ) {
                            warn!("Failed to write JSONL session mirror (streaming): {e}");
                        }
                        // Append daily memory log (best-effort)
                        append_daily_memory_log(
                            &kernel_clone.config.home_dir,
                            &manifest.name,
                            &result.response,
                            owner_id.as_deref(),
                            sender_id.as_deref(),
                        );
                    }

                    kernel_clone
                        .runtime
                        .scheduler
                        .record_usage(agent_id, &result.total_usage);

                    // Persist usage and check budget thresholds
                    let model = manifest.model.modality.clone();
                    match kernel_clone.metering.record_and_check(
                        &carrier_memory::usage::UsageRecord {
                            agent_id,
                            model: model.clone(),
                            input_tokens: result.total_usage.input_tokens,
                            output_tokens: result.total_usage.output_tokens,
                            tool_calls: result.iterations.saturating_sub(1),
                        },
                    ) {
                        Ok(Some(alert)) => kernel_clone.handle_budget_alert(&alert),
                        Err(e) => warn!("Failed to record metering: {e}"),
                        _ => {}
                    }

                    if let Err(e) = kernel_clone
                        .registry
                        .set_state(agent_id, AgentState::Running)
                    {
                        tracing::warn!(agent_id = %agent_id, error = %e, "Failed to set agent state to Running");
                    }

                    // Post-loop compaction check: if session now exceeds token threshold,
                    // trigger compaction in background for the next call.
                    {
                        use carrier_runtime::compactor::{
                            estimate_token_count, needs_compaction_by_tokens, CompactionConfig,
                        };
                        let config = CompactionConfig::default();
                        let estimated = estimate_token_count(&session.messages, None, None);
                        if needs_compaction_by_tokens(estimated, &config) {
                            let compact_session_id = session.id;
                            // Clone owner/sender so the background compaction can
                            // write its summary back to the right kv partition.
                            let oid = owner_id.clone();
                            let sid = sender_id.clone();
                            let kc = kernel_clone.clone();
                            tokio::spawn(async move {
                                info!(agent_id = %agent_id, estimated_tokens = estimated, "Post-loop compaction triggered");
                                if let Err(e) = kc
                                    .compact_agent_session(
                                        agent_id,
                                        compact_session_id,
                                        oid.as_deref(),
                                        sid.as_deref(),
                                    )
                                    .await
                                {
                                    warn!(agent_id = %agent_id, "Post-loop compaction failed: {e}");
                                }
                            });
                        }
                    }

                    Ok(result)
                }
                Err(e) => {
                    // Clean up running_tasks entry
                    kernel_clone.runtime.running_tasks.remove(&agent_id);

                    kernel_clone.runtime.supervisor.record_panic();
                    warn!(agent_id = %agent_id, error = %e, "Streaming agent loop failed");
                    Err(KernelError::Carrier(e))
                }
            }
        });

        // Store abort handle for cancellation support
        self.runtime
            .running_tasks
            .insert(agent_id, handle.abort_handle());

        Ok((rx, handle))
    }

    // ── Module dispatch: WASM / Python / LLM ───────────────────

    /// Execute a WASM module agent.
    ///
    /// Loads the `.wasm` or `.wat` file, maps manifest capabilities into
    /// `SandboxConfig`, and runs through the `WasmSandbox` engine.
    async fn execute_wasm_agent(
        &self,
        entry: &AgentEntry,
        message: &str,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
    ) -> KernelResult<AgentLoopResult> {
        let module_path = entry.manifest.module.strip_prefix("wasm:").unwrap_or("");
        let wasm_path = self.resolve_module_path(module_path);

        info!(agent = %entry.name, path = %wasm_path.display(), "Executing WASM agent");

        let wasm_bytes = std::fs::read(&wasm_path).map_err(|e| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "Failed to read WASM module '{}': {e}",
                wasm_path.display()
            )))
        })?;

        // Map manifest capabilities to sandbox capabilities
        let caps = manifest_to_capabilities(&entry.manifest);
        let sandbox_config = SandboxConfig {
            fuel_limit: entry.manifest.resources.max_cpu_time_ms * 100_000,
            max_memory_bytes: entry.manifest.resources.max_memory_bytes as usize,
            capabilities: caps,
            timeout_secs: Some(30),
        };

        let input = serde_json::json!({
            "message": message,
            "agent_id": entry.id.to_string(),
            "agent_name": entry.name,
        });

        let result = self
            .runtime
            .wasm_sandbox
            .execute(
                &wasm_bytes,
                input,
                sandbox_config,
                kernel_handle,
                &entry.id.to_string(),
            )
            .await
            .map_err(|e| {
                KernelError::Carrier(CarrierError::Internal(format!(
                    "WASM execution failed: {e}"
                )))
            })?;

        // Extract response text from WASM output JSON
        let response = result
            .output
            .get("response")
            .and_then(|v| v.as_str())
            .or_else(|| result.output.get("text").and_then(|v| v.as_str()))
            .or_else(|| result.output.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| serde_json::to_string(&result.output).unwrap_or_default());

        info!(
            agent = %entry.name,
            fuel_consumed = result.fuel_consumed,
            "WASM agent execution complete"
        );

        Ok(AgentLoopResult {
            response,
            total_usage: carrier_types::message::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            iterations: 1,
            silent: false,
            directives: Default::default(),
            plan: None,
        })
    }

    /// Execute a Python script agent.
    ///
    /// Delegates to `python_runtime::run_python_agent()` via subprocess.
    async fn execute_python_agent(
        &self,
        entry: &AgentEntry,
        agent_id: AgentId,
        message: &str,
    ) -> KernelResult<AgentLoopResult> {
        let script_path = entry.manifest.module.strip_prefix("python:").unwrap_or("");
        let resolved_path = self.resolve_module_path(script_path);

        info!(agent = %entry.name, path = %resolved_path.display(), "Executing Python agent");

        let config = PythonConfig {
            timeout_secs: (entry.manifest.resources.max_cpu_time_ms / 1000).max(30),
            working_dir: Some(
                resolved_path
                    .parent()
                    .unwrap_or(Path::new("."))
                    .to_string_lossy()
                    .to_string(),
            ),
            ..PythonConfig::default()
        };

        let context = serde_json::json!({
            "agent_name": entry.name,
            "system_prompt": entry.manifest.model.system_prompt,
        });

        let result = python_runtime::run_python_agent(
            &resolved_path.to_string_lossy(),
            &agent_id.to_string(),
            message,
            &context,
            &config,
        )
        .await
        .map_err(|e| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "Python execution failed: {e}"
            )))
        })?;

        info!(agent = %entry.name, "Python agent execution complete");

        Ok(AgentLoopResult {
            response: result.response,
            total_usage: carrier_types::message::TokenUsage {
                input_tokens: 0,
                output_tokens: 0,
            },
            iterations: 1,
            silent: false,
            directives: Default::default(),
            plan: None,
        })
    }

    /// 借用机制 · 离线会话轮 — 会话以票据进、以票据出，主人服务器零持久化。
    ///
    /// 复用 `run_agent_loop`，但喂它一块内存级 `MemorySubstrate`（事件日志落在
    /// 每轮独立 temp 目录、轮末销毁），跳过 `prepare_agent_context` 的 session
    /// 加载 + kv 持久化路径。借用者在主人服务器没有持久记忆：drawer/recall/tree
    /// 一律为空，kv/tree 工具只写进临时 substrate、随轮销毁。
    ///
    /// 与 `execute_llm_agent` 是平行的新路径——生产热路径零改动。不做自动
    /// flow classify；要 flow 就传显式 `active_flow`（确定性按名加载）。
    #[allow(clippy::too_many_arguments)]
    pub async fn run_borrowed_turn(
        &self,
        agent_id: AgentId,
        ticket: carrier_memory::session::SessionTicket,
        message: &str,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        active_flow: Option<&str>,
        materials: &[BorrowedMaterial],
        stream_tx: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
        borrower: Option<&str>,
    ) -> KernelResult<BorrowedTurnOutcome> {
        // 准入与配额：enabled 总开关 → allowlist 名单门（名单非空时）→
        // 每小时轮次上限（按 borrower+agent 计数）。本地直连（borrower 为空）
        // 免名单门但照常计数——主人自己用不受限，外部身份受双重闸。
        let policy = &self.config.borrow;
        let borrower_id = borrower.unwrap_or("local");
        if !policy.enabled {
            return Err(KernelError::Carrier(CarrierError::InvalidInput(
                "borrowing is disabled on this carrier".to_string(),
            )));
        }
        if borrower.is_some()
            && !policy.allow_borrowers.is_empty()
            && !policy.allow_borrowers.iter().any(|a| a == borrower_id)
        {
            warn!(borrower = %borrower_id, agent = %agent_id, "borrow request rejected by allowlist");
            return Err(KernelError::Carrier(CarrierError::InvalidInput(format!(
                "borrower {:?} is not allowed to borrow this agent",
                borrower_id
            ))));
        }
        if policy.max_turns_per_hour > 0 {
            let used = self.record_borrow_usage(borrower_id, &agent_id.to_string())?;
            if used > policy.max_turns_per_hour {
                warn!(borrower = %borrower_id, agent = %agent_id, used, limit = policy.max_turns_per_hour, "borrow rate limit exceeded");
                return Err(KernelError::Carrier(CarrierError::InvalidInput(format!(
                    "borrow rate limit exceeded: {} turns/hour (used {})",
                    policy.max_turns_per_hour, used
                ))));
            }
        }

        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;
        let agent_name = entry.name.clone();
        let mut manifest = entry.manifest.clone();

        // 会话从票据还原（全新 session id——服务器不记忆会话身份，票据即身份）。
        let mut session = carrier_memory::session::Session::from_ticket(ticket, agent_name.clone());
        if session.label.is_none() {
            session.label = Some(format!("borrow:{agent_name}"));
        }

        // 内存级 substrate + 每轮独立事件目录（轮末销毁）——借用轮零持久化。
        let events_dir =
            std::env::temp_dir().join(format!("aginx-borrow-{}", uuid::Uuid::new_v4()));
        let ephemeral = Arc::new(
            carrier_memory::MemorySubstrate::open_in_memory_with_events(events_dir.clone())
                .map_err(KernelError::Carrier)?,
        );

        let mut tools = self.resolve_tools(&entry);

        // 借用轮产物通道依赖 file_write，但 CORE_TOOL_NAMES 不含它（生产路径
        // 靠 flow 注入发现）——借用轮没有持久会话可等注入，直接补挂。
        if !tools.iter().any(|t| t.name == "file_write") {
            if let Some(fw) =
                carrier_runtime::tool_runner::builtin_tool_definitions(self.config.cli_exec.clone())
                    .into_iter()
                    .find(|t| t.name == "file_write")
            {
                tools.push(fw);
            }
        }

        // 显式 flow（确定性按名加载，不接 send-time classifier）。
        let mut auto_matched_flow: Option<String> = None;
        if let Some(flow_name) = active_flow {
            if let Some(ws) = manifest.workspace.as_ref() {
                if let Some(flow) = crate::prompt_sources::load_flow_by_name(ws, flow_name) {
                    Self::apply_flow_elevation(&mut tools, &mut manifest, &flow, &agent_name);
                    auto_matched_flow = Some(flow.name.clone());
                } else {
                    warn!(agent = %agent_name, flow = %flow_name, "borrowed turn: active_flow not found, running bare");
                }
            }
        }

        // Prompt：借用者无持久记忆——drawer/recall 一律为空（无状态的核心）。
        let turn_summaries = session.turn_summaries.clone();
        let sender_id: Option<String> = None;
        let owner_id: Option<String> = None;
        self.build_and_apply_prompt(
            &mut manifest,
            &tools,
            &sender_id,
            None,
            &owner_id,
            auto_matched_flow,
            turn_summaries,
            Vec::new(),
            Vec::new(),
            None,
            None,
        );
        // 失忆兜底：分身的记忆习惯先查 kv，借用轮 kv 为空是设计——没有这段
        // 指引时模型会无视 transcript 里的完整历史，自称"失忆"要用户重发
        // （08-23 互借 E2E 实证一次，同票据复跑即对，纯行为非接线）。
        manifest.model.system_prompt.push_str(
            "\n\n【借用轮须知】\n- 本轮为无状态借用：kv/记忆存储为空是设计使然（主人服务器对借用者零持久化），不是你失忆了。\n- 完整会话历史已随本轮对话消息送入（用户以会话票据带入，跨轮连续）。回答与历史相关的问题时从对话历史中查找，不要因 kv 为空就宣称不记得或要求用户重发。\n- 本轮结束一切销毁：需要跨轮记住的事实直接写进回复，回复会随会话票据回到用户侧。\n",
        );

        // 3.2 素材内存级生命周期：借用者素材落 <ws>/borrow/<uuid>/materials/，
        // 仅本轮可读（走 agent 自己的 file 工具沙箱），轮末整目录销毁。
        // 开轮先清扫残留——借道轮不跨进程存活，borrow/ 下的一切都是死轮遗留。
        // 3.3：同目录 output/ 是产物区，agent 写这里的文件轮末随响应回流。
        // 无素材轮也建目录（output/ 提示段始终注入，产物区恒可用）。
        #[allow(unused_assignments)]
        let mut borrow_turn_dir: Option<std::path::PathBuf> = None;
        {
            let ws = manifest.workspace.as_ref().ok_or_else(|| {
                KernelError::Carrier(CarrierError::InvalidInput(
                    "borrowed materials require an agent workspace".to_string(),
                ))
            })?;
            let mut total = 0usize;
            for m in materials {
                let p = std::path::Path::new(&m.name);
                if m.name.contains('\\')
                    || p.is_absolute()
                    || p.components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(KernelError::Carrier(CarrierError::InvalidInput(format!(
                        "bad borrowed material name: {:?}",
                        m.name
                    ))));
                }
                if m.content.len() > BORROWED_MATERIAL_MAX_FILE {
                    return Err(KernelError::Carrier(CarrierError::InvalidInput(format!(
                        "borrowed material {:?} exceeds {}B limit",
                        m.name, BORROWED_MATERIAL_MAX_FILE
                    ))));
                }
                total += m.content.len();
            }
            if total > BORROWED_MATERIAL_MAX_TOTAL {
                return Err(KernelError::Carrier(CarrierError::InvalidInput(format!(
                    "borrowed materials total {total}B exceeds {BORROWED_MATERIAL_MAX_TOTAL}B limit"
                ))));
            }

            let borrow_root = ws.join("borrow");
            if let Err(e) = std::fs::remove_dir_all(&borrow_root) {
                // NotFound 是正常首态；其他错误不阻断（best-effort 清扫）。
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(dir = %borrow_root.display(), error = %e, "borrow sweep failed");
                }
            }
            let turn_root = borrow_root.join(uuid::Uuid::new_v4().simple().to_string());
            std::fs::create_dir_all(turn_root.join("materials")).map_err(|e| {
                KernelError::Carrier(CarrierError::Internal(format!(
                    "borrowed materials staging failed: {e}"
                )))
            })?;
            for m in materials {
                let dest = turn_root.join("materials").join(&m.name);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        KernelError::Carrier(CarrierError::Internal(format!(
                            "borrowed material dir failed: {e}"
                        )))
                    })?;
                }
                std::fs::write(&dest, &m.content).map_err(|e| {
                    KernelError::Carrier(CarrierError::Internal(format!(
                        "borrowed material write failed: {e}"
                    )))
                })?;
            }
            // Prompt 提示：素材相对路径（相对 agent workspace）。
            let mats_rel = format!(
                "borrow/{}/materials",
                turn_root.file_name().unwrap().to_string_lossy()
            );
            let out_rel = format!(
                "borrow/{}/output",
                turn_root.file_name().unwrap().to_string_lossy()
            );
            manifest.model.system_prompt.push_str(&format!(
                "\n\n【借用轮素材】用户本轮提交的素材在 `{mats_rel}/` 下（相对 workspace，仅本轮有效，轮末销毁）。请用 file_read/file_list 等工具按需读取。\n【借用轮产物】需要交付给用户的文件请写到 `{out_rel}/` 下（相对 workspace，轮末自动回传给用户并销毁，主机不留存）。"
            ));
            borrow_turn_dir = Some(turn_root);
        }

        let driver = self.resolve_driver(&manifest)?;

        let brain_ref: Option<Arc<dyn carrier_runtime::llm_driver::Brain>> =
            Some(Arc::clone(&*self.brain.brain.read().unwrap_or_else(|e| {
                warn!("Brain RwLock poisoned, recovering");
                e.into_inner()
            }))
                as Arc<dyn carrier_runtime::llm_driver::Brain>);

        // In-process memory handle over the ephemeral substrate——kv/tree 工具
        // 写进临时 substrate（随轮销毁），且绝不走 aginxMemory 外置路由（防借用者
        // 记忆泄漏进共享 PG）。
        let memory_handle: Arc<dyn carrier_runtime::memory_handle::MemoryHandle> = Arc::new(
            crate::handle::MemorySubstrateHandle::new(Arc::clone(&ephemeral)),
        );

        let loop_result = run_agent_loop(
            &manifest,
            message,
            &mut session,
            &ephemeral,
            driver,
            &tools,
            kernel_handle,
            stream_tx,
            Some(&self.plugins.mcp_connections),
            manifest.workspace.as_deref(),
            None,
            Some(&self.coordination.hooks),
            None,
            Some(&self.coordination.process_manager),
            None,
            brain_ref,
            Some(memory_handle),
            None,
            None,
            Some("borrow"),
            Some(self.runtime.llm_concurrency_limit.clone()),
        )
        .await;

        // 轮末销毁——无论成败：事件目录 + 借道素材目录，主人服务器零持久化。
        if let Err(e) = std::fs::remove_dir_all(&events_dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                warn!(dir = %events_dir.display(), error = %e, "borrowed-turn events dir cleanup failed");
            }
        }

        let result = loop_result.map_err(KernelError::Carrier)?;

        // 3.3 产物回传：销毁前收集 output/ 下的文件（预算内），随响应回流用户侧。
        let mut files: Vec<BorrowedOutputFile> = Vec::new();
        let mut skipped_oversize = 0u32;
        if let Some(dir) = &borrow_turn_dir {
            use base64::Engine as _;
            let out_dir = dir.join("output");
            let mut budget_left = BORROWED_OUTPUT_MAX_TOTAL;
            let mut stack = vec![out_dir.clone()];
            while let Some(d) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&d) else {
                    continue;
                };
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        stack.push(p);
                        continue;
                    }
                    let Ok(meta) = e.metadata() else { continue };
                    if !meta.is_file() || meta.len() == 0 {
                        continue;
                    }
                    if meta.len() as usize > BORROWED_OUTPUT_MAX_FILE
                        || meta.len() as usize > budget_left
                    {
                        skipped_oversize += 1;
                        continue;
                    }
                    match (|| -> Result<(String, Vec<u8>), Box<dyn std::error::Error>> {
                        let name = p.strip_prefix(&out_dir)?.to_string_lossy().into_owned();
                        Ok((name, std::fs::read(&p)?))
                    })() {
                        Ok((name, content)) => {
                            budget_left -= content.len();
                            files.push(BorrowedOutputFile {
                                name,
                                content_base64: base64::engine::general_purpose::STANDARD
                                    .encode(content),
                            });
                        }
                        Err(e) => {
                            warn!(path = %p.display(), error = %e, "borrowed output collect failed")
                        }
                    }
                }
            }
            files.sort_by(|a, b| a.name.cmp(&b.name));
        }

        // 销毁借道素材/产物目录（回传已在手）。
        if let Some(dir) = borrow_turn_dir {
            if let Err(e) = std::fs::remove_dir_all(&dir) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    warn!(dir = %dir.display(), error = %e, "borrowed-turn materials dir cleanup failed");
                }
            }
        }
        if skipped_oversize > 0 {
            warn!(
                skipped = skipped_oversize,
                "borrowed outputs over budget not returned"
            );
        }

        let outcome = BorrowedTurnOutcome {
            response: result.response,
            silent: result.silent,
            ticket: session.to_ticket(),
            total_usage: result.total_usage,
            iterations: result.iterations,
            files,
        };

        Ok(outcome)
    }

    /// 借用用量台账：追加一行 `{ts, borrower, agent}` 到
    /// `{data_dir}/borrow_usage.jsonl` 并返回该 borrower+agent 过去一小时
    /// 的总轮次（含本次）。台账是唯一状态——与借用轮的无状态原则一致，
    /// 它只记录"谁借了几次"，不承载任何会话内容。
    fn record_borrow_usage(&self, borrower: &str, agent: &str) -> KernelResult<u32> {
        use std::io::Write as _;

        let path = self.config.data_dir.join("borrow_usage.jsonl");
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                KernelError::Carrier(CarrierError::Internal(format!(
                    "borrow usage dir failed: {e}"
                )))
            })?;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!(
            "{}\n",
            serde_json::json!({"ts": now, "borrower": borrower, "agent": agent})
        );
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| f.write_all(line.as_bytes()))
            .map_err(|e| {
                KernelError::Carrier(CarrierError::Internal(format!(
                    "borrow usage append failed: {e}"
                )))
            })?;

        // 滑窗计数：读全量（个人安装规模下极小），数 borrower+agent 一小时内
        // 的行数——本次已追加，天然计入。
        let window_start = now.saturating_sub(3600);
        let mut used: u32 = 0;
        if let Ok(content) = std::fs::read_to_string(&path) {
            for l in content.lines() {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(l) else {
                    continue;
                };
                if v.get("borrower").and_then(|b| b.as_str()) == Some(borrower)
                    && v.get("agent").and_then(|a| a.as_str()) == Some(agent)
                    && v.get("ts").and_then(|t| t.as_u64()).unwrap_or(0) >= window_start
                {
                    used += 1;
                }
            }
        }
        Ok(used)
    }

    /// Execute the default LLM-based agent loop.
    #[allow(clippy::too_many_arguments)]
    async fn execute_llm_agent(
        &self,
        entry: &AgentEntry,
        agent_id: AgentId,
        message: &str,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        content_blocks: Option<Vec<carrier_types::message::ContentBlock>>,
        sender_id: Option<String>,
        sender_name: Option<String>,
        owner_id: Option<String>,
        channel_type: Option<String>,
        task_id: Option<String>,
        resume: Option<&carrier_memory::FlowRunRow>,
        active_flow: Option<&str>,
        session_label: Option<&str>,
        chain_id: Option<String>,
    ) -> KernelResult<AgentLoopResult> {
        // Prepare shared context (session, tools, flow/subagent matching, manifest)
        let ctx = self
            .prepare_agent_context(
                agent_id,
                message,
                entry,
                &sender_id,
                sender_name,
                &owner_id,
                &channel_type,
                task_id.as_deref(),
                resume,
                active_flow,
                session_label,
                chain_id,
            )
            .await?;
        let PreparedContext {
            mut session,
            needs_compact,
            tools,
            manifest,
            flow,
            ..
        } = ctx;

        // Execute compaction if needed
        if needs_compact {
            match self
                .compact_agent_session(
                    agent_id,
                    session.id,
                    owner_id.as_deref(),
                    sender_id.as_deref(),
                )
                .await
            {
                Ok(msg) => {
                    info!(agent_id = %agent_id, "{msg}");
                    if let Ok(Some(reloaded)) = self.memory.get_session_async(session.id).await {
                        session = reloaded;
                    }
                }
                Err(e) => {
                    warn!(agent_id = %agent_id, "Pre-emptive compaction failed: {e}");
                }
            }
        }

        // Re-acquire Brain reference for LLM call and plan execution
        let brain_ref: Option<Arc<dyn carrier_runtime::llm_driver::Brain>> =
            Some(Arc::clone(&*self.brain.brain.read().unwrap_or_else(|e| {
                warn!("Brain RwLock poisoned, recovering");
                e.into_inner()
            }))
                as Arc<dyn carrier_runtime::llm_driver::Brain>);

        // Extract MemoryHandle from kernel.
        let memory_handle: Option<Arc<dyn carrier_runtime::memory_handle::MemoryHandle>> =
            Some(crate::handle::make_memory_handle(Arc::clone(&self.memory)));

        // Model routing is handled by Brain

        let driver = self.resolve_driver(&manifest)?;

        // Context window lookup disabled — model name managed by Brain
        let ctx_window: Option<usize> = None;

        // Snapshot output directory before the agent loop to detect new files
        // (recursive relative paths under output/, for automatic view_url append).
        let output_dir_before = sender_id.as_ref().and_then(|sid| {
            manifest.workspace.as_ref().map(|_ws| {
                let oid = owner_id.as_deref().unwrap_or(sid);
                let dir = carrier_types::config::sender_data_dir(
                    &self.config.home_dir,
                    oid,
                    &manifest.name,
                    Some(sid),
                )
                .join("output");
                let existing = list_output_rel_paths(&dir);
                (dir, existing)
            })
        });

        // Build link context from user message (auto-extract URLs for the agent)
        let message_with_links = if let Some(link_ctx) =
            carrier_runtime::link_understanding::build_link_context(message, &self.config.links)
        {
            format!("{message}{link_ctx}")
        } else {
            message.to_string()
        };

        // Resume guard: if we came in to resume a flow but its definition is no
        // longer findable (deleted/renamed between suspend and resume), mark the
        // run failed and fall back to a normal single-step reply.
        let resume = match (resume, &flow) {
            (Some(rf), None) => {
                let completed = rf.completed_steps.clone();
                let _ = self
                    .memory
                    .flow_runs()
                    .update_status(&rf.run_id, "failed", &completed);
                warn!(agent = %entry.name, run_id = %rf.run_id, flow = %rf.flow_name, "resume aborted: flow def not found, marked failed");
                None
            }
            (r, _) => r,
        };

        let is_multi_step = flow
            .as_ref()
            .is_some_and(|fm| !fm.flow_def.steps.is_empty());

        let mut result = if is_multi_step {
            // Multi-step flow: execute as a DAG via run_flow.
            let fm = flow.as_ref().expect("checked above");
            let base_prompt = manifest.model.system_prompt.clone();
            // Build resume state when continuing a suspended flow.
            let resume_state = resume.map(|rf| {
                let pre_outputs: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_str(&rf.completed_steps).unwrap_or_default();
                let waiting_step_id = rf.waiting_at.clone().unwrap_or_default();
                let cancel_keywords = fm
                    .flow_def
                    .steps
                    .iter()
                    .find(|s| s.id == waiting_step_id)
                    .map(|s| s.cancel_keywords.clone())
                    .unwrap_or_default();
                // map_context is set when the waiting step is inside an
                // interactive map body (stage E.2); None for a top-level
                // user_input. exec_interactive_map recomputes body cancel
                // keywords itself, so cancel_keywords stays empty here.
                let map_context = rf
                    .map_context
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<crate::flow_runner::MapContext>(s).ok());
                info!(agent = %entry.name, flow = %fm.name, run_id = %rf.run_id, step = %waiting_step_id, "Resuming suspended flow");
                crate::flow_runner::ResumeState {
                    run_id: rf.run_id.clone(),
                    pre_outputs,
                    waiting_step_id,
                    user_reply: message_with_links.clone(),
                    cancel_keywords,
                    map_context,
                }
            });
            info!(
                agent = %entry.name,
                flow = %fm.name,
                steps = fm.flow_def.steps.len(),
                resuming = resume_state.is_some(),
                "Executing multi-step flow via run_flow"
            );
            let outcome = self
                .run_flow(
                    agent_id,
                    &fm.flow_def,
                    &base_prompt,
                    &message_with_links,
                    &mut session,
                    &manifest,
                    &tools,
                    brain_ref.as_ref(),
                    kernel_handle.clone(),
                    sender_id.as_deref(),
                    owner_id.as_deref(),
                    channel_type.as_deref(),
                    resume_state.as_ref(),
                    None,
                )
                .await?;
            match outcome {
                crate::flow_runner::FlowOutcome::Completed { result, .. } => result,
                crate::flow_runner::FlowOutcome::Suspended {
                    question,
                    total_usage,
                    iterations,
                } => {
                    // The flow paused at a `user_input` step: the question IS the
                    // reply to send. Skip plan/file/evolution post-processing.
                    let r = AgentLoopResult {
                        response: question,
                        total_usage,
                        iterations,
                        silent: false,
                        directives: Default::default(),
                        plan: None,
                    };
                    return self
                        .finalize_suspended(r, agent_id, &manifest, &session, &sender_id, &owner_id)
                        .await;
                }
            }
        } else {
            run_agent_loop(
                &manifest,
                &message_with_links,
                &mut session,
                &self.memory,
                driver,
                &tools,
                kernel_handle.clone(),
                None, // stream_tx: non-streaming path
                Some(&self.plugins.mcp_connections),
                manifest.workspace.as_deref(),
                None, // on_phase callback
                Some(&self.coordination.hooks),
                ctx_window,
                Some(&self.coordination.process_manager),
                content_blocks,
                brain_ref.clone(),     // Brain for modality-based routing
                memory_handle.clone(), // Memory handle for kv/tree operations
                sender_id.as_deref(),
                owner_id.as_deref(),
                channel_type.as_deref(),
                Some(self.runtime.llm_concurrency_limit.clone()),
            )
            .await
            .map_err(KernelError::Carrier)?
        };

        // Detect new output files and append download URLs to the response

        // If agent produced a task_plan, execute it
        if let Some(plan) = result.plan.take() {
            info!(
                agent = %entry.name,
                plan_title = %plan.title,
                steps = plan.steps.len(),
                "Executing task_plan"
            );
            result = self
                .execute_plan(
                    agent_id,
                    &plan,
                    &manifest,
                    &tools,
                    brain_ref.as_ref(),
                    kernel_handle.clone(),
                    sender_id.clone(),
                    owner_id.clone(),
                    channel_type.clone(),
                )
                .await?;
        }

        if let (Some((dir, before)), Some(ref sid), Some(ref ext_url)) =
            (&output_dir_before, &sender_id, &self.config.external_url)
        {
            let after = list_output_rel_paths(dir);
            let mut new_files: Vec<String> =
                after.into_iter().filter(|f| !before.contains(f)).collect();
            new_files.sort();
            if !new_files.is_empty() {
                // Prefer /api/files/view/{agent_name}/… so links work like the file explorer.
                let mut links: Vec<String> = Vec::new();
                for rel in &new_files {
                    let under_sender = format!("output/{rel}");
                    if let Some(url) = carrier_runtime::file_view::build_file_view_url(
                        Some(ext_url.as_str()),
                        &manifest.name,
                        &under_sender,
                        sid,
                    ) {
                        links.push(url);
                    }
                }
                // Avoid duplicating links the agent already pasted from tool view_url.
                let existing = result.response.clone();
                links.retain(|u| !existing.contains(u));
                if !links.is_empty() {
                    result.response.push_str("\n\n📎 生成的文件:\n");
                    for link in &links {
                        result.response.push_str(&format!("- {link}\n"));
                    }
                }
            }
        }

        // Evolution hook — post-conversation auto-learning for clones
        self.maybe_run_evolution(
            &manifest,
            message,
            &result.response,
            owner_id.as_deref(),
            sender_id.as_deref(),
        );

        // Multi-tenancy: update user profile (touch last_seen, increment conversation_count)
        if let Some(ref sid) = sender_id {
            touch_user_profile(
                &self.config.home_dir,
                owner_id.as_deref().unwrap_or(sid),
                &manifest.name,
                Some(sid),
            );
        }

        // Append new messages to canonical session for cross-channel memory

        // Write JSONL session mirror to workspace
        if let Some(ref workspace) = manifest.workspace {
            if let Err(e) = self.memory.write_jsonl_mirror(
                &session,
                &workspace.join("sessions"),
                owner_id.as_deref(),
                sender_id.as_deref(),
                Some(&self.config.home_dir),
                Some(&manifest.name),
            ) {
                warn!("Failed to write JSONL session mirror: {e}");
            }
            // Append daily memory log (best-effort)
            append_daily_memory_log(
                &self.config.home_dir,
                &manifest.name,
                &result.response,
                owner_id.as_deref(),
                sender_id.as_deref(),
            );
        }

        // Record usage and check budget thresholds
        let model = manifest.model.modality.clone();
        match self
            .metering
            .record_and_check(&carrier_memory::usage::UsageRecord {
                agent_id,
                model: model.clone(),
                input_tokens: result.total_usage.input_tokens,
                output_tokens: result.total_usage.output_tokens,
                tool_calls: result.iterations.saturating_sub(1),
            }) {
            Ok(Some(alert)) => self.handle_budget_alert(&alert),
            Err(e) => warn!("Failed to record metering: {e}"),
            _ => {}
        }

        Ok(result)
    }

    /// Light post-processing for a suspended flow: the `user_input` question is
    /// the reply the channel sends. Records the user-profile touch, JSONL
    /// session mirror, and metering, but skips plan execution, output-file
    /// detection, and evolution (those belong to a completed turn). The
    /// question was already appended to the session inside `run_flow`.
    async fn finalize_suspended(
        &self,
        r: AgentLoopResult,
        agent_id: AgentId,
        manifest: &AgentManifest,
        session: &carrier_memory::session::Session,
        sender_id: &Option<String>,
        owner_id: &Option<String>,
    ) -> KernelResult<AgentLoopResult> {
        if let Some(ref sid) = sender_id {
            touch_user_profile(
                &self.config.home_dir,
                owner_id.as_deref().unwrap_or(sid),
                &manifest.name,
                Some(sid),
            );
        }

        if let Some(ref workspace) = manifest.workspace {
            if let Err(e) = self.memory.write_jsonl_mirror(
                session,
                &workspace.join("sessions"),
                owner_id.as_deref(),
                sender_id.as_deref(),
                Some(&self.config.home_dir),
                Some(&manifest.name),
            ) {
                warn!("Failed to write JSONL session mirror: {e}");
            }
        }

        let model = manifest.model.modality.clone();
        match self
            .metering
            .record_and_check(&carrier_memory::usage::UsageRecord {
                agent_id,
                model,
                input_tokens: r.total_usage.input_tokens,
                output_tokens: r.total_usage.output_tokens,
                tool_calls: r.iterations.saturating_sub(1),
            }) {
            Ok(Some(alert)) => self.handle_budget_alert(&alert),
            Err(e) => warn!("Failed to record metering: {e}"),
            _ => {}
        }

        Ok(r)
    }

    /// Handle a budget threshold alert — log prominently and store for API exposure.
    pub(crate) fn handle_budget_alert(&self, alert: &crate::metering::BudgetAlert) {
        warn!(
            percent = alert.percent,
            used = alert.used_tokens,
            limit = alert.limit_tokens,
            "BUDGET ALERT: {}% of monthly token budget consumed ({}/{} tokens) — \
             configure alert_channel and alert_recipient in [budget] to receive notifications",
            alert.percent,
            alert.used_tokens,
            alert.limit_tokens
        );

        // Channel dispatch will be added in a follow-up via the plugin bridge.
        // The alert is exposed through the /api/budget endpoint and the
        // MeteringEngine's get_budget_status() method.
    }

    /// Resolve a module path relative to the kernel's home directory.
    ///
    /// If the path is absolute, return it as-is. Otherwise, resolve relative
    /// to `config.home_dir`.
    pub(crate) fn resolve_module_path(&self, path: &str) -> PathBuf {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.config.home_dir.join(path)
        }
    }

    /// Execute a task plan — run steps with topological ordering and parallel layers.
    #[allow(clippy::too_many_arguments)]
    async fn execute_plan(
        &self,
        agent_id: AgentId,
        plan: &carrier_runtime::agent_loop::TaskPlan,
        manifest: &AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        brain: Option<&Arc<dyn carrier_runtime::llm_driver::Brain>>,
        kernel_handle: Option<Arc<dyn KernelHandle>>,
        sender_id: Option<String>,
        owner_id: Option<String>,
        channel_type: Option<String>,
    ) -> KernelResult<AgentLoopResult> {
        use std::collections::HashMap;
        use std::sync::Arc;

        let mut step_outputs: HashMap<String, String> = HashMap::new();
        let mut total_usage = carrier_types::message::TokenUsage::default();
        let mut total_iterations = 0u32;

        let driver = self.resolve_driver(manifest)?;

        // Partition steps into parallel execution layers
        let layers = partition_steps_by_layers(&plan.steps);

        info!(
            plan_title = %plan.title,
            layers = layers.len(),
            total_steps = plan.steps.len(),
            "Plan execution starting"
        );

        // Cache parsed flows across the whole plan: load_flow_by_name re-reads
        // and re-parses the entire flows/ dir on each call, so a K-step plan
        // that names a flow per step would otherwise do K full dir scans.
        let mut flow_cache: HashMap<String, Option<crate::prompt_sources::FlowMatch>> =
            HashMap::new();

        for (layer_idx, layer) in layers.iter().enumerate() {
            let mut layer_handles = Vec::new();

            for step in layer {
                // Build step message: prompt + predecessor outputs
                let mut message = format!("## Task: {}\n\n{}", step.id, step.prompt);
                for dep_id in &step.depends_on {
                    if let Some(output) = step_outputs.get(dep_id) {
                        message.push_str(&format!(
                            "\n\n## Output from step '{}':\n{}",
                            dep_id, output
                        ));
                    }
                }

                // Each step gets its own session
                let entry_opt = self.registry.get(agent_id);
                let agent_name = entry_opt
                    .as_ref()
                    .map(|e| e.name.clone())
                    .unwrap_or_else(|| agent_id.to_string());
                let step_session = self
                    .memory
                    .create_session_async(agent_name.clone())
                    .await
                    .map_err(KernelError::Carrier)?;

                // Per-step flow injection. Without this, plan steps run bare —
                // no flow body, full agent toolset — and ignore the flow's hard
                // rules. That was the root cause of ai-writer looping on
                // file_read and reaching for document_generate instead of using
                // file_write: the step agent never saw article-writer's rules.
                // When a step names a flow, load it exactly like an explicit
                // active_flow turn: inject the flow body ahead of the task,
                // scope tools to the flow's declared set, apply max_iterations
                // and (for shell_allow flows) elevation.
                let mut tools_owned = tools.to_vec();
                let mut manifest_clone = manifest.clone();
                if let Some(flow_name) = step.flow.as_deref() {
                    if let Some(entry_ref) = entry_opt.as_ref() {
                        // Cached parse: each flow file is read at most once per
                        // plan. The cheap tool-scoping + prompt build still runs
                        // per step (tools_owned is fresh each step).
                        let flow_opt = flow_cache
                            .entry(flow_name.to_string())
                            .or_insert_with(|| self.load_flow_match(entry_ref, flow_name))
                            .as_ref();
                        match flow_opt {
                            Some(flow) => {
                                let (flow_prompt, flow_max_iter) =
                                    self.apply_flow_to_turn(flow, &mut tools_owned, entry_ref);
                                info!(
                                    agent = %agent_name,
                                    step = %step.id,
                                    flow = %flow_name,
                                    "Plan step loaded flow — body + declared tools injected"
                                );
                                // Flow body first, then the task — same shape as
                                // a normal active_flow turn (build_and_apply_prompt).
                                message = format!("{}\n\n{}", flow_prompt, message);
                                Self::apply_flow_elevation(
                                    &mut tools_owned,
                                    &mut manifest_clone,
                                    flow,
                                    &agent_name,
                                );
                                Self::apply_manifest_overrides(
                                    &mut manifest_clone,
                                    flow_max_iter,
                                    None,
                                    &agent_name,
                                );
                            }
                            None => {
                                warn!(
                                    agent = %agent_name,
                                    step = %step.id,
                                    flow = %flow_name,
                                    "Plan step referenced flow not found — running bare"
                                );
                                message = format!(
                                    "⚠️ 本步声明的 flow `{flow_name}` 未找到，按裸任务执行（无 flow 指引）。\n\n{}",
                                    message
                                );
                            }
                        }
                    }
                }

                // Clone Arc references for the spawned task
                let memory = Arc::clone(&self.memory);
                let kh = kernel_handle.clone();
                let driver_clone = driver.clone();
                let brain_clone = brain.map(Arc::clone);
                let mh_clone: Option<Arc<dyn carrier_runtime::memory_handle::MemoryHandle>> =
                    Some(crate::handle::make_memory_handle(Arc::clone(&memory)));
                let sid = sender_id.clone();
                let oid = owner_id.clone();
                let ct = channel_type.clone();
                let ws = manifest.workspace.clone();
                let step_id = step.id.clone();

                info!(
                    step = %step_id,
                    layer = layer_idx,
                    depends_on = ?step.depends_on,
                    "Starting plan step"
                );

                let sem_clone = self.runtime.llm_concurrency_limit.clone();
                let mcp_arc = Arc::clone(&self.plugins.mcp_connections);

                let handle = tokio::spawn(async move {
                    let mut session = step_session;
                    let result = carrier_runtime::agent_loop::run_agent_loop(
                        &manifest_clone,
                        &message,
                        &mut session,
                        &memory,
                        driver_clone,
                        &tools_owned,
                        kh,
                        None,
                        Some(&*mcp_arc),
                        ws.as_deref(),
                        None, // on_phase
                        None, // hooks: not available in spawned task
                        None, // context_window_tokens
                        None, // process_manager
                        None, // user_content_blocks
                        brain_clone,
                        mh_clone,
                        sid.as_deref(),
                        oid.as_deref(),
                        ct.as_deref(),
                        Some(sem_clone),
                    )
                    .await;
                    (step_id, result, session)
                });
                layer_handles.push(handle);
            }

            // Wait for all steps in this layer to complete
            for handle in layer_handles {
                match handle.await {
                    Ok((step_id, Ok(step_result), session)) => {
                        let _ = self.memory.save_session_async(&session).await;
                        info!(
                            step = %step_id,
                            iterations = step_result.iterations,
                            response_len = step_result.response.len(),
                            "Plan step completed"
                        );
                        step_outputs.insert(step_id, step_result.response);
                        total_usage.input_tokens += step_result.total_usage.input_tokens;
                        total_usage.output_tokens += step_result.total_usage.output_tokens;
                        total_iterations += step_result.iterations;
                    }
                    Ok((step_id, Err(e), _)) => {
                        warn!(step = %step_id, error = %e, "Plan step failed");
                        return Err(KernelError::Carrier(CarrierError::Internal(format!(
                            "Plan step '{}' failed: {}",
                            step_id, e
                        ))));
                    }
                    Err(e) => {
                        return Err(KernelError::Carrier(CarrierError::Internal(format!(
                            "Plan step panicked: {}",
                            e
                        ))));
                    }
                }
            }
        }

        // Final result = last step's output
        let final_output = plan
            .steps
            .last()
            .and_then(|s| step_outputs.get(&s.id))
            .cloned()
            .unwrap_or_default();

        info!(
            plan_title = %plan.title,
            total_iterations,
            steps_completed = step_outputs.len(),
            "Plan execution completed"
        );

        Ok(AgentLoopResult {
            response: final_output,
            total_usage,
            iterations: total_iterations,
            silent: false,
            directives: Default::default(),
            plan: None,
        })
    }
}

/// Partition task plan steps into parallel execution layers using topological ordering.
///
/// Steps in the same layer have no dependencies on each other and can run in parallel.
/// Each layer only contains steps whose dependencies are all in earlier layers.
fn partition_steps_by_layers(
    steps: &[carrier_runtime::agent_loop::TaskStep],
) -> Vec<Vec<&carrier_runtime::agent_loop::TaskStep>> {
    use std::collections::HashMap;

    let step_map: HashMap<&str, &carrier_runtime::agent_loop::TaskStep> =
        steps.iter().map(|s| (s.id.as_str(), s)).collect();

    let mut layer_of: HashMap<String, usize> = HashMap::new();

    // Compute layer for each step: layer = max(dep.layer) + 1, or 0 if no deps
    // Process in topological order (simple iterative approach)
    let mut changed = true;
    while changed {
        changed = false;
        for step in steps {
            let computed_layer = if step.depends_on.is_empty() {
                0
            } else {
                step.depends_on
                    .iter()
                    .filter_map(|dep| layer_of.get(dep))
                    .max()
                    .map(|&l| l + 1)
                    .unwrap_or(0)
            };
            let current = layer_of.entry(step.id.clone()).or_insert(0);
            if computed_layer > *current {
                *current = computed_layer;
                changed = true;
            }
        }
    }

    // Assign layer 0 to any step not yet assigned (shouldn't happen but safety)
    for step in steps {
        layer_of.entry(step.id.clone()).or_insert(0);
    }

    // Group by layer
    let max_layer = layer_of.values().copied().max().unwrap_or(0);
    let mut layers: Vec<Vec<&carrier_runtime::agent_loop::TaskStep>> =
        vec![Vec::new(); max_layer + 1];
    for step in steps {
        if let Some(&layer) = layer_of.get(&step.id) {
            layers[layer].push(step_map[step.id.as_str()]);
        }
    }

    layers
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrier_types::agent::{
        AgentEntry, AgentId, AgentManifest, AgentMode, AgentState, ManifestCapabilities,
        ModelConfig, ResourceQuota, ScheduleMode, SessionId,
    };
    use chrono::Utc;
    use std::collections::HashMap;

    fn entry_with_workspace(ws: &std::path::Path) -> AgentEntry {
        AgentEntry {
            id: AgentId::new(),
            name: "test-agent".to_string(),
            manifest: AgentManifest {
                name: "test-agent".to_string(),
                display_name: String::new(),
                version: "0.1.0".to_string(),
                description: "test".to_string(),
                author: "test".to_string(),
                module: "test".to_string(),
                schedule: ScheduleMode::default(),
                model: ModelConfig::default(),
                resources: ResourceQuota::default(),
                priority: Priority::default(),
                capabilities: ManifestCapabilities::default(),
                profile: None,
                tools: HashMap::new(),
                flows: vec![],
                mcp_servers: vec![],
                max_tool_level: carrier_types::tool::PermissionLevel::Write,
                intent_classifier_enabled: None,
                default_flow: None,
                metadata: HashMap::new(),
                tags: vec![],
                autonomous: None,
                workspace: Some(ws.to_path_buf()),
                generate_identity_files: true,
                exec_policy: None,
                cli_exec: None,
                tool_allowlist: vec![],
                tool_blocklist: vec![],
                clone_source: None,
                knowledge_files: vec![],
                plugins: vec![],
                subagents: vec![],
            },
            state: AgentState::Created,
            mode: AgentMode::default(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            parent: None,
            children: vec![],
            session_id: SessionId::new(),
            tags: vec![],
            identity: Default::default(),
            onboarding_completed: false,
            onboarding_completed_at: None,
        }
    }

    /// Rollover sizing: empty session is 0; messages count toward the
    /// threshold; a realistic bloated chain session (the jiakao failure mode)
    /// crosses SESSION_ROLLOVER_CHARS.
    #[test]
    fn session_chars_measures_rollover_pressure() {
        use carrier_memory::session::Session;
        use carrier_types::message::{Message, MessageContent, Role};

        let mut s = Session {
            id: SessionId::new(),
            agent_name: "wechat-writer".into(),
            messages: vec![],
            turn_summaries: vec![],
            context_window_tokens: 0,
            label: None,
        };
        assert_eq!(session_chars(&s), 0, "empty session measures zero");

        // A realistic long chain turn (user msg + assistant reply + tool
        // exchange) — a few KB each. 50 of them is a heavy but healthy chain;
        // it must stay under the rollover threshold.
        s.messages.push(Message {
            role: Role::User,
            content: MessageContent::Text("x".repeat(2_000)),
        });
        s.messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::Text("y".repeat(2_000)),
        });
        let per_turn = session_chars(&s);
        assert!(per_turn > 4_000, "serde wrapper adds overhead: {per_turn}");
        assert!(
            per_turn * 25 < SESSION_ROLLOVER_CHARS,
            "25 chain turns (~50 messages) must stay under rollover: {} vs {SESSION_ROLLOVER_CHARS}",
            per_turn * 25
        );
        // The observed degeneration case: ~300K chars of history.
        assert!(
            per_turn * 150 > SESSION_ROLLOVER_CHARS,
            "150 chain turns (~300K chars, the jiakao case) must exceed rollover"
        );
    }

    #[test]
    fn session_label_override_beats_sender_and_is_trimmed() {
        // Chained pipelines: an explicit session_label wins over the sender's
        // user:<openid> — pipeline steps run in their own session.
        let sid = Some("oABC@im.wechat".to_string());
        assert_eq!(
            CarrierKernel::resolve_session_label(
                AgentId::new(),
                Some(" pipeline:20260815-glm53 "),
                &sid,
                Some("task-9"),
                &None,
                &None,
            )
            .unwrap(),
            "pipeline:20260815-glm53"
        );
        // Blank override is ignored — falls through to the sender label.
        assert_eq!(
            CarrierKernel::resolve_session_label(
                AgentId::new(),
                Some("   "),
                &sid,
                None,
                &None,
                &None,
            )
            .unwrap(),
            "user:oABC@im.wechat"
        );
    }

    /// Every flow that reaches apply_flow_elevation
    /// (deny + sandbox + elevation + shell_allow + report gate): the caged
    /// default_flow fallback mode was removed 2026-08-18 — a classifier miss
    /// now runs a bare turn instead of loading a guessed half-empowered flow.
    /// Regression guard: 86bus chat misses once loaded article-formatter and
    /// were elevated Write→Dangerous by accident (that is now prevented one
    /// level up, by not loading the flow at all).
    /// Boot a kernel on a fresh tempdir (offline brain pointing at a refused port).
    /// Same pattern as `kv_memory_recall_uses_sender_partition`.
    fn boot_test_kernel() -> (tempfile::TempDir, CarrierKernel) {
        let tmp = tempfile::tempdir().unwrap();
        let brain = serde_json::json!({
            "base_url": "http://127.0.0.1:1/v1/chat/completions",
            "api_key_env": "",
            "default_modality": "chat",
            "modalities": { "chat": { "description": "test" } }
        });
        std::fs::write(tmp.path().join("brain.json"), brain.to_string()).unwrap();
        let config = carrier_types::config::KernelConfig {
            home_dir: tmp.path().to_path_buf(),
            data_dir: tmp.path().join("data"),
            ..carrier_types::config::KernelConfig::default()
        };
        let kernel = CarrierKernel::boot_with_config(config).expect("kernel should boot");
        (tmp, kernel)
    }

    /// Driver that records the model-visible surface of every LLM request it
    /// serves (advertised tool names, system prompt, message roles) and then
    /// answers with a plain EndTurn. The deepseek-harness `model-visible.json`
    /// technique: pin what the model actually sees, not what assembly intended.
    struct RecordingDriver {
        requests: std::sync::Mutex<Vec<RecordedRequest>>,
    }

    #[derive(Default)]
    struct RecordedRequest {
        tool_names: Vec<String>,
        system: String,
        message_roles: Vec<&'static str>,
    }

    #[async_trait::async_trait]
    impl carrier_runtime::llm_driver::LlmDriver for RecordingDriver {
        async fn complete(
            &self,
            request: carrier_runtime::llm_driver::CompletionRequest,
        ) -> Result<
            carrier_runtime::llm_driver::CompletionResponse,
            carrier_runtime::llm_driver::LlmError,
        > {
            self.requests.lock().unwrap().push(RecordedRequest {
                tool_names: request.tools.iter().map(|t| t.name.clone()).collect(),
                system: request.system.clone().unwrap_or_default(),
                message_roles: request
                    .messages
                    .iter()
                    .map(|m| match m.role {
                        carrier_types::message::Role::System => "system",
                        carrier_types::message::Role::User => "user",
                        carrier_types::message::Role::Assistant => "assistant",
                    })
                    .collect(),
            });
            Ok(carrier_runtime::llm_driver::CompletionResponse {
                content: vec![carrier_types::message::ContentBlock::Text {
                    text: "done".to_string(),
                    provider_metadata: None,
                }],
                stop_reason: carrier_types::message::StopReason::EndTurn,
                tool_calls: vec![],
                usage: Default::default(),
                media: None,
            })
        }
    }

    impl RecordingDriver {
        fn new() -> Self {
            Self {
                requests: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    /// Tool names contributed by machine config — api_tools.toml on the global
    /// home plus process-global dynamic registrations (the two extra sources
    /// `resolve_tools` reads beyond CORE_TOOL_NAMES). Goldens pin the
    /// code-defined surface only, so these are subtracted before comparing.
    fn machine_config_tool_names(entry: &AgentEntry) -> std::collections::HashSet<String> {
        let home = carrier_types::config::home_dir();
        carrier_runtime::api_tools::loader::load_all_api_tools(
            &home,
            entry.manifest.workspace.as_deref(),
        )
        .into_iter()
        .map(|t| t.name)
        .chain(
            carrier_runtime::api_tools::register::dynamic_tools()
                .into_iter()
                .map(|t| t.name),
        )
        .collect()
    }

    /// Core tool surface golden — the bootstrap set every agent's turn starts
    /// from (`resolve_tools`: CORE_TOOL_NAMES ∩ builtin catalog). An independent
    /// pinned snapshot: editing CORE_TOOL_NAMES OR dropping/renaming a builtin
    /// definition must show up here in review, never only in production.
    /// Machine-config extras (api_tools.toml from the global home) are filtered
    /// out — they are config, not code, and vary per host.
    #[test]
    fn core_tool_surface_is_pinned() {
        let (_tmp, kernel) = boot_test_kernel();
        let entry = entry_with_workspace(std::path::Path::new("/tmp/nonexistent-ws"));
        let machine = machine_config_tool_names(&entry);
        let names: Vec<String> = {
            let mut v: Vec<String> = kernel
                .resolve_tools(&entry)
                .into_iter()
                .filter(|t| !machine.contains(&t.name))
                .map(|t| t.name)
                .collect();
            v.sort();
            v
        };
        let golden: Vec<&str> = vec![
            "api_tool_register",
            "cron_cancel",
            "cron_create",
            "cron_list",
            "document_generate",
            "file_list",
            "file_read",
            "flow_create",
            "flow_load",
            "flow_update",
            "image_generate",
            "knowledge_add",
            "knowledge_list",
            "knowledge_read",
            "knowledge_update",
            "kv_get",
            "kv_list",
            "kv_set",
            "session_summarize",
            "task_plan",
            "user_profile",
            "web_fetch",
            "web_search",
        ];
        assert_eq!(
        names,
        golden.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "core model-visible tool surface drifted — if intentional, update the golden deliberately"
    );
    }

    /// Flow turn assembly golden: a real flow.md parsed from disk through
    /// tool injection, deny_tools, elevation, and prompt assembly, then one
    /// turn through the real agent loop. Pins three invariants the historical
    /// "tool silently vanished from the model's view" bugs broke:
    ///   1. advertised tools = core ∪ flow tools − deny_tools (exactly)
    ///   2. advertised tools = META_FLOW_ALLOWED_TOOLS (dispatch sandbox matches
    ///      what the model was promised — the cage-era mismatch)
    ///   3. no "Flow Tool Warnings" (declared-but-unresolvable tools vanish
    ///      silently — the clone-generate description-empty bug class)
    #[tokio::test(flavor = "multi_thread")]
    async fn model_visible_surface_flow_assembly_golden() {
        let (tmp, kernel) = boot_test_kernel();
        let ws = tmp.path().join("workspaces").join("golden-agent");
        std::fs::create_dir_all(ws.join("flows").join("fixture-writer")).unwrap();
        std::fs::write(
            ws.join("flows").join("fixture-writer").join("flow.md"),
            "---\n\
         name: fixture-writer\n\
         description: golden fixture flow for the model-visible surface test\n\
         version: 1\n\
         tools:\n  - file_read\n  - file_write\n  - shell_exec\n\
         deny_tools:\n  - task_plan\n\
         shell_allow:\n  - python3 flows/fixture-writer/scripts/*\n\
         ---\n\nWrite the report.\n",
        )
        .unwrap();

        let entry = entry_with_workspace(&ws);
        let mut tools = kernel.resolve_tools(&entry);
        let mut manifest = entry.manifest.clone();

        // The explicit active_flow path, exactly as prepare_agent_context runs it.
        let flow = kernel
            .load_flow_match(&entry, "fixture-writer")
            .expect("fixture flow parses from disk");
        let (flow_prompt, _max_iter) = kernel.apply_flow_to_turn(&flow, &mut tools, &entry);
        assert!(
            !flow_prompt.contains("Flow Tool Warnings"),
            "flow tool injection failed — declared tools vanished: {flow_prompt}"
        );
        CarrierKernel::apply_flow_elevation(&mut tools, &mut manifest, &flow, &entry.name);
        kernel.build_and_apply_prompt(
            &mut manifest,
            &tools,
            &Some("user:golden".to_string()),
            Some("Golden User".to_string()),
            &None,
            Some(flow_prompt),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            None,
            None,
        );

        // Loop-observed surface.
        let mut session = carrier_memory::session::Session {
            id: SessionId::new(),
            agent_name: manifest.name.clone(),
            messages: Vec::new(),
            context_window_tokens: 0,
            turn_summaries: Vec::new(),
            label: None,
        };
        let recorder = std::sync::Arc::new(RecordingDriver::new());
        let peek: std::sync::Arc<RecordingDriver> = recorder.clone();
        let driver: std::sync::Arc<dyn carrier_runtime::llm_driver::LlmDriver> = recorder;
        let _result = carrier_runtime::agent_loop::run_agent_loop(
            &manifest,
            "write it",
            &mut session,
            &kernel.memory,
            driver,
            &tools,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("loop runs");

        let recorded = peek.requests.lock().unwrap();
        assert_eq!(recorded.len(), 1, "one EndTurn iteration = one request");
        let req = &recorded[0];

        // Filter machine-config extras (api_tools.toml on the global home):
        // golden pins the code-defined surface only.
        let machine = machine_config_tool_names(&entry);
        let mut advertised: Vec<String> = req
            .tool_names
            .iter()
            .filter(|n| !machine.contains(*n))
            .cloned()
            .collect();
        advertised.sort();
        let mut expected: Vec<String> = vec![
            "api_tool_register",
            "cron_cancel",
            "cron_create",
            "cron_list",
            "document_generate",
            "file_list",
            "file_read",
            "file_write",
            "flow_create",
            "flow_load",
            "flow_update",
            "image_generate",
            "knowledge_add",
            "knowledge_list",
            "knowledge_read",
            "knowledge_update",
            "kv_get",
            "kv_list",
            "kv_set",
            "session_summarize",
            "shell_exec",
            "user_profile",
            "web_fetch",
            "web_search",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        expected.sort();
        assert_eq!(
            advertised, expected,
            "flow-turn advertised tool surface drifted (core ∪ flow.tools − deny_tools)"
        );
        assert!(
            !req.tool_names.contains(&"task_plan".to_string()),
            "deny_tools must remove task_plan from the model's view"
        );

        // Advertised ⟺ enforced: the sandbox allow-list the dispatcher will check
        // must be exactly the set the model was shown (full sets, machine-config
        // extras cancel out on both sides).
        let advertised_full: Vec<String> = {
            let mut v = req.tool_names.clone();
            v.sort();
            v
        };
        let allowed: Vec<String> = {
            let mut v: Vec<String> = manifest
                .metadata
                .get(carrier_types::flow::META_FLOW_ALLOWED_TOOLS)
                .and_then(|v| v.as_array())
                .expect("sandbox stamp present")
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            allowed, advertised_full,
            "advertised tools must equal the dispatch sandbox allow-list"
        );

        assert!(
            req.system.contains("## Active Flow (auto-matched)"),
            "flow prompt must reach the system prompt"
        );
        assert!(
            req.system.contains("**fixture-writer**"),
            "flow header must be visible"
        );
        // First-request message shape: the user turn + the loop's injected
        // turn-status system line (build_status_message "📊 Turn…") — pinning the
        // position keeps future injections (which shift what the model sees) visible.
        assert_eq!(
            req.message_roles,
            vec!["user", "system"],
            "first request sees the user turn plus the loop status system message"
        );
    }

    #[test]
    fn matched_flow_gets_full_authority() {
        let flow = crate::prompt_sources::FlowMatch {
            name: "article-formatter".to_string(),
            body: "…".to_string(),
            max_iterations: None,
            tools: vec!["file_read".to_string(), "shell_exec".to_string()],
            flow_def: carrier_types::flow::FlowDef {
                name: "article-formatter".to_string(),
                description: "formats".to_string(),
                max_iterations: None,
                tools: vec!["file_read".to_string(), "shell_exec".to_string()],
                body: String::new(),
                steps: vec![],
                final_step: None,
                entry: None,
                output: Some("report".to_string()),
                privilege: Default::default(),
                shell_allow: vec!["python3 flows/article-formatter/scripts/*".to_string()],
                shell_allow_checks: vec![],
                deny_tools: vec!["task_plan".to_string()],
            },
        };
        assert!(flow.elevates(), "fixture must be an elevating flow");

        let tool = |name: &str| carrier_types::tool::ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            input_schema: serde_json::json!({}),
        };

        let mut tools = vec![tool("task_plan"), tool("file_read"), tool("shell_exec")];
        let mut manifest = entry_with_workspace(std::path::Path::new("/tmp")).manifest;
        CarrierKernel::apply_flow_elevation(&mut tools, &mut manifest, &flow, "a");
        assert!(
            !tools.iter().any(|t| t.name == "task_plan"),
            "deny_tools applies"
        );
        assert!(
            manifest
                .metadata
                .contains_key(carrier_types::flow::META_FLOW_ALLOWED_TOOLS),
            "hard sandbox applies"
        );
        assert!(
            manifest.max_tool_level > carrier_types::tool::PermissionLevel::Write,
            "elevates"
        );
        assert!(
            manifest
                .metadata
                .contains_key(carrier_types::flow::META_OUTPUT_REPORT),
            "report gate stamps"
        );
        assert!(
            manifest
                .metadata
                .contains_key(carrier_types::flow::META_FLOW_SHELL_ALLOW),
            "shell_allow stamps"
        );
        assert!(
            manifest
                .metadata
                .contains_key(carrier_types::flow::META_FLOW_ELEVATED_TOOLS),
            "elevated tools stamp"
        );
    }

    #[test]
    fn test_read_template_default_flow() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("template.json"),
            r#"{"default_flow": "consultation"}"#,
        )
        .unwrap();
        let entry = entry_with_workspace(dir.path());
        assert_eq!(
            CarrierKernel::read_template_default_flow(&entry).as_deref(),
            Some("consultation")
        );

        // Missing template.json → None.
        let empty = tempfile::tempdir().unwrap();
        let e2 = entry_with_workspace(empty.path());
        assert_eq!(CarrierKernel::read_template_default_flow(&e2), None);

        // Drifted template shape (mcp_servers object array) still reads the
        // single field — a full-struct parse would fail here.
        std::fs::write(
            dir.path().join("template.json"),
            r#"{"mcp_servers":[{"name":"srv","required":true}],"default_flow":"consultation"}"#,
        )
        .unwrap();
        let e3 = entry_with_workspace(dir.path());
        assert_eq!(
            CarrierKernel::read_template_default_flow(&e3).as_deref(),
            Some("consultation")
        );
    }

    /// 把 kernel 的 brain 换成注入 driver（boot_test_kernel 的 brain 指向
    /// 不可达 URL）。
    fn install_test_driver<D>(kernel: &CarrierKernel, driver: D)
    where
        D: carrier_runtime::llm_driver::LlmDriver + 'static,
    {
        use carrier_types::brain::{BrainConfig, ModalityEntry};

        let brain_config = BrainConfig {
            base_url: "http://127.0.0.1:1/v1/chat/completions".to_string(),
            api_key_env: String::new(),
            default_modality: "chat".to_string(),
            modalities: HashMap::from([(
                "chat".to_string(),
                ModalityEntry {
                    description: "test".to_string(),
                },
            )]),
        };
        *kernel.brain.brain.write().unwrap() = Arc::new(crate::brain::Brain::with_test_driver(
            brain_config,
            Arc::new(driver),
        ));
    }

    /// 借用轮（第三刀 3.1）：票据进/出 + 上下文连续性 + 无状态断言。
    ///
    /// EchoDriver 回显它看到的全部 user 消息——第二轮能看见第一轮的内容
    /// = 票据承载了会话连续性；真实 substrate 无 borrow label session 行
    /// = 主人服务器零持久化。
    #[tokio::test(flavor = "multi_thread")]
    async fn borrowed_turn_carries_ticket_context_and_leaves_no_server_state() {
        use carrier_memory::session::SessionTicket;
        use carrier_runtime::llm_driver::{
            CompletionRequest, CompletionResponse, LlmDriver, LlmError,
        };
        use carrier_types::message::{ContentBlock, Role, StopReason, TokenUsage};

        struct EchoDriver(Arc<std::sync::Mutex<Vec<String>>>);
        #[async_trait::async_trait]
        impl LlmDriver for EchoDriver {
            async fn complete(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                // 旁路捕获 system prompt——末尾断言"借用轮须知"在场。
                // 旁路捕获 system prompt——末尾断言"借用轮须知"在场。
                self.0
                    .lock()
                    .unwrap()
                    .push(request.system.clone().unwrap_or_default());
                let users: Vec<String> = request
                    .messages
                    .iter()
                    .filter(|m| m.role == Role::User)
                    .map(|m| m.content.text_content())
                    .collect();
                Ok(CompletionResponse {
                    content: vec![ContentBlock::Text {
                        text: format!("ECHO[{}] {}", users.len(), users.join("|")),
                        provider_metadata: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                    media: None,
                })
            }
        }

        let (_tmp, kernel) = boot_test_kernel();
        // boot_test_kernel 的 brain 指向不可达 URL——换成 EchoDriver。
        let systems: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        install_test_driver(&kernel, EchoDriver(Arc::clone(&systems)));

        // run_borrowed_turn 经 registry 解析 agent——直接注册测试 entry。
        let entry = entry_with_workspace(std::path::Path::new("/tmp/nonexistent-ws"));
        let agent_id = entry.id;
        kernel
            .registry
            .register(entry)
            .expect("agent should register");

        // 第一轮：空票据进。
        let out1 = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "ALPHA-MARKER",
                None,
                None,
                &[],
                None,
                None,
            )
            .await
            .expect("first borrowed turn should succeed");
        assert!(!out1.response.is_empty());
        assert!(
            out1.response.contains("ALPHA-MARKER"),
            "echo saw the user message: {}",
            out1.response
        );
        // 票据长出了本轮 user+assistant 对；label 兜底 borrow:<agent>。
        assert!(out1.ticket.messages.len() >= 2);
        assert_eq!(out1.ticket.label.as_deref(), Some("borrow:test-agent"));

        // 第二轮：回喂第一轮票据——echo 必须同时看见 ALPHA 与 BETA。
        let out2 = kernel
            .run_borrowed_turn(
                agent_id,
                out1.ticket,
                "BETA-MARKER",
                None,
                None,
                &[],
                None,
                None,
            )
            .await
            .expect("second borrowed turn should succeed");
        assert!(
            out2.response.contains("ALPHA-MARKER") && out2.response.contains("BETA-MARKER"),
            "ticket carried turn-1 context into turn 2: {}",
            out2.response
        );

        // 无状态断言：真实 substrate 无 borrow label 的 session 行。
        let rows = kernel.memory.list_sessions().expect("list_sessions");
        let borrow_rows = rows
            .iter()
            .filter(|r| {
                r.get("label")
                    .and_then(|l| l.as_str())
                    .map(|l| l.starts_with("borrow:"))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(
            borrow_rows, 0,
            "borrowed turns must not persist sessions on the host"
        );

        // 失忆兜底断言：每轮 agent system prompt 都带"借用轮须知"——分身查 kv
        // 置空会自称失忆，指引把模型拉回 transcript（历史在对话消息里）。
        // 旁路里混有 summarizer 等内部调用（system 无此段），只断言 agent 轮
        // （prompt 以 agent 身份开场 "You are test-agent"）。
        let agent_sys: Vec<String> = {
            let systems = systems.lock().unwrap();
            systems
                .iter()
                .filter(|s| s.contains("You are test-agent"))
                .cloned()
                .collect()
        };
        assert!(
            agent_sys.len() >= 2,
            "one agent-turn prompt per borrowed turn, got {}",
            agent_sys.len()
        );
        for (i, sys) in agent_sys.iter().enumerate() {
            assert!(
                sys.contains("借用轮须知") && sys.contains("不要因 kv 为空"),
                "agent turn {i} system prompt misses the borrow-turn amnesia note"
            );
        }
    }

    /// 借用轮素材（第三刀 3.2）：轮中可读（driver 轮内实读素材文件）、prompt
    /// 注入素材路径提示、轮末销毁、开轮清扫残留、恶意名/超限/无 workspace 拒绝。
    #[tokio::test(flavor = "multi_thread")]
    async fn borrowed_turn_materials_lifecycle() {
        use carrier_memory::session::SessionTicket;
        use carrier_runtime::llm_driver::{
            CompletionRequest, CompletionResponse, LlmDriver, LlmError,
        };
        use carrier_types::message::{ContentBlock, StopReason, TokenUsage};

        /// 轮中探针：complete() 被调时素材必须已落盘——实读 borrow/*/materials/
        /// secret.txt 回显内容，并回显 system prompt 是否含素材提示段。
        struct MaterialsProbeDriver {
            ws: std::path::PathBuf,
        }
        #[async_trait::async_trait]
        impl LlmDriver for MaterialsProbeDriver {
            async fn complete(
                &self,
                request: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                let mut material = "<missing>".to_string();
                if let Ok(rd) = std::fs::read_dir(self.ws.join("borrow")) {
                    for e in rd.flatten() {
                        if let Ok(c) =
                            std::fs::read_to_string(e.path().join("materials/secret.txt"))
                        {
                            material = c;
                            break;
                        }
                    }
                }
                let sys = request.system.unwrap_or_default();
                let hint = sys.contains("【借用轮素材】") && sys.contains("/materials/");
                Ok(CompletionResponse {
                    content: vec![ContentBlock::Text {
                        text: format!("MATERIAL={material} HINT={hint}"),
                        provider_metadata: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                    media: None,
                })
            }
        }

        let (_tmp, kernel) = boot_test_kernel();
        let ws = tempfile::tempdir().unwrap();
        install_test_driver(
            &kernel,
            MaterialsProbeDriver {
                ws: ws.path().to_path_buf(),
            },
        );

        // 植入上一个"死轮"残留——开轮清扫必须吃掉它。
        std::fs::create_dir_all(ws.path().join("borrow/stale-turn/materials")).unwrap();
        std::fs::write(ws.path().join("borrow/stale-turn/materials/junk"), b"junk").unwrap();

        let mut entry = entry_with_workspace(ws.path());
        entry.name = "materials-test-agent".to_string();
        entry.manifest.name = entry.name.clone();
        let agent_id = entry.id;
        kernel
            .registry
            .register(entry)
            .expect("agent should register");

        // 正常轮：素材轮中可读 + prompt 提示注入 + 轮末销毁 + 残留清扫。
        let materials = vec![super::BorrowedMaterial {
            name: "secret.txt".to_string(),
            content: b"secret 42".to_vec(),
        }];
        let out = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "读素材",
                None,
                None,
                &materials,
                None,
                None,
            )
            .await
            .expect("materials turn should succeed");
        assert!(
            out.response.contains("MATERIAL=secret 42"),
            "material must be readable mid-turn: {}",
            out.response
        );
        assert!(
            out.response.contains("HINT=true"),
            "system prompt must carry the materials hint: {}",
            out.response
        );
        let leftover: Vec<_> = std::fs::read_dir(ws.path().join("borrow"))
            .map(|rd| rd.flatten().collect())
            .unwrap_or_default();
        assert!(leftover.is_empty(), "borrow/ must be empty after the turn");

        // 恶意名拒绝：`..` 逃逸。
        let evil = vec![super::BorrowedMaterial {
            name: "../escape.txt".to_string(),
            content: b"x".to_vec(),
        }];
        let err = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "m",
                None,
                None,
                &evil,
                None,
                None,
            )
            .await
            .expect_err("traversal name must be rejected");
        assert!(err.to_string().contains("bad borrowed material name"));

        // 超限拒绝：单文件 > 8MiB。
        let big = vec![super::BorrowedMaterial {
            name: "big.bin".to_string(),
            content: vec![0u8; super::BORROWED_MATERIAL_MAX_FILE + 1],
        }];
        let err = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "m",
                None,
                None,
                &big,
                None,
                None,
            )
            .await
            .expect_err("oversize material must be rejected");
        assert!(err.to_string().contains("exceeds"));

        // 无 workspace 的 agent 带素材 → 拒绝。
        let mut bare = entry_with_workspace(ws.path());
        bare.id = carrier_types::agent::AgentId::new();
        bare.name = "bare-test-agent".to_string();
        bare.manifest.name = bare.name.clone();
        bare.manifest.workspace = None;
        let bare_id = bare.id;
        kernel.registry.register(bare).expect("bare agent register");
        let err = kernel
            .run_borrowed_turn(
                bare_id,
                SessionTicket::empty(None),
                "m",
                None,
                None,
                &materials,
                None,
                None,
            )
            .await
            .expect_err("materials without workspace must be rejected");
        assert!(err.to_string().contains("require an agent workspace"));
    }

    /// 借用轮产物回传（第三刀 3.3）：driver 轮中往 borrow/<uuid>/output/ 写
    /// 文件（等价 agent 的 file_write），轮末文件随 outcome.files 回流、主机
    /// 目录销毁；超预算大文件跳过不阻断。
    #[tokio::test(flavor = "multi_thread")]
    async fn borrowed_turn_output_files_returned_and_host_cleaned() {
        use carrier_memory::session::SessionTicket;
        use carrier_runtime::llm_driver::{
            CompletionRequest, CompletionResponse, LlmDriver, LlmError,
        };
        use carrier_types::message::{ContentBlock, StopReason, TokenUsage};

        /// 轮中写产物：一个正常文件 + 一个超过单文件上限的文件。
        /// `writes: Arc<AtomicU32>`——只在第一次 complete 写（第二轮断言空产物）。
        struct OutputWriterDriver {
            ws: std::path::PathBuf,
            writes: Arc<std::sync::atomic::AtomicU32>,
        }
        #[async_trait::async_trait]
        impl LlmDriver for OutputWriterDriver {
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                use std::sync::atomic::Ordering;
                if self.writes.fetch_add(1, Ordering::SeqCst) > 0 {
                    return Ok(CompletionResponse {
                        content: vec![ContentBlock::Text {
                            text: "done-again".to_string(),
                            provider_metadata: None,
                        }],
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        usage: TokenUsage {
                            input_tokens: 1,
                            output_tokens: 1,
                        },
                        media: None,
                    });
                }
                let mut out_dir = None;
                if let Ok(rd) = std::fs::read_dir(self.ws.join("borrow")) {
                    if let Some(e) = rd.flatten().next() {
                        let d = e.path().join("output");
                        std::fs::create_dir_all(&d).unwrap();
                        out_dir = Some(d);
                    }
                }
                let d = out_dir.expect("turn dir must exist mid-turn");
                std::fs::write(d.join("report.md"), "# 报告\n正文").unwrap();
                std::fs::create_dir_all(d.join("sub")).unwrap();
                std::fs::write(d.join("sub/nested.txt"), "nested").unwrap();
                std::fs::write(
                    d.join("huge.bin"),
                    vec![0u8; super::BORROWED_OUTPUT_MAX_FILE + 1],
                )
                .unwrap();
                Ok(CompletionResponse {
                    content: vec![ContentBlock::Text {
                        text: "done".to_string(),
                        provider_metadata: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                    media: None,
                })
            }
        }

        let (_tmp, kernel) = boot_test_kernel();
        let ws = tempfile::tempdir().unwrap();
        install_test_driver(
            &kernel,
            OutputWriterDriver {
                ws: ws.path().to_path_buf(),
                writes: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            },
        );

        let mut entry = entry_with_workspace(ws.path());
        entry.name = "output-test-agent".to_string();
        entry.manifest.name = entry.name.clone();
        let agent_id = entry.id;
        kernel.registry.register(entry).expect("register");

        let out = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "生成报告",
                None,
                None,
                &[],
                None,
                None,
            )
            .await
            .expect("output turn should succeed");

        // 回流：两个预算内文件（含子目录相对路径），超大文件跳过。
        let names: Vec<&str> = out.files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"report.md"), "files: {names:?}");
        assert!(names.contains(&"sub/nested.txt"), "files: {names:?}");
        assert_eq!(out.files.len(), 2);
        let report = out.files.iter().find(|f| f.name == "report.md").unwrap();
        use base64::Engine as _;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&report.content_base64)
                .unwrap(),
            "# \u{62a5}\u{544a}\n\u{6b63}\u{6587}".as_bytes()
        );

        // 主机销毁：borrow/ 清空。
        let leftover: Vec<_> = std::fs::read_dir(ws.path().join("borrow"))
            .map(|rd| rd.flatten().collect())
            .unwrap_or_default();
        assert!(leftover.is_empty(), "borrow/ must be empty after turn");

        // 无素材轮也建 output/（提示段始终注入）——空产物时 files 为空即可。
        let out2 = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "再来一轮",
                None,
                None,
                &[],
                None,
                None,
            )
            .await
            .expect("second output turn should succeed");
        assert!(out2.files.is_empty());
    }

    /// 借用准入与配额：allowlist 拒绝名单外身份；每小时轮次上限触发；
    /// 本地直连（borrower=None）免名单门。
    #[tokio::test(flavor = "multi_thread")]
    async fn borrowed_turn_admission_and_rate_limit() {
        use crate::kernel::CarrierKernel;
        use carrier_memory::session::SessionTicket;
        use carrier_types::config::{BorrowPolicyConfig, KernelConfig};

        use carrier_runtime::llm_driver::{
            CompletionRequest, CompletionResponse, LlmDriver, LlmError,
        };
        use carrier_types::message::{ContentBlock, StopReason, TokenUsage};

        struct Echo;
        #[async_trait::async_trait]
        impl LlmDriver for Echo {
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                Ok(CompletionResponse {
                    content: vec![ContentBlock::Text {
                        text: "ok".to_string(),
                        provider_metadata: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: TokenUsage {
                        input_tokens: 1,
                        output_tokens: 1,
                    },
                    media: None,
                })
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        let brain = serde_json::json!({
            "base_url": "http://127.0.0.1:1/v1/chat/completions",
            "api_key_env": "",
            "default_modality": "chat",
            "modalities": { "chat": { "description": "test" } }
        });
        std::fs::write(tmp.path().join("brain.json"), brain.to_string()).unwrap();
        let config = KernelConfig {
            home_dir: tmp.path().to_path_buf(),
            data_dir: tmp.path().join("data"),
            borrow: BorrowPolicyConfig {
                enabled: true,
                allow_borrowers: vec!["friend-a".to_string()],
                max_turns_per_hour: 2,
            },
            ..KernelConfig::default()
        };
        let kernel = CarrierKernel::boot_with_config(config).expect("kernel boot");
        install_test_driver(&kernel, Echo);

        let entry = entry_with_workspace(std::path::Path::new("/tmp/nonexistent-ws"));
        let agent_id = entry.id;
        kernel.registry.register(entry).expect("register");

        // 名单外身份 → 拒绝，且不产生台账计数。
        let err = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "hi",
                None,
                None,
                &[],
                None,
                Some("stranger"),
            )
            .await
            .expect_err("outsider must be rejected");
        assert!(err.to_string().contains("not allowed to borrow"));

        // 名单内身份：第 1、2 轮通过（上限 2），第 3 轮触发限流。
        kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "1",
                None,
                None,
                &[],
                None,
                Some("friend-a"),
            )
            .await
            .expect("turn 1 within limit");
        kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "2",
                None,
                None,
                &[],
                None,
                Some("friend-a"),
            )
            .await
            .expect("turn 2 at limit");
        let err = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "3",
                None,
                None,
                &[],
                None,
                Some("friend-a"),
            )
            .await
            .expect_err("turn 3 must hit rate limit");
        assert!(err.to_string().contains("rate limit exceeded"));

        // 本地直连（borrower=None）不受名单门限制——但配额按 "local" 独立计。
        let out = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "local",
                None,
                None,
                &[],
                None,
                None,
            )
            .await
            .expect("local direct turn must bypass allowlist");
        assert!(!out.response.is_empty());

        // 台账存在且只含元数据（无消息内容）。
        let ledger = std::fs::read_to_string(tmp.path().join("data/borrow_usage.jsonl")).unwrap();
        assert!(ledger.contains("\"borrower\":\"friend-a\""));
        assert!(!ledger.contains("ALPHA") && !ledger.contains("生成报告"));
    }

    /// enabled=false 总开关：一切借用轮直接拒绝。
    #[tokio::test(flavor = "multi_thread")]
    async fn borrowed_turn_disabled_rejects_all() {
        use crate::kernel::CarrierKernel;
        use carrier_memory::session::SessionTicket;
        use carrier_types::config::{BorrowPolicyConfig, KernelConfig};

        let tmp = tempfile::tempdir().unwrap();
        let brain = serde_json::json!({
            "base_url": "http://127.0.0.1:1/v1/chat/completions",
            "api_key_env": "",
            "default_modality": "chat",
            "modalities": { "chat": { "description": "test" } }
        });
        std::fs::write(tmp.path().join("brain.json"), brain.to_string()).unwrap();
        let config = KernelConfig {
            home_dir: tmp.path().to_path_buf(),
            data_dir: tmp.path().join("data"),
            borrow: BorrowPolicyConfig {
                enabled: false,
                ..BorrowPolicyConfig::default()
            },
            ..KernelConfig::default()
        };
        let kernel = CarrierKernel::boot_with_config(config).expect("kernel boot");

        let entry = entry_with_workspace(std::path::Path::new("/tmp/nonexistent-ws"));
        let agent_id = entry.id;
        kernel.registry.register(entry).expect("register");

        let err = kernel
            .run_borrowed_turn(
                agent_id,
                SessionTicket::empty(None),
                "hi",
                None,
                None,
                &[],
                None,
                None,
            )
            .await
            .expect_err("disabled borrowing must reject even local turns");
        assert!(err.to_string().contains("borrowing is disabled"));
    }
}
