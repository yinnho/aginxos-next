//! Session management and agent lifecycle — reset, compact, configure, kill.
//!
//! Handles session CRUD, session compaction, agent configuration mutations
//! (model, flows, MCP servers, tool filters), and agent termination.

use std::sync::Arc;

use carrier_types::agent::*;
use carrier_types::error::CarrierError;
use tracing::{debug, info, warn};

use crate::error::{KernelError, KernelResult};
use crate::kernel::CarrierKernel;

impl CarrierKernel {
    /// Reset an agent's session — auto-saves a summary to memory, then clears messages
    /// and creates a fresh session ID.
    pub fn reset_session(&self, agent_id: AgentId) -> KernelResult<()> {
        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        // Auto-save session context to workspace memory before clearing
        if let Ok(Some(old_session)) = self.memory.get_session(entry.session_id) {
            if old_session.messages.len() >= 2 {
                self.save_session_summary(agent_id, &entry, &old_session);
            }
        }

        // Delete the old session
        if let Err(e) = self.memory.delete_session(entry.session_id) {
            warn!(agent_id = %agent_id, error = %e, "Failed to delete old session");
        }

        // Create a fresh session
        let agent_name = entry.name.clone();
        let new_session = self
            .memory
            .create_session(agent_name)
            .map_err(KernelError::Carrier)?;

        // Update registry with new session ID
        self.registry
            .update_session_id(agent_id, new_session.id)
            .map_err(KernelError::Carrier)?;

        // Reset quota tracking so /new clears "token quota exceeded"
        self.runtime.scheduler.reset_usage(agent_id);

        info!(agent_id = %agent_id, "Session reset (summary saved to memory)");
        Ok(())
    }

    /// Clear ALL conversation history for an agent (sessions + canonical).
    ///
    /// Creates a fresh empty session afterward so the agent is still usable.
    pub fn clear_agent_history(&self, agent_id: AgentId) -> KernelResult<()> {
        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        // Delete all regular sessions
        if let Err(e) = self.memory.delete_agent_sessions(&entry.name) {
            warn!(agent = %entry.name, error = %e, "Failed to delete agent sessions");
        }

        // Delete canonical (cross-channel) session

        // Create a fresh session
        let agent_name = entry.name.clone();
        let new_session = self
            .memory
            .create_session(agent_name)
            .map_err(KernelError::Carrier)?;

        // Update registry with new session ID
        self.registry
            .update_session_id(agent_id, new_session.id)
            .map_err(KernelError::Carrier)?;

        info!(agent_id = %agent_id, "All agent history cleared");
        Ok(())
    }

    /// List all sessions for a specific agent.
    pub fn list_agent_sessions(&self, agent_id: AgentId) -> KernelResult<Vec<serde_json::Value>> {
        // Verify agent exists
        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        let mut sessions = self
            .memory
            .list_agent_sessions(&entry.name)
            .map_err(KernelError::Carrier)?;

        // Mark the active session
        for s in &mut sessions {
            if let Some(obj) = s.as_object_mut() {
                let is_active = obj
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|sid| sid == entry.session_id.0.to_string())
                    .unwrap_or(false);
                obj.insert("active".to_string(), serde_json::json!(is_active));
            }
        }

        Ok(sessions)
    }

    /// Create a new named session for an agent.
    pub fn create_agent_session(
        &self,
        agent_id: AgentId,
        label: Option<&str>,
    ) -> KernelResult<serde_json::Value> {
        // Verify agent exists
        let _entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        let agent_name = self
            .registry
            .get(agent_id)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| agent_id.to_string());
        let session = self
            .memory
            .create_session_with_label(agent_name, label)
            .map_err(KernelError::Carrier)?;

        // Switch to the new session
        self.registry
            .update_session_id(agent_id, session.id)
            .map_err(KernelError::Carrier)?;

        info!(agent_id = %agent_id, label = ?label, "Created new session");

        Ok(serde_json::json!({
            "session_id": session.id.0.to_string(),
            "label": session.label,
        }))
    }

    /// Switch an agent to an existing session by session ID.
    pub fn switch_agent_session(
        &self,
        agent_id: AgentId,
        session_id: SessionId,
    ) -> KernelResult<()> {
        // Verify agent exists
        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        // Verify session exists and belongs to this agent
        let session = self
            .memory
            .get_session(session_id)
            .map_err(KernelError::Carrier)?
            .ok_or_else(|| {
                KernelError::Carrier(CarrierError::Internal("Session not found".to_string()))
            })?;

        if session.agent_name != entry.name {
            return Err(KernelError::Carrier(CarrierError::Internal(
                "Session belongs to a different agent".to_string(),
            )));
        }

        self.registry
            .update_session_id(agent_id, session_id)
            .map_err(KernelError::Carrier)?;

        info!(agent_id = %agent_id, session_id = %session_id.0, "Switched session");
        Ok(())
    }

    /// Save a summary of the current session to agent memory before reset.
    fn save_session_summary(
        &self,
        agent_id: AgentId,
        entry: &AgentEntry,
        session: &carrier_memory::session::Session,
    ) {
        use carrier_types::message::{MessageContent, Role};

        // Take last 10 messages (or all if fewer)
        let recent = &session.messages[session.messages.len().saturating_sub(10)..];

        // Extract key topics from user messages
        let topics: Vec<&str> = recent
            .iter()
            .filter(|m| m.role == Role::User)
            .filter_map(|m| match &m.content {
                MessageContent::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();

        if topics.is_empty() {
            return;
        }

        // Generate a slug from first user message (first 6 words, slugified)
        let slug: String = topics[0]
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join("-")
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .take(60)
            .collect();

        let date = chrono::Utc::now().format("%Y-%m-%d");
        let summary = format!(
            "Session on {date}: {slug}\n\nKey exchanges:\n{}",
            topics
                .iter()
                .take(5)
                .enumerate()
                .map(|(i, t)| {
                    let truncated = carrier_types::truncate_str(t, 200);
                    format!("{}. {}", i + 1, truncated)
                })
                .collect::<Vec<_>>()
                .join("\n")
        );

        // Save to structured memory store (key = "session_{date}_{slug}").
        // NOTE: owner_id="" / user_id="" is intentional, not a bug. reset_session
        // is an owner-level action (HTTP/WS handlers carry no sender context), so
        // there is no single user to attribute this archival summary to. It lands
        // in the empty-string partition and is NOT picked up by the per-user
        // drawer prefetch — that's fine, this is archival, not inference recall.
        let key = format!("session_{date}_{slug}");
        // Through the shared memory handle — single write path, the same
        // substrate the tools and the agent loop see (memory is local, M35).
        let mem_handle = crate::handle::make_memory_handle(std::sync::Arc::clone(&self.memory));
        if let Err(e) = mem_handle.kv_set(
            &agent_id.to_string(),
            "",
            "",
            &key,
            serde_json::Value::String(summary.clone()),
        ) {
            warn!(agent_id = %agent_id, error = %e, "Failed to save session summary to KV store");
        }

        // Also write to workspace memory/ dir if workspace exists
        if let Some(ref workspace) = entry.manifest.workspace {
            let mem_dir = workspace.join("memory");
            let filename = format!("{date}-{slug}.md");
            if let Err(e) = std::fs::write(mem_dir.join(&filename), &summary) {
                warn!(agent_id = %agent_id, error = %e, "Failed to write session summary to workspace");
            }
        }

        debug!(
            agent_id = %agent_id,
            key = %key,
            "Saved session summary to memory before reset"
        );
    }

    // ── Agent configuration mutations ──────────────────────────

    /// Persist the agent's current registry entry to SQLite (best-effort).
    ///
    /// Single shared tail for every registry mutation — previously inlined
    /// 5× in kernel and 5× in the api routes with drifting warn messages,
    /// and occasionally forgotten entirely (set_agent_mode).
    pub fn persist_agent(&self, agent_id: AgentId) {
        if let Some(entry) = self.registry.get(agent_id) {
            if let Err(e) = self.memory.save_agent(&entry) {
                warn!(agent_id = %agent_id, error = %e, "Failed to persist agent update");
            }
        }
    }

    /// Switch an agent's modality (resolved to model by Brain at inference time).
    ///
    /// The `model` parameter is the modality name (e.g. "chat", "fast", "vision").
    /// Brain maps the modality to the actual provider/model/endpoint.
    pub fn set_agent_model(&self, agent_id: AgentId, model: &str) -> KernelResult<()> {
        // Model/provider management moved to Brain — this updates modality only
        let modality = model.to_string();

        self.registry
            .update_modality(agent_id, modality.clone())
            .map_err(KernelError::Carrier)?;
        info!(agent_id = %agent_id, modality = %modality, "Agent modality updated");

        // Persist the updated entry
        self.persist_agent(agent_id);

        // Clear canonical session to prevent memory poisoning from old model's responses
        debug!(agent_id = %agent_id, "Cleared canonical session after model switch");

        Ok(())
    }

    /// Update an agent's MCP server allowlist. Empty = all servers (backward compat).
    pub fn set_agent_mcp_servers(
        &self,
        agent_id: AgentId,
        servers: Vec<String>,
    ) -> KernelResult<()> {
        // Validate server names if allowlist is non-empty
        if !servers.is_empty() {
            let mut known_servers: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for entry in self.plugins.mcp_connections.iter() {
                known_servers.insert(entry.key().clone());
            }
            for name in &servers {
                let normalized = carrier_runtime::mcp::normalize_name(name);
                if !known_servers.contains(&normalized) {
                    return Err(KernelError::Carrier(CarrierError::Internal(format!(
                        "Unknown MCP server: {name}"
                    ))));
                }
            }
        }

        self.registry
            .update_mcp_servers(agent_id, servers.clone())
            .map_err(KernelError::Carrier)?;

        self.persist_agent(agent_id);

        info!(agent_id = %agent_id, servers = ?servers, "Agent MCP servers updated");
        Ok(())
    }

    /// Update an agent's tool allowlist and/or blocklist.
    pub fn set_agent_tool_filters(
        &self,
        agent_id: AgentId,
        allowlist: Option<Vec<String>>,
        blocklist: Option<Vec<String>>,
    ) -> KernelResult<()> {
        self.registry
            .update_tool_filters(agent_id, allowlist.clone(), blocklist.clone())
            .map_err(KernelError::Carrier)?;

        self.persist_agent(agent_id);

        info!(
            agent_id = %agent_id,
            allowlist = ?allowlist,
            blocklist = ?blocklist,
            "Agent tool filters updated"
        );
        Ok(())
    }

    /// Cancel an agent's currently running LLM task.
    pub fn stop_agent_run(&self, agent_id: AgentId) -> KernelResult<bool> {
        if let Some((_, handle)) = self.runtime.running_tasks.remove(&agent_id) {
            handle.abort();
            info!(agent_id = %agent_id, "Agent run cancelled");
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Re-read `agent.toml` from the agent's workspace and swap the registry
    /// manifest for the parsed copy (shared by the KernelHandle restart path
    /// and the API restart route — previously two diverging copies).
    ///
    /// Empty `display_name`/`description` are filled from the workspace's
    /// `template.json`: agent.toml is generated at install time and dup
    /// pushes never touch the runtime layer, so definition-layer backfills
    /// (e.g. 2026-08-17 Chinese display_name pass) would otherwise never
    /// reach a running agent. Fill-if-empty only — a non-empty runtime
    /// value is never clobbered.
    ///
    /// Returns true when the registry manifest was replaced.
    pub fn reload_manifest_from_workspace(&self, agent_id: AgentId) -> bool {
        let Some(entry) = self.registry.get(agent_id) else {
            return false;
        };
        let Some(ref ws) = entry.manifest.workspace else {
            return false;
        };
        let toml_path = ws.join("agent.toml");
        if !toml_path.exists() {
            return false;
        }
        let Ok(toml_str) = std::fs::read_to_string(&toml_path) else {
            warn!(agent = %entry.name, "Failed to read agent.toml on restart");
            return false;
        };
        let Ok(mut new_manifest) = toml::from_str::<AgentManifest>(&toml_str) else {
            // Surface type drift that would silently empty security fields.
            warn!(agent = %entry.name, "Failed to parse agent.toml on restart");
            return false;
        };
        let drift = carrier_types::serde_compat::take_lenient_diagnostics();
        if !drift.is_empty() {
            warn!(
                agent = %entry.name,
                count = drift.len(),
                details = ?drift,
                "agent.toml fields fell back to empty defaults due to type drift — check tool_blocklist/tool_allowlist"
            );
        }
        // Definition-layer overlay: template.json fills empty presentation
        // metadata (see doc comment).
        fill_presentation_from_template_json(&mut new_manifest, ws);
        new_manifest.workspace = Some(ws.clone());
        if new_manifest.exec_policy.is_none() {
            new_manifest.exec_policy = Some(self.config.exec_policy.clone());
        }
        if self
            .registry
            .update_manifest(agent_id, new_manifest.clone())
            .is_err()
        {
            return false;
        }
        let caps = crate::capabilities::manifest_to_capabilities(&new_manifest);
        self.coordination.capabilities.grant(agent_id, caps);
        self.persist_agent(agent_id);
        info!(agent = %entry.name, "Reloaded manifest from agent.toml (+template.json fill-if-empty overlay)");
        true
    }

    /// Resolve the "fast" modality (fallback: the agent's own) plus its model
    /// and driver, for background LLM work (compaction, intent classification).
    ///
    /// Previously inlined twice in this file with drifting error strings — a
    /// fix to one copy silently missed the other.
    fn resolve_fast_llm(
        &self,
        entry: &AgentEntry,
        purpose: &str,
    ) -> KernelResult<(
        String,
        String,
        std::sync::Arc<dyn carrier_runtime::llm_driver::LlmDriver>,
    )> {
        let modality = if self.brain_read().has_modality("fast") {
            "fast".to_string()
        } else {
            entry.manifest.model.modality.clone()
        };
        let model = self.brain_read().model_for(&modality);
        let driver = {
            let brain = self.brain_read();
            let endpoints = brain.endpoints_for(&modality);
            if let Some(ep) = endpoints.first() {
                brain.driver_for_endpoint(&ep.id).ok_or_else(|| {
                    KernelError::Carrier(CarrierError::LlmDriver(format!(
                        "No driver for {purpose} modality '{modality}'"
                    )))
                })?
            } else {
                self.resolve_driver(&entry.manifest)?
            }
        };
        Ok((modality, model, driver))
    }

    /// Compact an agent's session using LLM-based summarization.
    ///
    /// Replaces the existing text-truncation compaction with an intelligent
    /// LLM-generated summary of older messages, keeping only recent messages.
    pub async fn compact_agent_session(
        &self,
        agent_id: AgentId,
        session_id: carrier_types::agent::SessionId,
        owner_id: Option<&str>,
        user_id: Option<&str>,
    ) -> KernelResult<String> {
        use carrier_runtime::compactor::{
            compact_session, estimate_token_count, needs_compaction, needs_compaction_by_tokens,
            CompactionConfig,
        };

        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        let session = self
            .memory
            .get_session(session_id)
            .map_err(KernelError::Carrier)?
            .unwrap_or_else(|| carrier_memory::session::Session {
                id: session_id,
                agent_name: entry.name.clone(),
                messages: Vec::new(),
                context_window_tokens: 0,
                turn_summaries: Vec::new(),
                label: None,
            });

        let config = CompactionConfig::default();

        let by_messages = needs_compaction(&session, &config);
        let by_tokens = {
            let estimated = estimate_token_count(&session.messages, None, None);
            needs_compaction_by_tokens(estimated, &config)
        };

        if !by_messages && !by_tokens {
            return Ok(format!(
                "No compaction needed ({} messages, threshold {})",
                session.messages.len(),
                config.threshold
            ));
        }

        // Use "fast" modality for compaction (cheaper, faster); fall back to agent modality
        let (_compaction_modality, compaction_model, driver) =
            self.resolve_fast_llm(&entry, "compaction")?;

        let result = compact_session(driver, &compaction_model, &session, &config)
            .await
            .map_err(KernelError::Carrier)?;

        // Post-compaction audit: validate and repair the kept messages
        let (repaired_messages, repair_stats) =
            carrier_runtime::session_repair::validate_and_repair_with_stats(&result.kept_messages);

        // Prepend summary as a User message. Role::User is required (not System) because
        // agent_loop filters out Role::System messages from the LLM request. The split
        // alignment in compact_session ensures kept[0] is Assistant, so the summary
        // (User) + kept[0] (Assistant) pair won't be merged by validate_and_repair.
        // The SESSION_SUMMARY_PREFIX marker lets the next compaction recognize this
        // message and fold it in as labeled context instead of re-summarizing it.
        let mut final_messages = Vec::new();
        if !result.summary.is_empty() {
            final_messages.push(carrier_types::message::Message::user(format!(
                "{}\n{}",
                carrier_runtime::compactor::SESSION_SUMMARY_PREFIX,
                result.summary
            )));
        }
        final_messages.extend(repaired_messages);

        // Also update the regular session with the repaired messages
        // P1-C: compaction becomes a replace event in the session event log
        // (non-destructive at the fact level — the shadowed originals stay
        // foldable for audit/rebuild). "Must be smaller": a summary that
        // isn't cheaper than the span it shadows is rejected — the event is
        // skipped so the folded surface keeps the pre-compaction history.
        // (Counting note: `shadowed_msgs` is in DB-row message space, which
        // maps 1:1 to message events for sessions born after P1-A; for a
        // partially-logged legacy session the fold just shadows everything
        // logged so far, which only ever over-summarizes the tail.)
        {
            let split = result.compacted_count.min(session.messages.len());
            let shadowed_msgs = result.compacted_count as u64;
            let shadowed_tokens_est =
                estimate_token_count(&session.messages[..split], None, None) as u64;
            let summary_tokens_est = (result.summary.len() as u64) / 4;
            if summary_tokens_est < shadowed_tokens_est {
                self.memory.session_events_append(
                    &entry.name,
                    &session_id.0.to_string(),
                    vec![carrier_memory::SessionEventKind::CompactionSummary {
                        summary: result.summary.clone(),
                        shadowed_msgs,
                        shadowed_tokens_est,
                    }],
                );
            } else {
                warn!(
                    agent = %entry.name,
                    summary_est = summary_tokens_est,
                    shadowed_est = shadowed_tokens_est,
                    "compaction summary not smaller than shadowed span — event skipped, folded surface unchanged"
                );
            }
        }
        let mut updated_session = session;
        updated_session.messages = final_messages;
        // The compaction summary message (final_messages[0]) now captures the
        // old turns, so the per-turn L0 summaries for those turns are orphaned —
        // keeping them would inject stale intent/outcome labels that reference
        // messages which no longer exist. Reset the L0 layer; it rebuilds over
        // the upcoming turns. (Kept recent messages are still in the prompt
        // directly, so no context is lost.)
        let cleared_summaries = updated_session.turn_summaries.len();
        updated_session.turn_summaries.clear();
        self.memory
            .save_session(&updated_session)
            .map_err(KernelError::Carrier)?;

        // Write-back bridge: persist the compaction summary + extracted facts to
        // the durable kv drawer so they survive a session reset and stay
        // retrievable. The per-turn path already flushes each turn's key_facts
        // to the drawer, so this mainly (a) saves the cross-turn prose summary
        // and (b) rescues facts from turns whose per-turn summary failed.
        // Runs on the canonical kv partition (owner_id or "") — the old
        // `if let Some(owner)` guard skipped this entirely on every production
        // path (API/channel/cron all pass owner_id=None), so compaction
        // summaries were never persisted. The prompt side recalls the two most
        // recent session_compaction.* entries (prefetch_kv_memories).
        let owner = owner_id.unwrap_or("");
        // Resolve user: explicit param → session label ("user:{id}") → "".
        let uid = user_id
            .map(str::to_string)
            .or_else(|| {
                updated_session
                    .label
                    .as_deref()
                    .and_then(|l| l.strip_prefix("user:"))
                    .map(str::to_string)
            })
            .unwrap_or_default();

        // 1. Persist the prose compaction summary under a non-state prefix
        //    (session_compaction.*) — recalled into the next prompt as
        //    cross-session memory (prefetch_kv_memories).
        if !result.summary.is_empty() {
            let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let summary_key = format!("session_compaction.{date}");
            // Through the shared memory handle — single write path (memory
            // is local, M35).
            let mem_handle = crate::handle::make_memory_handle(std::sync::Arc::clone(&self.memory));
            match mem_handle.kv_set(
                &entry.name,
                owner,
                &uid,
                &summary_key,
                serde_json::Value::String(result.summary.clone()),
            ) {
                Ok(()) => info!(
                    agent_id = %agent_id, key = %summary_key,
                    facts = result.key_facts.len(),
                    "Compaction summary persisted to kv"
                ),
                Err(e) => warn!(
                    agent_id = %agent_id, error = %e,
                    "Failed to persist compaction summary to kv"
                ),
            }
        }

        // 2. Flush structured facts via the same idempotent merge the
        //    per-turn path uses (state keys dedup, event.* now dedup too).
        if !result.key_facts.is_empty() {
            let handle: Arc<dyn carrier_runtime::memory_handle::MemoryHandle> =
                crate::handle::make_memory_handle(Arc::clone(&self.memory));
            carrier_runtime::agent_loop::merge_key_facts(
                &handle,
                &entry.name,
                owner,
                &uid,
                &result.key_facts,
            );
        }

        // Build result message with audit summary
        let mut msg = format!(
            "Compacted {} messages into summary ({} chars), kept {} recent messages.",
            result.compacted_count,
            result.summary.len(),
            updated_session.messages.len()
        );
        if cleared_summaries > 0 {
            msg.push_str(&format!(
                " Cleared {cleared_summaries} stale turn summaries (L0 rebuilds)."
            ));
        }

        let repairs = repair_stats.orphaned_results_removed
            + repair_stats.synthetic_results_inserted
            + repair_stats.duplicates_removed
            + repair_stats.messages_merged;
        if repairs > 0 {
            msg.push_str(&format!(" Post-audit: repaired ({} orphaned removed, {} synthetic inserted, {} merged, {} deduped).",
                repair_stats.orphaned_results_removed,
                repair_stats.synthetic_results_inserted,
                repair_stats.messages_merged,
                repair_stats.duplicates_removed,
            ));
        } else {
            msg.push_str(" Post-audit: clean.");
        }

        Ok(msg)
    }

    /// Run the intent classifier and switch the agent to a new session if the
    /// new message is judged to start a new conversation.
    ///
    /// Returns Ok(()) on success (whether or not a rotation occurred). Returns
    /// Err on infrastructure failure (DB, LLM, etc.); the caller is expected
    /// to fall back gracefully.
    pub async fn maybe_rotate_session_by_intent(
        &self,
        agent_id: AgentId,
        entry: &AgentEntry,
        new_user_msg: &str,
    ) -> KernelResult<()> {
        use carrier_runtime::intent_classifier::classify_intent;
        use carrier_types::message::{MessageContent, Role};

        // Load current session. If missing or empty, nothing to classify against.
        let session = match self.memory.get_session(entry.session_id) {
            Ok(Some(s)) if !s.messages.is_empty() => s,
            _ => return Ok(()),
        };

        // Extract last assistant message text for classifier context.
        let last_assistant: Option<String> = session
            .messages
            .iter()
            .rev()
            .find(|m| m.role == Role::Assistant)
            .and_then(|m| match &m.content {
                MessageContent::Text(t) => Some(t.clone()),
                MessageContent::Blocks(blocks) => {
                    let text: String = blocks
                        .iter()
                        .filter_map(|b| match b {
                            carrier_types::message::ContentBlock::Text { text, .. } => {
                                Some(text.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    if text.is_empty() {
                        None
                    } else {
                        Some(text)
                    }
                }
            });

        // Prefer the "fast" modality for cheap/quick classification.
        let (_modality, model, driver) = self.resolve_fast_llm(entry, "classifier")?;

        let classification =
            classify_intent(driver, &model, last_assistant.as_deref(), new_user_msg)
                .await
                .map_err(KernelError::Carrier)?;

        if classification.is_new {
            tracing::info!(
                agent_id = %agent_id,
                reasoning = %classification.reasoning,
                "Intent: new conversation — rotating session"
            );
            let new_session = self
                .memory
                .create_session(entry.name.clone())
                .map_err(KernelError::Carrier)?;
            self.registry
                .update_session_id(agent_id, new_session.id)
                .map_err(KernelError::Carrier)?;
        } else {
            tracing::info!(
                agent_id = %agent_id,
                reasoning = %classification.reasoning,
                "Intent: continuing session"
            );
        }

        Ok(())
    }

    /// Generate a context window usage report for an agent.
    pub fn context_report(
        &self,
        agent_id: AgentId,
    ) -> KernelResult<carrier_runtime::compactor::ContextReport> {
        use carrier_runtime::compactor::generate_context_report;

        let entry = self.registry.get(agent_id).ok_or_else(|| {
            KernelError::Carrier(CarrierError::AgentNotFound(agent_id.to_string()))
        })?;

        let session = self
            .memory
            .get_session(entry.session_id)
            .map_err(KernelError::Carrier)?
            .unwrap_or_else(|| carrier_memory::session::Session {
                id: entry.session_id,
                agent_name: entry.name.clone(),
                messages: Vec::new(),
                context_window_tokens: 0,
                turn_summaries: Vec::new(),
                label: None,
            });

        let system_prompt = &entry.manifest.model.system_prompt;
        // Core tool set (same as messaging.rs — other tools injected per manifest/flow)
        let mut tools: Vec<carrier_types::tool::ToolDefinition> =
            carrier_runtime::tool_runner::builtin_tool_definitions(self.config.cli_exec.clone())
                .into_iter()
                .filter(|t| carrier_types::tool::CORE_TOOL_NAMES.contains(&t.name.as_str()))
                .collect();
        if !entry.manifest.subagents.is_empty() {
            tools.extend(carrier_types::agent::build_subagent_tool_definitions(
                &entry.manifest.subagents,
            ));
        }
        // Use 200K default or the model's known context window
        let context_window = if session.context_window_tokens > 0 {
            session.context_window_tokens
        } else {
            200_000
        };

        Ok(generate_context_report(
            &session.messages,
            Some(system_prompt),
            Some(&tools),
            context_window as usize,
        ))
    }

    /// Kill an agent.
    pub fn kill_agent(&self, agent_id: AgentId) -> KernelResult<()> {
        let entry = self
            .registry
            .remove(agent_id)
            .map_err(KernelError::Carrier)?;
        self.runtime.background.stop_agent(agent_id);
        self.runtime.scheduler.unregister(agent_id);
        self.coordination.capabilities.revoke_all(agent_id);
        self.coordination.event_bus.unsubscribe_agent(agent_id);

        // Remove cron jobs so they don't linger as orphans (#504)
        let cron_removed = self.cron_scheduler.remove_agent_jobs(agent_id);
        if cron_removed > 0 {
            if let Err(e) = self.cron_scheduler.persist() {
                warn!("Failed to persist cron jobs after agent deletion: {e}");
            }
        }

        // Remove from persistent storage
        if let Err(e) = self.memory.remove_agent(agent_id) {
            warn!(agent_id = %agent_id, error = %e, "Failed to remove agent from persistent storage");
        }

        // aginx 入网钩子：分身卸载即离网（~/.aginx/agents/<name>/ 移除）。
        if let Err(e) = crate::aginx_net::unregister_clone_default(&entry.name) {
            warn!(name = %entry.name, error = %e, "Failed to remove aginx registration");
        }

        // Clean up per-agent runtime resources to prevent leaks
        self.runtime.running_tasks.remove(&agent_id);
        if let Ok(mut bindings) = self.coordination.bindings.lock() {
            bindings.retain(|b| b.agent != entry.name);
        }

        // SECURITY: Record agent kill in audit trail
        self.audit_log.record(
            agent_id.to_string(),
            carrier_runtime::audit::AuditAction::AgentKill,
            format!("name={}", entry.name),
            "ok",
        );

        info!(agent = %entry.name, id = %agent_id, "Agent killed");
        Ok(())
    }
}

/// Fill an empty `display_name`/`description` from the workspace's
/// `template.json` (definition layer). Parses via `serde_json::Value` and
/// extracts only the two fields: real-world template.json files drift from
/// `carrier_clone::TemplateManifest` (e.g. `mcp_servers` as objects `[{name,
/// required}]` where the struct wants `Vec<String>`), and a whole-struct
/// parse failure would silently disable the fill — the same trap
/// `read_template_default_flow` already guards against.
pub(crate) fn fill_presentation_from_template_json(
    manifest: &mut AgentManifest,
    ws: &std::path::Path,
) {
    let Ok(content) = std::fs::read_to_string(ws.join("template.json")) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    if manifest.display_name.trim().is_empty() {
        if let Some(dn) = v
            .get("display_name")
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            manifest.display_name = dn.to_string();
        }
    }
    if manifest.description.trim().is_empty() {
        if let Some(d) = v
            .get("description")
            .and_then(|x| x.as_str())
            .filter(|s| !s.trim().is_empty())
        {
            manifest.description = d.to_string();
        }
    }
}

#[cfg(test)]
mod presentation_fill_tests {
    use super::*;

    fn temp_ws() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sessions-presentation-fill-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn bare_manifest() -> AgentManifest {
        AgentManifest {
            display_name: String::new(),
            description: String::new(),
            ..serde::Deserialize::deserialize(serde_json::json!({"name": "t"}))
                .expect("manifest fixture")
        }
    }

    /// Real-world template.json shape (mcp_servers as OBJECTS, exported_at as
    /// string number) must not defeat the presentation fill — regression:
    /// 2026-08-17 the overlay used full-struct TemplateManifest parsing and
    /// silently skipped 8 clones whose template.json carried this drift.
    #[test]
    fn fill_ignores_struct_drift_and_fills_empty_fields() {
        let ws = temp_ws();
        std::fs::write(
            ws.join("template.json"),
            r#"{
                "version": "2",
                "name": "ai-writer",
                "display_name": "AI科技写手",
                "description": "AI科技公众号写手",
                "exported_at": "1745395200",
                "mcp_servers": [{"name": "searxng", "required": true}]
            }"#,
        )
        .unwrap();
        let mut m = bare_manifest();
        fill_presentation_from_template_json(&mut m, &ws);
        assert_eq!(m.display_name, "AI科技写手");
        assert_eq!(m.description, "AI科技公众号写手");
    }

    /// Non-empty runtime values are never clobbered (fill-if-empty only).
    #[test]
    fn fill_never_clobbers_non_empty_runtime_values() {
        let ws = temp_ws();
        std::fs::write(
            ws.join("template.json"),
            r#"{"display_name": "模板名", "description": "模板描述"}"#,
        )
        .unwrap();
        let mut m = bare_manifest();
        m.display_name = "运行时名".into();
        m.description = "运行时描述".into();
        fill_presentation_from_template_json(&mut m, &ws);
        assert_eq!(m.display_name, "运行时名");
        assert_eq!(m.description, "运行时描述");
    }
}
