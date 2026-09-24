//! Persistent WeChat iLink session store backed by SQLite.
//!
//! Replaces the previous `senders/*/session.json` files.
//! Session data (bot_token, context_tokens, etc.) is stored in the
//! `weixin_sessions` table within the central `opencarrier.db`.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};

/// Serialized form of a WeChat iLink bot session.
/// Mirrors channel_weixin::models::BotTokenFile without depending on the weixin crate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WeixinSessionRow {
    pub channel: String,
    pub sender_key: String,
    pub bot_id: String,
    pub bot_token: String,
    pub baseurl: String,
    pub ilink_bot_id: String,
    pub user_id: Option<String>,
    pub expires_at: i64,
    pub bind_agent: Option<String>,
    pub context_tokens: String, // JSON: {"target_user": "token"}
}

/// SQLite-backed WeChat iLink session store.
#[derive(Clone)]
pub struct WeixinSessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl WeixinSessionStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Load all persisted sessions.
    pub fn load_all(&self) -> CarrierResult<Vec<WeixinSessionRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT channel, sender_key, bot_id, bot_token, baseurl, ilink_bot_id, \
                    user_id, expires_at, bind_agent, context_tokens \
             FROM weixin_sessions",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(WeixinSessionRow {
                    channel: row.get(0)?,
                    sender_key: row.get(1)?,
                    bot_id: row.get(2)?,
                    bot_token: row.get(3)?,
                    baseurl: row.get(4)?,
                    ilink_bot_id: row.get(5)?,
                    user_id: row.get(6)?,
                    expires_at: row.get(7)?,
                    bind_agent: row.get(8)?,
                    context_tokens: row.get(9)?,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(sessions)
    }

    /// Upsert a single session.
    pub fn upsert(&self, row: &WeixinSessionRow) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let user_id = row.user_id.as_deref().unwrap_or("");
        conn.execute(
            "INSERT INTO weixin_sessions (user_id, channel, sender_key, bot_id, bot_token, baseurl, ilink_bot_id, expires_at, bind_agent, context_tokens) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) \
             ON CONFLICT(user_id) DO UPDATE SET \
               channel=?2, sender_key=?3, bot_id=?4, bot_token=?5, baseurl=?6, \
               ilink_bot_id=?7, expires_at=?8, bind_agent=?9, context_tokens=?10, \
               updated_at=datetime('now')",
            rusqlite::params![
                user_id, row.channel, row.sender_key, row.bot_id, row.bot_token,
                row.baseurl, row.ilink_bot_id, row.expires_at, row.bind_agent, row.context_tokens,
            ],
        ).map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Delete a session by user_id.
    pub fn delete(&self, user_id: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM weixin_sessions WHERE user_id = ?1",
            rusqlite::params![user_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }
}
