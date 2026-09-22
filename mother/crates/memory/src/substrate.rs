//! MemorySubstrate: unified memory substrate built around the tree memory system.
//!
//! Composes the system KV store, session store, cron delivery store,
//! and tree memory behind a single API.

use crate::automation_store::AutomationRuleStore;
use crate::chain_resume_store::ChainResumeStore;
use crate::cron_delivery::CronDeliveryStore;
use crate::cron_store::CronJobStore;
use crate::flow_run::FlowRunStore;
use crate::follower_store::FollowerStore;
use crate::migration::run_migrations;
use crate::notify_store::NotifyRouteStore;
use crate::session::{Session, SessionStore};
use crate::system_kv::SystemKV;
use crate::tree::ingest::IngestPipeline;
use crate::tree::retrieval;
use crate::tree::types::SourceKind;
use crate::usage::UsageStore;
use crate::weixin_store::WeixinSessionStore;

use carrier_types::agent::{AgentEntry, AgentId, SessionId};
use carrier_types::automation::AutomationRule;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::memory_tree::{
    DrillDownQuery, EntityMatch, EntitySearch, FetchLeavesQuery, GlobalQuery, IngestRequest,
    IngestResult, QueryResponse, SourceQuery, TopicQuery, TreeSummary,
};
use carrier_types::message::Message;

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The unified memory substrate. Tree memory is the primary memory interface;
/// system_kv, sessions, and cron_delivery are infrastructure stores.
pub struct MemorySubstrate {
    conn: Arc<Mutex<Connection>>,
    system_kv: SystemKV,
    sessions: SessionStore,
    cron_delivery: CronDeliveryStore,
    cron_store: CronJobStore,
    followers: FollowerStore,
    weixin_store: WeixinSessionStore,
    notify_store: NotifyRouteStore,
    flow_runs: FlowRunStore,
    automation_rules: AutomationRuleStore,
    chain_resume: ChainResumeStore,
    tickets: crate::ticket_store::TicketStore,
    content_root: PathBuf,
    /// Append-only session event log (P1-A observational bypass — see
    /// [`crate::session_events`]).
    session_events: crate::session_events::SessionEventLog,
}

impl MemorySubstrate {
    /// Open or create a memory substrate at the given database path.
    pub fn open(db_path: &Path) -> CarrierResult<Self> {
        let conn = Connection::open(db_path).map_err(|e| CarrierError::Memory(e.to_string()))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;",
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        run_migrations(&conn).map_err(|e| CarrierError::Memory(e.to_string()))?;
        let shared = Arc::new(Mutex::new(conn));

        // Default content root: sibling to the db file
        let content_root = db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("memory_tree")
            .join("content");
        // Event log lives beside the db, kernel-owned (not per-workspace).
        let events_root = db_path
            .parent()
            .unwrap_or(Path::new("."))
            .join("session-events");

        Ok(Self {
            conn: Arc::clone(&shared),
            system_kv: SystemKV::new(Arc::clone(&shared)),
            sessions: SessionStore::new(Arc::clone(&shared)),
            cron_delivery: CronDeliveryStore::new(Arc::clone(&shared)),
            cron_store: CronJobStore::new(Arc::clone(&shared)),
            followers: FollowerStore::new(Arc::clone(&shared)),
            weixin_store: WeixinSessionStore::new(Arc::clone(&shared)),
            notify_store: NotifyRouteStore::new(Arc::clone(&shared)),
            flow_runs: FlowRunStore::new(Arc::clone(&shared)),
            automation_rules: AutomationRuleStore::new(Arc::clone(&shared)),
            chain_resume: ChainResumeStore::new(Arc::clone(&shared)),
            tickets: crate::ticket_store::TicketStore::new(Arc::clone(&shared)),
            content_root,
            session_events: crate::session_events::SessionEventLog::new(events_root),
        })
    }

    /// Create an in-memory substrate (for testing).
    pub fn open_in_memory() -> CarrierResult<Self> {
        // Tests share a per-process dir so cross-instance seq continuity
        // is exercised without touching the real db location.
        let events_root = std::env::temp_dir()
            .join(format!("opencarrier-session-events-{}", std::process::id()));
        Self::open_in_memory_with_events(events_root)
    }

    /// Create an in-memory substrate whose session-event log lives at
    /// `events_root` (instead of the shared per-process temp dir).
    ///
    /// Borrowed turns (`run_borrowed_turn`) use this to point the event log at
    /// a per-turn temp directory that is deleted when the turn ends — so a
    /// borrowed session leaves no durable trace on the host.
    pub fn open_in_memory_with_events(events_root: PathBuf) -> CarrierResult<Self> {
        let conn = Connection::open_in_memory().map_err(|e| CarrierError::Memory(e.to_string()))?;
        run_migrations(&conn).map_err(|e| CarrierError::Memory(e.to_string()))?;
        let shared = Arc::new(Mutex::new(conn));

        Ok(Self {
            conn: Arc::clone(&shared),
            system_kv: SystemKV::new(Arc::clone(&shared)),
            sessions: SessionStore::new(Arc::clone(&shared)),
            cron_delivery: CronDeliveryStore::new(Arc::clone(&shared)),
            cron_store: CronJobStore::new(Arc::clone(&shared)),
            followers: FollowerStore::new(Arc::clone(&shared)),
            weixin_store: WeixinSessionStore::new(Arc::clone(&shared)),
            notify_store: NotifyRouteStore::new(Arc::clone(&shared)),
            flow_runs: FlowRunStore::new(Arc::clone(&shared)),
            automation_rules: AutomationRuleStore::new(Arc::clone(&shared)),
            chain_resume: ChainResumeStore::new(Arc::clone(&shared)),
            tickets: crate::ticket_store::TicketStore::new(Arc::clone(&shared)),
            content_root: PathBuf::from("/tmp/opencarrier_tree_content"),
            session_events: crate::session_events::SessionEventLog::new(events_root),
        })
    }

    /// Get a reference to the cron delivery store (last-channel tracking + buffer).
    pub fn cron_delivery(&self) -> &CronDeliveryStore {
        &self.cron_delivery
    }

    /// Get a reference to the cron job store (persistent cron_jobs table).
    pub fn cron_store(&self) -> &CronJobStore {
        &self.cron_store
    }

    /// Get a reference to the weixin session store.
    pub fn weixin_store(&self) -> &WeixinSessionStore {
        &self.weixin_store
    }

    /// Get a reference to the notify route store.
    pub fn notify_store(&self) -> &NotifyRouteStore {
        &self.notify_store
    }

    /// Get a reference to the flow run store (multi-step flow execution state).
    pub fn flow_runs(&self) -> &FlowRunStore {
        &self.flow_runs
    }

    /// Get a reference to the automation rule store.
    pub fn automation_rules(&self) -> &AutomationRuleStore {
        &self.automation_rules
    }

    /// Get a reference to the chain-resume ledger (断链自动接续 attempt
    /// budgets). Daemon-side sync access — same precedent as
    /// [`Self::cron_delivery`].
    pub fn chain_resume(&self) -> &ChainResumeStore {
        &self.chain_resume
    }

    /// Get a reference to the user-side ticket store (借用机制的会话真源在
    /// 用户侧——App/CLI 把借道轮返回的票据存这里，下次借用取回提交)。
    pub fn tickets(&self) -> &crate::ticket_store::TicketStore {
        &self.tickets
    }

    // -----------------------------------------------------------------
    // Automation rules (subscribe/keyword -> fixed push, no LLM)
    // -----------------------------------------------------------------

    /// List automation rules for (channel, app_id), highest priority first.
    pub async fn automation_rule_list(
        &self,
        channel: &str,
        app_id: &str,
    ) -> CarrierResult<Vec<AutomationRule>> {
        let store = self.automation_rules.clone();
        let channel = channel.to_string();
        let app_id = app_id.to_string();
        tokio::task::spawn_blocking(move || store.list_by_app(&channel, &app_id))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Insert or update an automation rule.
    pub async fn automation_rule_upsert(&self, rule: AutomationRule) -> CarrierResult<()> {
        let store = self.automation_rules.clone();
        tokio::task::spawn_blocking(move || store.upsert(&rule))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Delete an automation rule by id.
    pub async fn automation_rule_delete(&self, id: &str) -> CarrierResult<()> {
        let store = self.automation_rules.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || store.delete(&id))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    // -----------------------------------------------------------------
    // Followers ledger (automation Phase 2: scheduled pushes + growth digests)
    // -----------------------------------------------------------------

    /// Record a follow (upsert; re-follow clears unfollowed_at).
    pub async fn follower_record_follow(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
        unionid: Option<&str>,
        scene: Option<&str>,
    ) -> CarrierResult<()> {
        let store = self.followers.clone();
        let (channel, app_id, openid) =
            (channel.to_string(), app_id.to_string(), openid.to_string());
        let (unionid, scene) = (unionid.map(str::to_string), scene.map(str::to_string));
        let now = chrono::Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            store.record_follow(
                &channel,
                &app_id,
                &openid,
                unionid.as_deref(),
                scene.as_deref(),
                &now,
            )
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Refresh `last_seen` on any inbound message (creates the row if missing).
    pub async fn follower_touch(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
    ) -> CarrierResult<()> {
        let store = self.followers.clone();
        let (channel, app_id, openid) =
            (channel.to_string(), app_id.to_string(), openid.to_string());
        let now = chrono::Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || store.touch(&channel, &app_id, &openid, &now))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Stamp unfollowed_at (row kept for growth stats).
    pub async fn follower_mark_unfollowed(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
    ) -> CarrierResult<()> {
        let store = self.followers.clone();
        let (channel, app_id, openid) =
            (channel.to_string(), app_id.to_string(), openid.to_string());
        let now = chrono::Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || store.mark_unfollowed(&channel, &app_id, &openid, &now))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Active followers with `last_seen_at` >= since (deliverable audience).
    pub async fn follower_list_pushable(
        &self,
        channel: &str,
        app_id: &str,
        since_rfc3339: &str,
    ) -> CarrierResult<Vec<String>> {
        let store = self.followers.clone();
        let (channel, app_id, since) = (
            channel.to_string(),
            app_id.to_string(),
            since_rfc3339.to_string(),
        );
        tokio::task::spawn_blocking(move || store.list_pushable_since(&channel, &app_id, &since))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Growth summary for FollowerReport digests.
    pub async fn follower_stats(
        &self,
        channel: &str,
        app_id: &str,
        since_rfc3339: &str,
        push_window_since_rfc3339: &str,
    ) -> CarrierResult<crate::follower_store::FollowerStats> {
        let store = self.followers.clone();
        let (channel, app_id, s1, s2) = (
            channel.to_string(),
            app_id.to_string(),
            since_rfc3339.to_string(),
            push_window_since_rfc3339.to_string(),
        );
        tokio::task::spawn_blocking(move || store.stats_since(&channel, &app_id, &s1, &s2))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    // -----------------------------------------------------------------
    // Analytics operations (for data_analyze tool)
    // -----------------------------------------------------------------

    /// User statistics: total users, active users, new users.
    pub fn analytics_user_stats(
        &self,
        agent_id: &str,
        active_days: u32,
    ) -> CarrierResult<serde_json::Value> {
        let users = self.sessions.list_agent_users(agent_id)?;
        let total_users = users.len() as u32;
        let active_users = self.sessions.count_active_users(agent_id, active_days)?;
        let new_users = self.sessions.count_new_users(agent_id, active_days)?;
        Ok(serde_json::json!({
            "total_users": total_users,
            "active_users": active_users,
            "new_users": new_users,
            "active_days": active_days,
        }))
    }

    /// Per-user lookup: session count, last active, recent conversation summary.
    pub fn analytics_user_lookup(
        &self,
        agent_id: &str,
        sender_id: &str,
    ) -> CarrierResult<serde_json::Value> {
        let user_sessions = self.sessions.list_user_sessions(agent_id, sender_id)?;
        let total_sessions = user_sessions.len();
        let mut total_messages = 0;
        let mut recent_summary = Vec::new();

        for (_session_id, messages) in &user_sessions {
            total_messages += messages.len();
        }

        // Extract summary from the most recent session's last few user/assistant messages
        if let Some((_last_id, last_msgs)) = user_sessions.last() {
            let mut count = 0;
            for msg in last_msgs.iter().rev() {
                if count >= 3 {
                    break;
                }
                let text = msg.content.text_content();
                if !text.is_empty() && msg.role != carrier_types::message::Role::System {
                    let role_str = match msg.role {
                        carrier_types::message::Role::User => "user",
                        carrier_types::message::Role::Assistant => "assistant",
                        carrier_types::message::Role::System => "system",
                    };
                    let truncated = if text.len() > 200 {
                        let mut end = 200;
                        while !text.is_char_boundary(end) && end > 0 {
                            end -= 1;
                        }
                        format!("{}...", &text[..end])
                    } else {
                        text
                    };
                    recent_summary.push(serde_json::json!({
                        "role": role_str,
                        "text": truncated,
                    }));
                    count += 1;
                }
            }
            recent_summary.reverse();
        }

        // Get last_active from list_agent_users for this sender
        let last_active = self
            .sessions
            .list_agent_users(agent_id)?
            .into_iter()
            .find(|u| u.get("sender_id").and_then(|v| v.as_str()) == Some(sender_id))
            .and_then(|u| {
                u.get("last_active")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_default();

        Ok(serde_json::json!({
            "sender_id": sender_id,
            "session_count": total_sessions,
            "total_messages": total_messages,
            "last_active": last_active,
            "recent_summary": recent_summary,
        }))
    }

    /// Usage analytics: token consumption, daily trends, per-model breakdown.
    pub fn analytics_usage(&self, agent_id: &str, days: u32) -> CarrierResult<serde_json::Value> {
        let usage = self.usage();
        let summary = usage.query_summary(Some(carrier_types::agent::AgentId::from_string(agent_id)))?;
        let daily = usage.query_daily_breakdown_for_agent(agent_id, days)?;
        let by_model = usage.query_by_model()?;

        Ok(serde_json::json!({
            "summary": {
                "total_input_tokens": summary.total_input_tokens,
                "total_output_tokens": summary.total_output_tokens,
                "call_count": summary.call_count,
                "total_tool_calls": summary.total_tool_calls,
            },
            "daily_trend": daily.iter().map(|d| serde_json::json!({
                "date": d.date,
                "tokens": d.tokens,
                "calls": d.calls,
            })).collect::<Vec<_>>(),
            "by_model": by_model.iter().map(|m| serde_json::json!({
                "model": m.model,
                "total_input_tokens": m.total_input_tokens,
                "total_output_tokens": m.total_output_tokens,
                "call_count": m.call_count,
            })).collect::<Vec<_>>(),
        }))
    }

    /// Recent conversations list (metadata only, no message content).
    pub fn analytics_recent_conversations(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> CarrierResult<serde_json::Value> {
        let sessions = self.sessions.recent_sessions(agent_id, limit)?;
        Ok(serde_json::json!({
            "conversations": sessions,
        }))
    }

    /// Create a new UsageStore from the shared connection.
    pub fn usage(&self) -> UsageStore {
        UsageStore::new(Arc::clone(&self.conn))
    }

    /// Get the shared database connection (for constructing stores from outside).
    pub fn usage_conn(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }

    // -----------------------------------------------------------------
    // System KV operations (agent entries, schedules, config)
    // -----------------------------------------------------------------

    /// Save an agent entry to persistent storage.
    pub fn save_agent(&self, entry: &AgentEntry) -> CarrierResult<()> {
        self.system_kv.save_agent(entry)
    }

    /// Load an agent entry from persistent storage.
    pub fn load_agent(&self, agent_id: AgentId) -> CarrierResult<Option<AgentEntry>> {
        self.system_kv.load_agent(agent_id)
    }

    /// Remove an agent from persistent storage and cascade-delete sessions.
    pub fn remove_agent(&self, agent_id: AgentId) -> CarrierResult<()> {
        let _ = self.sessions.delete_agent_sessions(&agent_id.to_string());
        self.system_kv.remove_agent(agent_id)
    }

    /// Load all agent entries from persistent storage.
    pub fn load_all_agents(&self) -> CarrierResult<Vec<AgentEntry>> {
        self.system_kv.load_all_agents()
    }

    /// List all saved agents.
    pub fn list_agents(&self) -> CarrierResult<Vec<(String, String, String)>> {
        self.system_kv.list_agents()
    }

    /// Synchronous get from the system KV store.
    pub fn system_kv_get(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<Option<serde_json::Value>> {
        self.system_kv.get(agent_id, owner_id, user_id, key)
    }

    /// Synchronous set in the system KV store.
    pub fn system_kv_set(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> CarrierResult<()> {
        self.system_kv.set(agent_id, owner_id, user_id, key, value)
    }

    /// List all KV pairs for an agent (per-user).
    pub fn list_kv(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
    ) -> CarrierResult<Vec<(String, serde_json::Value)>> {
        self.system_kv.list_kv(agent_id, owner_id, user_id)
    }

    /// Delete a KV entry for an agent (per-user).
    pub fn system_kv_delete(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<()> {
        self.system_kv.delete(agent_id, owner_id, user_id, key)
    }

    // -----------------------------------------------------------------
    // Session operations
    // -----------------------------------------------------------------

    /// Get a session by ID.
    pub fn get_session(&self, session_id: SessionId) -> CarrierResult<Option<Session>> {
        self.sessions.get_session(session_id)
    }

    /// Async wrapper for get_session — runs SQLite query in a blocking thread.
    pub async fn get_session_async(&self, session_id: SessionId) -> CarrierResult<Option<Session>> {
        let sessions = self.sessions.clone();
        tokio::task::spawn_blocking(move || sessions.get_session(session_id))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Save a session.
    pub fn save_session(&self, session: &Session) -> CarrierResult<()> {
        self.sessions.save_session(session)
    }

    /// Save a session asynchronously — runs the SQLite write in a blocking
    /// thread so the tokio runtime stays responsive.
    pub async fn save_session_async(&self, session: &Session) -> CarrierResult<()> {
        let sessions = self.sessions.clone();
        let session = session.clone();
        tokio::task::spawn_blocking(move || sessions.save_session(&session))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Append new messages to a session (concurrency-safe).
    ///
    /// Acquires a per-session write lock, loads current state, appends
    /// messages, and saves. Safe for concurrent agent loops.
    ///
    /// If `turn_summaries` is provided, it replaces the existing summaries.
    pub async fn save_session_append_async(
        &self,
        session_id: SessionId,
        agent_id: &str,
        new_messages: &[Message],
        context_window_tokens: u64,
        label: Option<&str>,
        turn_summaries: Option<&[carrier_types::message::TurnSummary]>,
    ) -> CarrierResult<()> {
        // P1-A observational bypass: every durable surface append is also an
        // event. Written BEFORE the DB save so the log exists even if the
        // save later fails (log is the more durable copy by design). Errors
        // are logged, never propagated — the event log must not break the
        // turn path while the sessions table is still authoritative.
        self.session_events_append(
            agent_id,
            &session_id.0.to_string(),
            crate::session_events::message_events(new_messages),
        );
        self.sessions
            .save_session_append(
                session_id,
                agent_id,
                new_messages,
                context_window_tokens,
                label,
                turn_summaries,
            )
            .await
    }

    /// Append events to a session's event log (observational, best-effort —
    /// see [`crate::session_events`]). Failures are warn-logged, not
    /// propagated.
    pub fn session_events_append(
        &self,
        agent_id: &str,
        session_id: &str,
        kinds: Vec<crate::session_events::SessionEventKind>,
    ) {
        if kinds.is_empty() {
            return;
        }
        if let Err(e) = self.session_events.append(agent_id, session_id, &kinds) {
            tracing::warn!(
                agent = agent_id,
                session = session_id,
                error = %e,
                "session event append failed (observational log; turn continues)"
            );
        }
    }

    /// Read a session's event log (fold input — see P1-B).
    pub fn session_events_read(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> CarrierResult<Vec<crate::session_events::SessionEvent>> {
        self.session_events.read(agent_id, session_id)
    }

    /// Fold a session's durable surface from the event log (P1-C load path:
    /// compaction summaries honored, tool blocks placeholder-rendered).
    pub fn session_events_fold(
        &self,
        agent_id: &str,
        session_id: &str,
    ) -> CarrierResult<Vec<Message>> {
        Ok(crate::session_events::fold_surface(
            &self.session_events.read(agent_id, session_id)?,
        ))
    }

    /// Create a new empty session for an agent.
    pub fn create_session(&self, agent_id: String) -> CarrierResult<Session> {
        self.sessions.create_session(agent_id)
    }

    /// Async wrapper for create_session — runs SQLite query in a blocking thread.
    pub async fn create_session_async(&self, agent_id: String) -> CarrierResult<Session> {
        let sessions = self.sessions.clone();
        tokio::task::spawn_blocking(move || sessions.create_session(agent_id))
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// List all sessions with metadata.
    pub fn list_sessions(&self) -> CarrierResult<Vec<serde_json::Value>> {
        self.sessions.list_sessions()
    }

    /// List all users (by label) for a given agent, with session stats.
    pub fn list_agent_users(&self, agent_id: &str) -> CarrierResult<Vec<serde_json::Value>> {
        self.sessions.list_agent_users(agent_id)
    }

    /// Load all sessions + messages for a given agent + sender_id.
    pub fn list_user_sessions(
        &self,
        agent_id: &str,
        sender_id: &str,
    ) -> CarrierResult<Vec<(String, Vec<carrier_types::message::Message>)>> {
        self.sessions.list_user_sessions(agent_id, sender_id)
    }

    /// Delete a session by ID.
    pub fn delete_session(&self, session_id: SessionId) -> CarrierResult<()> {
        self.sessions.delete_session(session_id)
    }

    /// Delete all sessions belonging to an agent.
    pub fn delete_agent_sessions(&self, agent_id: &str) -> CarrierResult<()> {
        self.sessions.delete_agent_sessions(agent_id)
    }

    /// Set or clear a session label.
    pub fn set_session_label(
        &self,
        session_id: SessionId,
        label: Option<&str>,
    ) -> CarrierResult<()> {
        self.sessions.set_session_label(session_id, label)
    }

    /// Find a session by label for a given agent.
    pub fn find_session_by_label(
        &self,
        agent_id: &str,
        label: &str,
    ) -> CarrierResult<Option<Session>> {
        self.sessions.find_session_by_label(agent_id, label)
    }

    /// Async wrapper for find_active_session_by_label (staleness-windowed).
    pub async fn find_active_session_by_label_async(
        &self,
        agent_id: &str,
        label: &str,
        stale_secs: i64,
    ) -> CarrierResult<Option<Session>> {
        let sessions = self.sessions.clone();
        let agent_id = agent_id.to_string();
        let label = label.to_string();
        tokio::task::spawn_blocking(move || {
            sessions.find_active_session_by_label(&agent_id, &label, stale_secs)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// List all sessions for a specific agent.
    pub fn list_agent_sessions(&self, agent_id: &str) -> CarrierResult<Vec<serde_json::Value>> {
        self.sessions.list_agent_sessions(agent_id)
    }

    /// Create a new session with an optional label.
    pub fn create_session_with_label(
        &self,
        agent_id: String,
        label: Option<&str>,
    ) -> CarrierResult<Session> {
        self.sessions.create_session_with_label(agent_id, label)
    }

    /// Write a human-readable JSONL mirror of a session to disk.
    pub fn write_jsonl_mirror(
        &self,
        session: &Session,
        sessions_dir: &Path,
        owner_id: Option<&str>,
        sender_id: Option<&str>,
        home_dir: Option<&Path>,
        agent_name: Option<&str>,
    ) -> Result<(), std::io::Error> {
        self.sessions.write_jsonl_mirror(
            session,
            sessions_dir,
            owner_id,
            sender_id,
            home_dir,
            agent_name,
        )
    }

    // -----------------------------------------------------------------
    // Tree memory operations
    // -----------------------------------------------------------------

    /// Ingest messages into the tree memory system.
    pub fn tree_ingest(&self, req: &IngestRequest) -> CarrierResult<IngestResult> {
        let pipeline = IngestPipeline::new(Arc::clone(&self.conn), self.content_root.clone());
        pipeline.ingest(req)
    }

    /// Async wrapper for tree_ingest (runs in blocking thread).
    pub async fn tree_ingest_async(&self, req: IngestRequest) -> CarrierResult<IngestResult> {
        let conn = Arc::clone(&self.conn);
        let content_root = self.content_root.clone();
        tokio::task::spawn_blocking(move || {
            let pipeline = IngestPipeline::new(conn, content_root);
            pipeline.ingest(&req)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Query source tree summaries.
    pub fn tree_query_source(&self, req: &SourceQuery<'_>) -> CarrierResult<QueryResponse> {
        let source_kind = req.source_kind.and_then(|k| match k {
            "chat" => Some(SourceKind::Chat),
            "email" => Some(SourceKind::Email),
            "document" => Some(SourceKind::Document),
            _ => None,
        });
        retrieval::source::query_source(
            &self.conn,
            req.owner_id,
            req.user_id,
            req.source_id,
            source_kind,
            req.time_window_days,
            req.limit,
        )
    }

    /// Query global tree summaries.
    pub fn tree_query_global(&self, req: &GlobalQuery<'_>) -> CarrierResult<QueryResponse> {
        retrieval::global::query_global(&self.conn, req.owner_id, req.time_window_days, req.limit)
    }

    /// Query topic tree by entity.
    pub fn tree_query_topic(&self, req: &TopicQuery<'_>) -> CarrierResult<QueryResponse> {
        retrieval::topic::query_topic(
            &self.conn,
            req.owner_id,
            req.user_id,
            req.entity_id,
            req.time_window_days,
            req.limit,
        )
    }

    /// Search entities by substring.
    pub fn tree_search_entities(&self, req: &EntitySearch<'_>) -> CarrierResult<Vec<EntityMatch>> {
        let kind = req
            .kind
            .map(crate::tree::entity_store::EntityStore::parse_entity_kind);
        retrieval::search::search_entities(
            &self.conn,
            req.owner_id,
            req.user_id,
            req.query,
            kind,
            req.limit,
        )
    }

    /// Drill down from a summary node to its children.
    pub fn tree_drill_down(&self, req: &DrillDownQuery<'_>) -> CarrierResult<QueryResponse> {
        let hits = retrieval::drill_down::drill_down(
            &self.conn,
            req.owner_id,
            req.user_id,
            req.node_id,
            req.max_depth.clamp(1, 3),
            Some(req.limit),
        )?;
        let total = hits.len();
        let truncated = total > req.limit;
        Ok(QueryResponse {
            hits,
            total,
            truncated,
        })
    }

    /// Fetch leaf chunks by their IDs directly.
    pub fn tree_fetch_leaves(&self, req: &FetchLeavesQuery<'_>) -> CarrierResult<QueryResponse> {
        retrieval::fetch::fetch_leaves(
            &self.conn,
            req.owner_id,
            req.user_id,
            &req.chunk_ids,
            req.limit,
        )
    }

    /// List all source trees for an owner.
    pub fn tree_list_sources(
        &self,
        owner_id: &str,
        source_kind: Option<&str>,
        limit: usize,
    ) -> CarrierResult<Vec<TreeSummary>> {
        use carrier_types::memory_tree::TreeKind;
        let tree_store = crate::tree::tree_store::TreeTreeStore::new(Arc::clone(&self.conn));
        // Owner-level listing (no user filter) — metadata only, no conversation content.
        let mut trees = tree_store.list_trees(owner_id, None, Some(TreeKind::Source), limit)?;
        if let Some(sk) = source_kind {
            trees.retain(|t| t.scope.starts_with(&format!("{sk}:")));
        }
        Ok(trees)
    }

    /// Async wrapper for tree_query_source (runs in blocking thread).
    pub async fn tree_query_source_async(
        &self,
        req: SourceQuery<'_>,
    ) -> CarrierResult<QueryResponse> {
        let conn = Arc::clone(&self.conn);
        let owner_id = req.owner_id.to_string();
        let user_id = req.user_id.map(String::from);
        let source_id = req.source_id.map(String::from);
        let source_kind = req.source_kind.map(String::from);
        let time_window_days = req.time_window_days;
        let _query = req.query.map(String::from);
        let limit = req.limit;
        tokio::task::spawn_blocking(move || {
            let source_kind_ref = source_kind.as_deref();
            let source_kind_val = source_kind_ref.and_then(|k| match k {
                "chat" => Some(SourceKind::Chat),
                "email" => Some(SourceKind::Email),
                "document" => Some(SourceKind::Document),
                _ => None,
            });
            retrieval::source::query_source(
                &conn,
                &owner_id,
                user_id.as_deref(),
                source_id.as_deref(),
                source_kind_val,
                time_window_days,
                limit,
            )
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Async wrapper for tree_query_global (runs in blocking thread).
    pub async fn tree_query_global_async(
        &self,
        req: GlobalQuery<'_>,
    ) -> CarrierResult<QueryResponse> {
        let conn = Arc::clone(&self.conn);
        let owner_id = req.owner_id.to_string();
        let time_window_days = req.time_window_days;
        let _query = req.query.map(String::from);
        let limit = req.limit;
        tokio::task::spawn_blocking(move || {
            retrieval::global::query_global(&conn, &owner_id, time_window_days, limit)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Async wrapper for tree_query_topic (runs in blocking thread).
    pub async fn tree_query_topic_async(
        &self,
        req: TopicQuery<'_>,
    ) -> CarrierResult<QueryResponse> {
        let conn = Arc::clone(&self.conn);
        let owner_id = req.owner_id.to_string();
        let user_id = req.user_id.map(String::from);
        let entity_id = req.entity_id.to_string();
        let _query = req.query.map(String::from);
        let time_window_days = req.time_window_days;
        let limit = req.limit;
        tokio::task::spawn_blocking(move || {
            retrieval::topic::query_topic(
                &conn,
                &owner_id,
                user_id.as_deref(),
                &entity_id,
                time_window_days,
                limit,
            )
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Async wrapper for tree_search_entities (runs in blocking thread).
    pub async fn tree_search_entities_async(
        &self,
        req: EntitySearch<'_>,
    ) -> CarrierResult<Vec<EntityMatch>> {
        let conn = Arc::clone(&self.conn);
        let owner_id = req.owner_id.to_string();
        let user_id = req.user_id.map(String::from);
        let query = req.query.to_string();
        let kind = req.kind.map(String::from);
        let limit = req.limit;
        tokio::task::spawn_blocking(move || {
            let parsed_kind = kind
                .as_deref()
                .map(crate::tree::entity_store::EntityStore::parse_entity_kind);
            retrieval::search::search_entities(
                &conn,
                &owner_id,
                user_id.as_deref(),
                &query,
                parsed_kind,
                limit,
            )
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Async wrapper for tree_drill_down (runs in blocking thread).
    pub async fn tree_drill_down_async(
        &self,
        req: DrillDownQuery<'_>,
    ) -> CarrierResult<QueryResponse> {
        let conn = Arc::clone(&self.conn);
        let owner_id = req.owner_id.to_string();
        let user_id = req.user_id.map(String::from);
        let node_id = req.node_id.to_string();
        let max_depth = req.max_depth.clamp(1, 3);
        let limit = req.limit;
        tokio::task::spawn_blocking(move || {
            let hits = retrieval::drill_down::drill_down(
                &conn,
                &owner_id,
                user_id.as_deref(),
                &node_id,
                max_depth,
                Some(limit),
            )?;
            let total = hits.len();
            let truncated = total > limit;
            Ok(QueryResponse {
                hits,
                total,
                truncated,
            })
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Async wrapper for tree_fetch_leaves (runs in blocking thread).
    pub async fn tree_fetch_leaves_async(
        &self,
        req: FetchLeavesQuery<'_>,
    ) -> CarrierResult<QueryResponse> {
        let conn = Arc::clone(&self.conn);
        let owner_id = req.owner_id.to_string();
        let user_id = req.user_id.map(String::from);
        let chunk_ids = req.chunk_ids.clone();
        let limit = req.limit;
        tokio::task::spawn_blocking(move || {
            retrieval::fetch::fetch_leaves(&conn, &owner_id, user_id.as_deref(), &chunk_ids, limit)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Async wrapper for tree_list_sources (runs in blocking thread).
    pub async fn tree_list_sources_async(
        &self,
        owner_id: &str,
        source_kind: Option<&str>,
        limit: usize,
    ) -> CarrierResult<Vec<TreeSummary>> {
        let conn = Arc::clone(&self.conn);
        let owner_id = owner_id.to_string();
        let source_kind = source_kind.map(String::from);
        tokio::task::spawn_blocking(move || {
            use carrier_types::memory_tree::TreeKind;
            let tree_store = crate::tree::tree_store::TreeTreeStore::new(Arc::clone(&conn));
            // Owner-level listing (no user filter) — metadata only.
            let mut trees =
                tree_store.list_trees(&owner_id, None, Some(TreeKind::Source), limit)?;
            if let Some(ref sk) = source_kind {
                trees.retain(|t| t.scope.starts_with(&format!("{sk}:")));
            }
            Ok(trees)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    // -----------------------------------------------------------------
    // Task queue operations
    // -----------------------------------------------------------------

    /// Post a new task to the shared queue. Returns the task ID.
    pub async fn task_post(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
    ) -> CarrierResult<String> {
        let conn = Arc::clone(&self.conn);
        let title = title.to_string();
        let description = description.to_string();
        let assigned_to = assigned_to.unwrap_or("").to_string();
        let created_by = created_by.unwrap_or("").to_string();

        tokio::task::spawn_blocking(move || {
            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            let db = conn.lock().map_err(|e| CarrierError::Internal(e.to_string()))?;
            db.execute(
                "INSERT INTO task_queue (id, agent_id, task_type, payload, status, priority, created_at, title, description, assigned_to, created_by)
                 VALUES (?1, ?2, ?3, ?4, 'pending', 0, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![id, &created_by, &title, b"", now, title, description, assigned_to, created_by],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
            Ok(id)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Claim the next pending task. Returns task JSON or None.
    pub async fn task_claim(&self, agent_id: &str) -> CarrierResult<Option<serde_json::Value>> {
        let conn = Arc::clone(&self.conn);
        let agent_id = agent_id.to_string();

        tokio::task::spawn_blocking(move || {
            let db = conn.lock().map_err(|e| CarrierError::Internal(e.to_string()))?;

            let sql = "SELECT id, title, description, assigned_to, created_by, created_at
                     FROM task_queue
                     WHERE status = 'pending' AND (assigned_to = ?1 OR assigned_to = '')
                     ORDER BY priority DESC, created_at ASC
                     LIMIT 1";

            let mut stmt = db.prepare(sql).map_err(|e| CarrierError::Memory(e.to_string()))?;

            let result = stmt.query_row(rusqlite::params![agent_id.clone()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            });

            match result {
                Ok((id, title, description, assigned, created_by, created_at)) => {
                    db.execute(
                        "UPDATE task_queue SET status = 'in_progress', assigned_to = ?2 WHERE id = ?1",
                        rusqlite::params![id, agent_id],
                    ).map_err(|e| CarrierError::Memory(e.to_string()))?;

                    Ok(Some(serde_json::json!({
                        "id": id,
                        "title": title,
                        "description": description,
                        "status": "in_progress",
                        "assigned_to": if assigned.is_empty() { &agent_id } else { &assigned },
                        "created_by": created_by,
                        "created_at": created_at,
                    })))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(CarrierError::Memory(e.to_string())),
            }
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// Mark a task as completed with a result string.
    pub async fn task_complete(&self, task_id: &str, result: &str) -> CarrierResult<()> {
        let conn = Arc::clone(&self.conn);
        let task_id = task_id.to_string();
        let result = result.to_string();

        tokio::task::spawn_blocking(move || {
            let now = chrono::Utc::now().to_rfc3339();
            let db = conn.lock().map_err(|e| CarrierError::Internal(e.to_string()))?;
            let rows = db.execute(
                "UPDATE task_queue SET status = 'completed', result = ?2, completed_at = ?3 WHERE id = ?1",
                rusqlite::params![task_id, result, now],
            ).map_err(|e| CarrierError::Memory(e.to_string()))?;
            if rows == 0 {
                return Err(CarrierError::Internal(format!("Task not found: {task_id}")));
            }
            Ok(())
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }

    /// List tasks, optionally filtered by status.
    pub async fn task_list(&self, status: Option<&str>) -> CarrierResult<Vec<serde_json::Value>> {
        let conn = Arc::clone(&self.conn);
        let status = status.map(|s| s.to_string());

        tokio::task::spawn_blocking(move || {
            let db = conn.lock().map_err(|e| CarrierError::Internal(e.to_string()))?;
            let (sql, params): (&str, Vec<Box<dyn rusqlite::types::ToSql>>) = match &status {
                Some(s) => (
                    "SELECT id, title, description, status, assigned_to, created_by, created_at, completed_at, result FROM task_queue WHERE status = ?1 ORDER BY created_at DESC",
                    vec![Box::new(s.clone())],
                ),
                None => (
                    "SELECT id, title, description, status, assigned_to, created_by, created_at, completed_at, result FROM task_queue ORDER BY created_at DESC",
                    vec![],
                ),
            };

            let mut stmt = db.prepare(sql).map_err(|e| CarrierError::Memory(e.to_string()))?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
            let rows = stmt.query_map(params_refs.as_slice(), |row| {
                Ok(serde_json::json!({
                    "id": row.get::<_, String>(0)?,
                    "title": row.get::<_, String>(1).unwrap_or_default(),
                    "description": row.get::<_, String>(2).unwrap_or_default(),
                    "status": row.get::<_, String>(3)?,
                    "assigned_to": row.get::<_, String>(4).unwrap_or_default(),
                    "created_by": row.get::<_, String>(5).unwrap_or_default(),
                    "created_at": row.get::<_, String>(6).unwrap_or_default(),
                    "completed_at": row.get::<_, Option<String>>(7).unwrap_or(None),
                    "result": row.get::<_, Option<String>>(8).unwrap_or(None),
                }))
            }).map_err(|e| CarrierError::Memory(e.to_string()))?;

            let mut tasks = Vec::new();
            for row in rows {
                tasks.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
            }
            Ok(tasks)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_kv_set_get() {
        let substrate = MemorySubstrate::open_in_memory().unwrap();
        let agent_id_str = "test-agent".to_string();
        substrate
            .system_kv_set(
                &agent_id_str,
                "user1",
                "user1",
                "test_key",
                serde_json::json!("test_value"),
            )
            .unwrap();
        let val = substrate
            .system_kv_get(&agent_id_str, "user1", "user1", "test_key")
            .unwrap();
        assert_eq!(val, Some(serde_json::json!("test_value")));
    }

    #[tokio::test]
    async fn test_task_post_and_list() {
        let substrate = MemorySubstrate::open_in_memory().unwrap();
        let id = substrate
            .task_post(
                "Review code",
                "Check the auth module for issues",
                Some("auditor"),
                Some("orchestrator"),
            )
            .await
            .unwrap();
        assert!(!id.is_empty());

        let tasks = substrate.task_list(Some("pending")).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["title"], "Review code");
        assert_eq!(tasks[0]["assigned_to"], "auditor");
        assert_eq!(tasks[0]["status"], "pending");
    }

    #[tokio::test]
    async fn test_task_claim_and_complete() {
        let substrate = MemorySubstrate::open_in_memory().unwrap();
        let task_id = substrate
            .task_post(
                "Audit endpoint",
                "Security audit the /api/login endpoint",
                Some("auditor"),
                None,
            )
            .await
            .unwrap();

        let claimed = substrate.task_claim("auditor").await.unwrap();
        assert!(claimed.is_some());
        let claimed = claimed.unwrap();
        assert_eq!(claimed["id"], task_id);
        assert_eq!(claimed["status"], "in_progress");

        substrate
            .task_complete(&task_id, "No vulnerabilities found")
            .await
            .unwrap();

        let tasks = substrate.task_list(Some("completed")).await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["result"], "No vulnerabilities found");
    }

    #[tokio::test]
    async fn test_task_claim_empty() {
        let substrate = MemorySubstrate::open_in_memory().unwrap();
        let claimed = substrate.task_claim("nobody").await.unwrap();
        assert!(claimed.is_none());
    }
}
