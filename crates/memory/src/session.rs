//! Session management — load/save conversation history.

use chrono::Utc;
use dashmap::DashMap;
use rusqlite::Connection;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use carrier_types::agent::SessionId;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::message::{ContentBlock, Message, MessageContent, Role, TurnSummary};

/// A conversation session with message history.
#[derive(Debug, Clone)]
pub struct Session {
    /// Session ID.
    pub id: SessionId,
    /// Owning agent name (stable across restarts).
    pub agent_name: String,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Summaries of older turns (L1 context layer).
    pub turn_summaries: Vec<TurnSummary>,
    /// Estimated token count for the context window.
    pub context_window_tokens: u64,
    /// Optional human-readable session label.
    pub label: Option<String>,
}

/// 借用机制的会话票据 — 用户侧持有的会话状态（session 只在用户侧持久）。
///
/// 借用轮（`run_borrowed_turn`）以票据进、以票据出：主人服务器把票据还原成
/// `Session`，在内存级 substrate 上跑一轮，再把更新后的状态打包回票据还给
/// 用户侧。服务器全程零持久化——票据就是会话的唯一真源，由用户自己保管、
/// 下一轮再提交。
///
/// wire 形状 camelCase（`turnSummaries`/`contextWindowTokens`），与协议其余
/// 字段（sessionTicket/activeFlow/contentBase64）同一约定——金样本见 aginx
/// 仓 ACP.md。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTicket {
    /// 票据格式版本（当前 1）。
    pub version: u32,
    /// 可选会话标签（透传到 Session.label）。
    #[serde(default)]
    pub label: Option<String>,
    /// 完整对话历史。
    #[serde(default)]
    pub messages: Vec<Message>,
    /// 旧轮摘要（L1 上下文层）。
    #[serde(default)]
    pub turn_summaries: Vec<TurnSummary>,
    /// 上下文窗口 token 估计（跨轮保真压缩判断）。
    #[serde(default)]
    pub context_window_tokens: u64,
}

impl SessionTicket {
    /// 当前票据版本。
    pub const CURRENT_VERSION: u32 = 1;

    /// 票据消息历史的字节预算。超出时从最老的消息对开始丢弃——更早轮次的
    /// 语义由 `turn_summaries`（L1 摘要层）承载，票据不无限膨胀。
    /// 256KB ≈ 数十轮普通对话；素材/产物走 materials/files 通道，不占这里。
    pub const MAX_MESSAGES_BYTES: usize = 256 * 1024;

    /// 一张空票据（首轮借用）。
    pub fn empty(label: Option<String>) -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            label,
            messages: Vec::new(),
            turn_summaries: Vec::new(),
            context_window_tokens: 0,
        }
    }

    /// 单条消息的粗略字节体积（wire 序列化后同量级）。
    fn message_bytes(m: &Message) -> usize {
        16 + match &m.content {
            MessageContent::Text(s) => s.len(),
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text, .. } => text.len() + 32,
                    _ => 64,
                })
                .sum(),
        }
    }

    /// 把消息历史截到预算内：保留最新的消息，丢最老的完整 user+assistant 对
    /// （不对半劈对话）。被丢的轮次语义已在 `turn_summaries` 里。
    fn truncate_messages(messages: Vec<Message>, budget: usize) -> (Vec<Message>, bool) {
        let total: usize = messages.iter().map(Self::message_bytes).sum();
        if total <= budget {
            return (messages, false);
        }
        // 找最小的偶数前缀（完整轮次），使剩余部分 ≤ 预算。奇数条开头说明
        // 有未配对消息，一并视为可丢前缀的一部分。
        let mut drop_end = 0usize;
        let mut acc = 0usize;
        for (i, m) in messages.iter().enumerate() {
            acc += Self::message_bytes(m);
            if i % 2 == 1 && total - acc <= budget {
                drop_end = i + 1;
                break;
            }
        }
        if drop_end == 0 {
            // 连单对都装不下（极端：单条消息超预算）——只留最新一条。
            let last = messages.len().saturating_sub(2);
            return (messages[last..].to_vec(), true);
        }
        (messages[drop_end..].to_vec(), true)
    }
}

impl Session {
    /// 从票据还原会话（借用轮入口）。每次借用都是一个全新的 session id ——
    /// 服务器不记忆任何会话身份，票据即身份。
    pub fn from_ticket(ticket: SessionTicket, agent_name: String) -> Self {
        Self {
            id: SessionId::new(),
            agent_name,
            messages: ticket.messages,
            turn_summaries: ticket.turn_summaries,
            context_window_tokens: ticket.context_window_tokens,
            label: ticket.label,
        }
    }

    /// 把当前会话状态打包成票据（借用轮出口），交还用户侧持久化。
    ///
    /// 出口处做票据膨胀治理：消息历史截到 `MAX_MESSAGES_BYTES` 内（丢最老
    /// 的完整轮次对），更早轮次的语义由 `turn_summaries` 承载——摘要层保留
    /// 全量，原文层滚动窗口。
    pub fn to_ticket(&self) -> SessionTicket {
        let (messages, truncated) =
            SessionTicket::truncate_messages(self.messages.clone(), SessionTicket::MAX_MESSAGES_BYTES);
        if truncated {
            tracing::debug!(
                agent = %self.agent_name,
                kept = messages.len(),
                total = self.messages.len(),
                "ticket messages truncated to budget"
            );
        }
        SessionTicket {
            version: SessionTicket::CURRENT_VERSION,
            label: self.label.clone(),
            messages,
            turn_summaries: self.turn_summaries.clone(),
            context_window_tokens: self.context_window_tokens,
        }
    }
}

/// Session store backed by SQLite.
#[derive(Clone)]
pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
    /// Per-session write locks for concurrency-safe append operations.
    session_locks: Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl SessionStore {
    /// Create a new session store wrapping the given connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self {
            conn,
            session_locks: Arc::new(DashMap::new()),
        }
    }

    /// Load a session from the database.
    pub fn get_session(&self, session_id: SessionId) -> CarrierResult<Option<Session>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT agent_id, messages, turn_summaries, context_window_tokens, label FROM sessions WHERE id = ?1")
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let result = stmt.query_row(rusqlite::params![session_id.0.to_string()], |row| {
            let agent_str: String = row.get(0)?;
            let messages_blob: Vec<u8> = row.get(1)?;
            let summaries_blob: Option<Vec<u8>> = row.get(2)?;
            let tokens: i64 = row.get(3)?;
            let label: Option<String> = row.get(4).unwrap_or(None);
            Ok((agent_str, messages_blob, summaries_blob, tokens, label))
        });

        match result {
            Ok((agent_str, messages_blob, summaries_blob, tokens, label)) => {
                let messages: Vec<Message> = rmp_serde::from_slice(&messages_blob)
                    .map_err(|e| CarrierError::Serialization(e.to_string()))?;
                let turn_summaries: Vec<TurnSummary> = match summaries_blob {
                    Some(blob) => match rmp_serde::from_slice(&blob) {
                        Ok(v) => v,
                        // A corrupt/stale summaries blob must NOT break the main
                        // turn path — degrade to empty (L0 summaries rebuild over
                        // upcoming turns) rather than failing message sending.
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "turn_summaries blob could not be deserialized; using empty"
                            );
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                };
                Ok(Some(Session {
                    id: session_id,
                    agent_name: agent_str,
                    messages,
                    turn_summaries,
                    context_window_tokens: tokens as u64,
                    label,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Save a session to the database.
    ///
    /// Strips tool_use/tool_result blocks before persisting — these are
    /// execution details needed only during the current agent loop, not
    /// for future conversation continuity.
    pub fn save_session(&self, session: &Session) -> CarrierResult<()> {
        let clean_messages = strip_tool_history(&session.messages);
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let messages_blob = rmp_serde::to_vec_named(&clean_messages)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let summaries_blob = rmp_serde::to_vec_named(&session.turn_summaries)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sessions (id, agent_id, messages, turn_summaries, context_window_tokens, label, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET messages = ?3, turn_summaries = ?4, context_window_tokens = ?5, label = ?6, updated_at = ?7",
            rusqlite::params![
                session.id.0.to_string(),
                &session.agent_name,
                messages_blob,
                summaries_blob,
                session.context_window_tokens as i64,
                session.label.as_deref(),
                now,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Append messages to a session (concurrency-safe).
    ///
    /// Acquires a per-session write lock, loads current state from DB,
    /// appends new messages, and saves back. This allows multiple agent
    /// loops to run in parallel for the same agent — each appends its
    /// own new messages without overwriting the other's.
    ///
    /// If `turn_summaries` is provided, it replaces the existing summaries
    /// (used when the agent loop has generated new turn summaries).
    pub async fn save_session_append(
        &self,
        session_id: SessionId,
        agent_id: &str,
        new_messages: &[Message],
        context_window_tokens: u64,
        label: Option<&str>,
        turn_summaries: Option<&[TurnSummary]>,
    ) -> CarrierResult<()> {
        let key = session_id.0.to_string();
        let lock = self
            .session_locks
            .entry(key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        // Clone what we need for the blocking closure
        let store = self.clone();
        let agent_id = agent_id.to_string();
        let new_messages = new_messages.to_vec();
        let label = label.map(String::from);
        let turn_summaries = turn_summaries.map(|s| s.to_vec());

        tokio::task::spawn_blocking(move || {
            let mut session = match store.get_session(session_id)? {
                Some(s) => s,
                None => Session {
                    id: session_id,
                    agent_name: agent_id,
                    messages: Vec::new(),
                    turn_summaries: Vec::new(),
                    context_window_tokens: 0,
                    label: None,
                },
            };
            session.messages.extend_from_slice(&new_messages);
            session.context_window_tokens = context_window_tokens;
            if let Some(l) = label {
                session.label = Some(l);
            }
            if let Some(summaries) = turn_summaries {
                session.turn_summaries = summaries;
            }
            store.save_session(&session)
        })
        .await
        .map_err(|e| CarrierError::Internal(e.to_string()))??;

        // Clean up lock entry if no one else is waiting
        drop(_guard);
        self.session_locks
            .retain(|k, v| Arc::strong_count(v) > 1 || k != &key);
        Ok(())
    }

    /// Delete a session from the database.
    pub fn delete_session(&self, session_id: SessionId) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM sessions WHERE id = ?1",
            rusqlite::params![session_id.0.to_string()],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Delete all sessions belonging to an agent.
    pub fn delete_agent_sessions(&self, agent_id: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM sessions WHERE agent_id = ?1",
            rusqlite::params![agent_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// List all sessions with metadata (session_id, agent_id, message_count, created_at).
    pub fn list_sessions(&self) -> CarrierResult<Vec<serde_json::Value>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let sql = "SELECT id, agent_id, messages, created_at, label FROM sessions ORDER BY created_at DESC";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let row_data: Vec<rusqlite::Result<serde_json::Value>> = stmt
            .query_map([], Self::session_row_to_json)
            .map_err(|e| CarrierError::Memory(e.to_string()))?
            .collect();

        let mut sessions = Vec::new();
        for row in row_data {
            sessions.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(sessions)
    }

    /// List all users (by label) for a given agent, with session stats.
    ///
    /// Groups sessions by their label (format `user:{sender_id}`), returning
    /// each user's sender_id, session count, and last active timestamp.
    pub fn list_agent_users(&self, agent_id: &str) -> CarrierResult<Vec<serde_json::Value>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let sql = "SELECT label, COUNT(*) as session_count, MAX(created_at) as last_active \
                   FROM sessions \
                   WHERE agent_id = ?1 AND label LIKE 'user:%' \
                   GROUP BY label \
                   ORDER BY last_active DESC";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let rows: Vec<serde_json::Value> = stmt
            .query_map(rusqlite::params![agent_id], |row| {
                let label: String = row.get(0)?;
                let session_count: i64 = row.get(1)?;
                let last_active: String = row.get(2)?;
                Ok((label, session_count, last_active))
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?
            .filter_map(|r| r.ok())
            .map(|(label, session_count, last_active)| {
                let sender_id = label.strip_prefix("user:").unwrap_or(&label).to_string();
                serde_json::json!({
                    "sender_id": sender_id,
                    "session_count": session_count,
                    "last_active": last_active,
                })
            })
            .collect();

        Ok(rows)
    }

    /// Helper to map a session row to JSON.
    fn session_row_to_json(row: &rusqlite::Row) -> rusqlite::Result<serde_json::Value> {
        let session_id: String = row.get(0)?;
        let agent_id: String = row.get(1)?;
        let messages_blob: Vec<u8> = row.get(2)?;
        let created_at: String = row.get(3)?;
        let label: Option<String> = row.get(4)?;
        let msg_count = rmp_serde::from_slice::<Vec<Message>>(&messages_blob)
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(serde_json::json!({
            "session_id": session_id,
            "agent_id": agent_id,
            "message_count": msg_count,
            "created_at": created_at,
            "label": label,
        }))
    }

    /// Create a new empty session for an agent.
    pub fn create_session(&self, agent_id: String) -> CarrierResult<Session> {
        let session = Session {
            id: SessionId::new(),
            agent_name: agent_id,
            messages: Vec::new(),
            turn_summaries: Vec::new(),
            context_window_tokens: 0,
            label: None,
        };
        self.save_session(&session)?;
        Ok(session)
    }

    /// Set the label on an existing session.
    pub fn set_session_label(
        &self,
        session_id: SessionId,
        label: Option<&str>,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "UPDATE sessions SET label = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![label, Utc::now().to_rfc3339(), session_id.0.to_string()],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Find a session by label for a given agent.
    /// Find the most recent session with this label that is still within the
    /// staleness window (updated_at newer than `stale_secs` ago). Returns None
    /// if the labeled session is older than the window — the caller starts a
    /// fresh session; the stale one stays archived-in-place (same label, but
    /// its old updated_at keeps it out of the active window). This is how
    /// sessions auto-split by inactivity: a sender returning after the window
    /// gets a clean session instead of dragging in stale cross-task context.
    pub fn find_active_session_by_label(
        &self,
        agent_id: &str,
        label: &str,
        stale_secs: i64,
    ) -> CarrierResult<Option<Session>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                // julianday() (not a string compare) because updated_at is
                // stored as rfc3339 ('2026-07-17T15:29:32+00:00') while
                // datetime('now') uses a space separator — a naive string
                // compare would mis-judge same-day rows (T > space).
                "SELECT id, messages, turn_summaries, context_window_tokens, label FROM sessions \
                 WHERE agent_id = ?1 AND label = ?2 \
                 AND julianday(updated_at) > julianday('now') - (?3 / 86400.0) \
                 ORDER BY updated_at DESC LIMIT 1",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let result = stmt.query_row(rusqlite::params![agent_id, label, stale_secs], |row| {
            let id_str: String = row.get(0)?;
            let messages_blob: Vec<u8> = row.get(1)?;
            let summaries_blob: Option<Vec<u8>> = row.get(2)?;
            let tokens: i64 = row.get(3)?;
            let lbl: Option<String> = row.get(4).unwrap_or(None);
            Ok((id_str, messages_blob, summaries_blob, tokens, lbl))
        });
        match result {
            Ok((id_str, messages_blob, summaries_blob, tokens, lbl)) => {
                let session_id = uuid::Uuid::parse_str(&id_str)
                    .map(SessionId)
                    .map_err(|e| CarrierError::Memory(e.to_string()))?;
                let messages: Vec<Message> = rmp_serde::from_slice(&messages_blob)
                    .map_err(|e| CarrierError::Serialization(e.to_string()))?;
                let turn_summaries: Vec<TurnSummary> = match summaries_blob {
                    Some(blob) => match rmp_serde::from_slice(&blob) {
                        Ok(v) => v,
                        // A corrupt/stale summaries blob must NOT break the main
                        // turn path — degrade to empty (L0 summaries rebuild over
                        // upcoming turns) rather than failing message sending.
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "turn_summaries blob could not be deserialized; using empty"
                            );
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                };
                Ok(Some(Session {
                    id: session_id,
                    agent_name: agent_id.to_string(),
                    messages,
                    turn_summaries,
                    context_window_tokens: tokens as u64,
                    label: lbl,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    pub fn find_session_by_label(
        &self,
        agent_id: &str,
        label: &str,
    ) -> CarrierResult<Option<Session>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, messages, turn_summaries, context_window_tokens, label FROM sessions \
                 WHERE agent_id = ?1 AND label = ?2 LIMIT 1",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let result = stmt.query_row(rusqlite::params![agent_id, label], |row| {
            let id_str: String = row.get(0)?;
            let messages_blob: Vec<u8> = row.get(1)?;
            let summaries_blob: Option<Vec<u8>> = row.get(2)?;
            let tokens: i64 = row.get(3)?;
            let lbl: Option<String> = row.get(4).unwrap_or(None);
            Ok((id_str, messages_blob, summaries_blob, tokens, lbl))
        });

        match result {
            Ok((id_str, messages_blob, summaries_blob, tokens, lbl)) => {
                let session_id = uuid::Uuid::parse_str(&id_str)
                    .map(SessionId)
                    .map_err(|e| CarrierError::Memory(e.to_string()))?;
                let messages: Vec<Message> = rmp_serde::from_slice(&messages_blob)
                    .map_err(|e| CarrierError::Serialization(e.to_string()))?;
                let turn_summaries: Vec<TurnSummary> = match summaries_blob {
                    Some(blob) => match rmp_serde::from_slice(&blob) {
                        Ok(v) => v,
                        // A corrupt/stale summaries blob must NOT break the main
                        // turn path — degrade to empty (L0 summaries rebuild over
                        // upcoming turns) rather than failing message sending.
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "turn_summaries blob could not be deserialized; using empty"
                            );
                            Vec::new()
                        }
                    },
                    None => Vec::new(),
                };
                Ok(Some(Session {
                    id: session_id,
                    agent_name: agent_id.to_string(),
                    messages,
                    turn_summaries,
                    context_window_tokens: tokens as u64,
                    label: lbl,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }
}

impl SessionStore {
    /// List all sessions for a specific agent.
    pub fn list_agent_sessions(&self, agent_id: &str) -> CarrierResult<Vec<serde_json::Value>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, messages, created_at, label FROM sessions WHERE agent_id = ?1 ORDER BY created_at DESC",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![agent_id], |row| {
                let session_id: String = row.get(0)?;
                let messages_blob: Vec<u8> = row.get(1)?;
                let created_at: String = row.get(2)?;
                let label: Option<String> = row.get(3)?;
                let msg_count = rmp_serde::from_slice::<Vec<Message>>(&messages_blob)
                    .map(|m| m.len())
                    .unwrap_or(0);
                Ok(serde_json::json!({
                    "session_id": session_id,
                    "message_count": msg_count,
                    "created_at": created_at,
                    "label": label,
                }))
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(sessions)
    }

    /// Load all sessions + messages for a given agent + sender_id (label = "user:{sender_id}").
    pub fn list_user_sessions(
        &self,
        agent_id: &str,
        sender_id: &str,
    ) -> CarrierResult<Vec<(String, Vec<Message>)>> {
        let label = format!("user:{}", sender_id);
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, messages FROM sessions \
                 WHERE agent_id = ?1 AND label = ?2 \
                 ORDER BY created_at ASC",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![agent_id, label], |row| {
                let session_id: String = row.get(0)?;
                let messages_blob: Vec<u8> = row.get(1)?;
                Ok((session_id, messages_blob))
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            let (session_id, messages_blob) =
                row.map_err(|e| CarrierError::Memory(e.to_string()))?;
            let messages: Vec<Message> = rmp_serde::from_slice(&messages_blob)
                .map_err(|e| CarrierError::Serialization(e.to_string()))?;
            result.push((session_id, messages));
        }
        Ok(result)
    }

    /// Create a new session with an optional label.
    pub fn create_session_with_label(
        &self,
        agent_id: String,
        label: Option<&str>,
    ) -> CarrierResult<Session> {
        let session = Session {
            id: SessionId::new(),
            agent_name: agent_id,
            messages: Vec::new(),
            turn_summaries: Vec::new(),
            context_window_tokens: 0,
            label: label.map(|s| s.to_string()),
        };
        self.save_session(&session)?;
        Ok(session)
    }

    /// Count users with sessions updated in the last N days.
    pub fn count_active_users(&self, agent_id: &str, days: u32) -> CarrierResult<u32> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let sql = format!(
            "SELECT COUNT(DISTINCT label) FROM sessions \
             WHERE agent_id = ?1 AND label LIKE 'user:%' \
             AND updated_at > datetime('now', '-{} days')",
            days
        );
        let count: u32 = conn
            .query_row(&sql, rusqlite::params![agent_id], |row| row.get(0))
            .unwrap_or(0);
        Ok(count)
    }

    /// Count users whose first session was created in the last N days.
    pub fn count_new_users(&self, agent_id: &str, days: u32) -> CarrierResult<u32> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let sql = format!(
            "SELECT COUNT(*) FROM (\
               SELECT label FROM sessions \
               WHERE agent_id = ?1 AND label LIKE 'user:%' \
               GROUP BY label \
               HAVING MIN(created_at) > datetime('now', '-{} days')\
             )",
            days
        );
        let count: u32 = conn
            .query_row(&sql, rusqlite::params![agent_id], |row| row.get(0))
            .unwrap_or(0);
        Ok(count)
    }

    /// Return the N most recent sessions with metadata (no msgpack blob).
    pub fn recent_sessions(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> CarrierResult<Vec<serde_json::Value>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, label, created_at, updated_at, context_window_tokens \
                 FROM sessions \
                 WHERE agent_id = ?1 \
                 ORDER BY updated_at DESC LIMIT ?2",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![agent_id, limit], |row| {
                let id: String = row.get(0)?;
                let label: Option<String> = row.get(1)?;
                let created_at: String = row.get(2)?;
                let updated_at: String = row.get(3)?;
                let tokens: i64 = row.get(4)?;
                Ok((id, label, created_at, updated_at, tokens))
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut sessions = Vec::new();
        for row in rows {
            let (id, label, created_at, updated_at, tokens) =
                row.map_err(|e| CarrierError::Memory(e.to_string()))?;
            sessions.push(serde_json::json!({
                "session_id": id,
                "label": label,
                "created_at": created_at,
                "updated_at": updated_at,
                "tokens": tokens,
            }));
        }
        Ok(sessions)
    }
}

/// A single JSONL line in the session mirror file.
#[derive(serde::Serialize)]
struct JsonlLine {
    timestamp: String,
    role: String,
    content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_use: Option<serde_json::Value>,
}

impl SessionStore {
    /// Write a human-readable JSONL mirror of a session to disk.
    ///
    /// **Append-only**: reads the existing file to find how many lines are
    /// already written, then appends only the new messages. Never truncates
    /// or rewrites existing lines — conversation history is immutable.
    pub fn write_jsonl_mirror(
        &self,
        session: &Session,
        sessions_dir: &Path,
        owner_id: Option<&str>,
        sender_id: Option<&str>,
        home_dir: Option<&Path>,
        agent_name: Option<&str>,
    ) -> Result<(), std::io::Error> {
        // Route to per-sender sessions directory when sender_id is present
        let effective_dir = if let (Some(oid), Some(hd), Some(an)) =
            (owner_id.or(sender_id), home_dir, agent_name)
        {
            let user_dir = carrier_types::config::sender_data_dir(hd, oid, an, sender_id).join("sessions");
            std::fs::create_dir_all(&user_dir)?;
            user_dir
        } else {
            std::fs::create_dir_all(sessions_dir)?;
            sessions_dir.to_path_buf()
        };
        let path = effective_dir.join(format!("{}.jsonl", session.id.0));

        // Count existing lines to find what's already written
        let existing_lines = if path.exists() {
            std::io::BufRead::lines(std::io::BufReader::new(std::fs::File::open(&path)?)).count()
        } else {
            0
        };

        // Only append new messages (those beyond what's already written)
        let new_messages = if session.messages.len() > existing_lines {
            &session.messages[existing_lines..]
        } else {
            return Ok(()); // Nothing new to append
        };

        if new_messages.is_empty() {
            return Ok(());
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        let now = Utc::now().to_rfc3339();

        for msg in new_messages {
            let role_str = match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
            };

            let mut text_parts: Vec<String> = Vec::new();
            let mut tool_parts: Vec<serde_json::Value> = Vec::new();

            match &msg.content {
                MessageContent::Text(t) => {
                    text_parts.push(t.clone());
                }
                MessageContent::Blocks(blocks) => {
                    for block in blocks {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                text_parts.push(text.clone());
                            }
                            ContentBlock::ToolUse {
                                id, name, input, ..
                            } => {
                                tool_parts.push(serde_json::json!({
                                    "type": "tool_use",
                                    "id": id,
                                    "name": name,
                                    "input": input,
                                }));
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                tool_name: _,
                                content,
                                is_error,
                            } => {
                                tool_parts.push(serde_json::json!({
                                    "type": "tool_result",
                                    "tool_use_id": tool_use_id,
                                    "content": content,
                                    "is_error": is_error,
                                }));
                            }
                            ContentBlock::Image { media_type, .. } => {
                                text_parts.push(format!("[image: {media_type}]"));
                            }
                            ContentBlock::Audio { media_type, .. } => {
                                text_parts.push(format!("[audio: {media_type}]"));
                            }
                            ContentBlock::Thinking { thinking } => {
                                text_parts.push(format!(
                                    "[thinking: {}]",
                                    carrier_types::truncate_str(thinking, 200)
                                ));
                            }
                            ContentBlock::Unknown => {}
                        }
                    }
                }
            }

            let line = JsonlLine {
                timestamp: now.clone(),
                role: role_str.to_string(),
                content: serde_json::Value::String(text_parts.join("\n")),
                tool_use: if tool_parts.is_empty() {
                    None
                } else {
                    Some(serde_json::Value::Array(tool_parts))
                },
            };

            serde_json::to_writer(&mut file, &line).map_err(std::io::Error::other)?;
            file.write_all(b"\n")?;
        }

        Ok(())
    }
}

/// Strip tool_use/tool_result blocks from messages before persisting to DB.
///
/// The purpose of the session is to maintain conversational continuity
/// (what the user asked, what the assistant responded). Tool calls and
/// results are execution ephemera — needed during the current agent loop
/// but useless noise for future turns. Stripping them prevents context
/// bloat (especially from large tool results like base64 image data).
///
/// After stripping:
/// - User messages: kept as-is
/// - Assistant messages: keep only text/thinking blocks, drop tool_use
/// - Messages that become empty after stripping are removed entirely
pub(crate) fn strip_tool_history(messages: &[Message]) -> Vec<Message> {
    let mut clean = Vec::with_capacity(messages.len());
    for msg in messages {
        match &msg.content {
            MessageContent::Text(_) => {
                // Plain text messages always kept
                clean.push(msg.clone());
            }
            MessageContent::Blocks(blocks) => {
                let mut has_tool_use = false;
                let mut kept_blocks: Vec<ContentBlock> = Vec::new();
                for block in blocks {
                    match block {
                        ContentBlock::ToolUse { name, input, .. } => {
                            has_tool_use = true;
                            // Replace tool_use with a brief summary line
                            let summary = format!("[Called {name}]");
                            kept_blocks.push(ContentBlock::Text {
                                text: summary,
                                provider_metadata: None,
                            });
                            // Log input size for debugging (not persisted)
                            let _ = input;
                        }
                        ContentBlock::ToolResult {
                            tool_name,
                            is_error,
                            ..
                        } => {
                            // Replace tool_result with a brief placeholder
                            let marker = if *is_error { " (error)" } else { "" };
                            let summary = format!("[Result from {tool_name}{marker}]");
                            kept_blocks.push(ContentBlock::Text {
                                text: summary,
                                provider_metadata: None,
                            });
                        }
                        ContentBlock::Image { .. } | ContentBlock::Audio { .. } => {
                            // Drop inline media — too large for persistence
                        }
                        other => {
                            kept_blocks.push(other.clone());
                        }
                    }
                }

                if kept_blocks.is_empty() && !has_tool_use {
                    // Message with only images/audio — skip entirely
                    continue;
                }

                // If all blocks were tool-use related but we have summaries, keep them
                // If the message had text alongside tool_use, the text is preserved
                if !kept_blocks.is_empty() {
                    clean.push(Message {
                        role: msg.role,
                        content: MessageContent::Blocks(kept_blocks),
                    });
                }
            }
        }
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> SessionStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        SessionStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_create_and_load_session() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        let session = store.create_session(agent_id.clone()).unwrap();

        let loaded = store.get_session(session.id).unwrap().unwrap();
        assert_eq!(loaded.agent_name, agent_id);
        assert!(loaded.messages.is_empty());
    }

    #[test]
    fn test_find_active_session_respects_staleness_window() {
        let store = setup();
        let agent = "test-agent";
        let label = "user:alice";
        let session = store
            .create_session_with_label(agent.to_string(), Some(label))
            .unwrap();
        let sid = session.id.0.to_string();

        // Fresh session (updated_at = now) is within a 1h window -> found.
        assert!(store
            .find_active_session_by_label(agent, label, 3600)
            .unwrap()
            .is_some());

        // Push updated_at far into the past (rfc3339 with timezone — must be
        // parsed correctly by julianday, not a naive string compare).
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params!["2020-01-01T00:00:00+00:00", sid],
            )
            .unwrap();
        }
        // Stale -> None (caller should start a fresh session).
        assert!(store
            .find_active_session_by_label(agent, label, 3600)
            .unwrap()
            .is_none());

        // 30 min ago (space-separated datetime) is within the 1h window -> found.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE sessions SET updated_at = datetime('now', '-1800 seconds') WHERE id = ?1",
                rusqlite::params![sid],
            )
            .unwrap();
        }
        assert!(store
            .find_active_session_by_label(agent, label, 3600)
            .unwrap()
            .is_some());
    }

    #[test]
    fn test_save_and_load_with_messages() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        let mut session = store.create_session(agent_id).unwrap();
        session.messages.push(Message::user("Hello"));
        session.messages.push(Message::assistant("Hi there!"));
        store.save_session(&session).unwrap();

        let loaded = store.get_session(session.id).unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
    }

    #[test]
    fn test_get_missing_session() {
        let store = setup();
        let result = store.get_session(SessionId::new()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_delete_session() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        let session = store.create_session(agent_id).unwrap();
        let sid = session.id;
        assert!(store.get_session(sid).unwrap().is_some());
        store.delete_session(sid).unwrap();
        assert!(store.get_session(sid).unwrap().is_none());
    }

    #[test]
    fn test_delete_agent_sessions() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        let s1 = store.create_session(agent_id.clone()).unwrap();
        let s2 = store.create_session(agent_id.clone()).unwrap();
        assert!(store.get_session(s1.id).unwrap().is_some());
        assert!(store.get_session(s2.id).unwrap().is_some());
        store.delete_agent_sessions(&agent_id).unwrap();
        assert!(store.get_session(s1.id).unwrap().is_none());
        assert!(store.get_session(s2.id).unwrap().is_none());
    }

    #[test]
    fn test_jsonl_mirror_write() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        let mut session = store.create_session(agent_id).unwrap();
        session
            .messages
            .push(carrier_types::message::Message::user("Hello"));
        session
            .messages
            .push(carrier_types::message::Message::assistant("Hi there!"));
        store.save_session(&session).unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = dir.path().join("sessions");
        store
            .write_jsonl_mirror(&session, &sessions_dir, None, None, None, None)
            .unwrap();

        let jsonl_path = sessions_dir.join(format!("{}.jsonl", session.id.0));
        assert!(jsonl_path.exists());

        let content = std::fs::read_to_string(&jsonl_path).unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        // Verify first line is user message
        let line1: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(line1["role"], "user");
        assert_eq!(line1["content"], "Hello");

        // Verify second line is assistant message
        let line2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(line2["role"], "assistant");
        assert_eq!(line2["content"], "Hi there!");
        assert!(line2.get("tool_use").is_none());
    }

    #[test]
    fn test_strip_tool_history_removes_tool_blocks() {
        let messages = vec![
            Message::user("Generate an image"),
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                    id: "tu1".to_string(),
                    name: "image_generate".to_string(),
                    input: serde_json::json!({"prompt": "a cat"}),
                    provider_metadata: None,
                }]),
            },
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "tu1".to_string(),
                    tool_name: "image_generate".to_string(),
                    content: "huge base64 data here...".repeat(1000),
                    is_error: false,
                }]),
            },
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![ContentBlock::Text {
                    text: "Image generated successfully".to_string(),
                    provider_metadata: None,
                }]),
            },
        ];

        let clean = super::strip_tool_history(&messages);

        // Should have 4 messages (none removed entirely — tool blocks replaced with summaries)
        assert_eq!(clean.len(), 4);

        // First message unchanged
        assert_eq!(clean[0].role, Role::User);

        // Tool_use replaced with text summary
        if let MessageContent::Blocks(blocks) = &clean[1].content {
            assert!(blocks.iter().any(|b| matches!(b, ContentBlock::Text { text, .. } if text.contains("Called image_generate"))));
            assert!(!blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { .. })));
        } else {
            panic!("Expected Blocks");
        }

        // Tool_result replaced with text summary
        if let MessageContent::Blocks(blocks) = &clean[2].content {
            assert!(blocks.iter().any(
                |b| matches!(b, ContentBlock::Text { text, .. } if text.contains("Result from"))
            ));
            assert!(!blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolResult { .. })));
        } else {
            panic!("Expected Blocks");
        }

        // Final assistant text preserved
        if let MessageContent::Blocks(blocks) = &clean[3].content {
            assert!(blocks.iter().any(
                |b| matches!(b, ContentBlock::Text { text, .. } if text.contains("successfully"))
            ));
        }
    }

    #[test]
    fn test_save_session_strips_tools() {
        let store = setup();
        let mut session = store.create_session("test-agent".to_string()).unwrap();

        // Add messages with tool blocks
        session.messages.push(Message::user("Hello"));
        session.messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::Blocks(vec![ContentBlock::ToolUse {
                id: "tu1".to_string(),
                name: "some_tool".to_string(),
                input: serde_json::json!({}),
                provider_metadata: None,
            }]),
        });
        session.messages.push(Message {
            role: Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "tu1".to_string(),
                tool_name: "some_tool".to_string(),
                content: "big result data".repeat(1000),
                is_error: false,
            }]),
        });

        store.save_session(&session).unwrap();

        // Reload — tool blocks should be stripped
        let loaded = store.get_session(session.id).unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 3);

        // No ToolUse or ToolResult blocks in loaded messages
        for msg in &loaded.messages {
            if let MessageContent::Blocks(blocks) = &msg.content {
                for block in blocks {
                    assert!(
                        !matches!(block, ContentBlock::ToolUse { .. }),
                        "ToolUse should be stripped"
                    );
                    assert!(
                        !matches!(block, ContentBlock::ToolResult { .. }),
                        "ToolResult should be stripped"
                    );
                }
            }
        }
    }

    #[test]
    fn test_session_ticket_roundtrip() {
        let session = Session {
            id: SessionId::new(),
            agent_name: "gaokao-advisor".to_string(),
            messages: vec![
                Message::user("我模考 580 分"),
                Message::assistant("能冲哪些 985？"),
            ],
            turn_summaries: vec![TurnSummary {
                turn_number: 1,
                timestamp: "2026-08-22T00:00:00Z".to_string(),
                user_intent: "咨询高考志愿".to_string(),
                assistant_outcome: "给出冲稳保建议".to_string(),
                tools_used: vec![],
                key_facts: vec!["模考 580 分".to_string()],
            }],
            context_window_tokens: 1234,
            label: Some("borrow:gaokao-advisor".to_string()),
        };

        let ticket = session.to_ticket();
        assert_eq!(ticket.version, SessionTicket::CURRENT_VERSION);
        assert_eq!(ticket.messages.len(), 2);
        assert_eq!(ticket.turn_summaries.len(), 1);
        assert_eq!(ticket.context_window_tokens, 1234);

        // JSON round-trip over the wire (what the ACP bridge sends/receives).
        let json = serde_json::to_string(&ticket).unwrap();
        let decoded: SessionTicket = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.messages.len(), 2);
        assert_eq!(decoded.turn_summaries.len(), 1);
        assert_eq!(decoded.label.as_deref(), Some("borrow:gaokao-advisor"));

        // from_ticket restores a Session with a fresh id but the same state.
        let restored = Session::from_ticket(decoded, "gaokao-advisor".to_string());
        assert_eq!(restored.agent_name, "gaokao-advisor");
        assert_eq!(restored.messages.len(), 2);
        assert_eq!(restored.turn_summaries.len(), 1);
        assert_eq!(restored.context_window_tokens, 1234);
        assert_ne!(restored.id.0, session.id.0); // 全新 session id——服务器不记忆身份
    }

    /// 票据膨胀治理：消息历史超预算时丢最老轮次对，摘要层全量保留；
    /// 预算内会话原样保留。
    #[test]
    fn test_session_ticket_truncation_budget() {
        fn big_turn(i: usize) -> Vec<Message> {
            vec![
                Message::user(format!("第 {i} 轮问题：{}", "x".repeat(4096))),
                Message::assistant(format!("第 {i} 轮回答：{}", "y".repeat(4096))),
            ]
        }

        // 小会话（预算内）：不截断。
        let small = Session {
            id: SessionId::new(),
            agent_name: "a".to_string(),
            messages: vec![Message::user("hi"), Message::assistant("hello")],
            turn_summaries: Vec::new(),
            context_window_tokens: 10,
            label: None,
        };
        let t = small.to_ticket();
        assert_eq!(t.messages.len(), 2);

        // 大会话：60 对 × ~8KB ≈ 490KB，超 256KB 预算 → 必须收敛。
        let mut messages = Vec::new();
        for i in 0..60 {
            messages.extend(big_turn(i));
        }
        let summaries: Vec<TurnSummary> = (0..60)
            .map(|i| TurnSummary {
                turn_number: i as u32 + 1,
                timestamp: "2026-08-23T00:00:00Z".to_string(),
                user_intent: format!("意图{i}"),
                assistant_outcome: format!("结果{i}"),
                tools_used: vec![],
                key_facts: vec![],
            })
            .collect();
        let big = Session {
            id: SessionId::new(),
            agent_name: "a".to_string(),
            messages,
            turn_summaries: summaries,
            context_window_tokens: 999_999,
            label: None,
        };
        let ticket = big.to_ticket();

        // 收敛上界：截断后的消息体积 ≤ 预算。
        let kept_bytes: usize = ticket.messages.iter().map(SessionTicket::message_bytes).sum();
        assert!(
            kept_bytes <= SessionTicket::MAX_MESSAGES_BYTES,
            "kept {kept_bytes} > budget {}",
            SessionTicket::MAX_MESSAGES_BYTES
        );
        // 完整轮次对保留（偶数条），且保留了最新的消息。
        assert_eq!(ticket.messages.len() % 2, 0);
        assert!(ticket.messages.last().unwrap().content.text_content().contains("第 59 轮回答"));
        // 摘要层全量保留——丢掉的原文语义在摘要里。
        assert_eq!(ticket.turn_summaries.len(), 60);

        // 极端：单对超预算——只留最新一对，不再膨胀。
        let mut huge = Vec::new();
        for i in 0..3 {
            huge.push(Message::user(format!("u{}", "z".repeat(300_000))));
            huge.push(Message::assistant(format!("a{i}:{}", "w".repeat(1000))));
        }
        let extreme = Session {
            id: SessionId::new(),
            agent_name: "a".to_string(),
            messages: huge,
            turn_summaries: Vec::new(),
            context_window_tokens: 0,
            label: None,
        };
        let t2 = extreme.to_ticket();
        assert!(t2.messages.len() <= 2);
    }
}
