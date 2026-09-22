//! Cron delivery routing: tracks last channel per sender and buffers
//! notifications for channels that don't support proactive push.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};

/// Channel + bot identification for a sender's most recent inbound message.
#[derive(Debug, Clone)]
pub struct LastChannel {
    pub channel_type: String,
    pub bot_id: String,
    pub last_seen_at: i64,
}

/// A buffered notification waiting for the user to send an inbound message.
#[derive(Debug, Clone)]
pub struct PendingNotification {
    pub id: i64,
    pub sender_id: String,
    pub agent_id: String,
    pub message: String,
    pub kind: String,
    pub created_at: i64,
}

/// Default TTL for pending notifications (24 hours).
pub const DEFAULT_TTL_SECS: i64 = 24 * 3600;

/// Storage for cron delivery routing data.
#[derive(Clone)]
pub struct CronDeliveryStore {
    conn: Arc<Mutex<Connection>>,
}

impl CronDeliveryStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Record that a sender just sent an inbound message via this channel.
    pub fn touch_sender_channel(
        &self,
        sender_id: &str,
        channel_type: &str,
        bot_id: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let now = chrono::Utc::now().timestamp();
        conn.execute(
            "INSERT INTO sender_channels (sender_id, channel_type, bot_id, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(sender_id) DO UPDATE SET \
               channel_type = ?2, bot_id = ?3, last_seen_at = ?4",
            rusqlite::params![sender_id, channel_type, bot_id, now],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Look up the channel a sender most recently used.
    pub fn get_last_channel(&self, sender_id: &str) -> CarrierResult<Option<LastChannel>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT channel_type, bot_id, last_seen_at FROM sender_channels WHERE sender_id = ?1")
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let result = stmt.query_row(rusqlite::params![sender_id], |row| {
            Ok(LastChannel {
                channel_type: row.get(0)?,
                bot_id: row.get(1)?,
                last_seen_at: row.get(2)?,
            })
        });
        match result {
            Ok(c) => Ok(Some(c)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Buffer a notification for later delivery.
    pub fn buffer_notification(
        &self,
        sender_id: &str,
        agent_id: &str,
        message: &str,
        kind: &str,
        ttl_secs: i64,
    ) -> CarrierResult<i64> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let now = chrono::Utc::now().timestamp();
        let expires_at = now + ttl_secs;
        conn.execute(
            "INSERT INTO pending_notifications \
             (sender_id, agent_id, message, kind, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![sender_id, agent_id, message, kind, now, expires_at],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(conn.last_insert_rowid())
    }

    /// Take all undelivered notifications for a sender (in chronological order).
    /// Returns the notifications and marks them as delivered atomically.
    pub fn drain_pending(&self, sender_id: &str) -> CarrierResult<Vec<PendingNotification>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let now = chrono::Utc::now().timestamp();

        // Fetch undelivered + not expired
        let notifications: Vec<PendingNotification> = {
            let mut stmt = conn
                .prepare(
                    "SELECT id, sender_id, agent_id, message, kind, created_at \
                     FROM pending_notifications \
                     WHERE sender_id = ?1 AND delivered_at IS NULL AND expires_at > ?2 \
                     ORDER BY created_at ASC",
                )
                .map_err(|e| CarrierError::Memory(e.to_string()))?;
            let rows = stmt
                .query_map(rusqlite::params![sender_id, now], |row| {
                    Ok(PendingNotification {
                        id: row.get(0)?,
                        sender_id: row.get(1)?,
                        agent_id: row.get(2)?,
                        message: row.get(3)?,
                        kind: row.get(4)?,
                        created_at: row.get(5)?,
                    })
                })
                .map_err(|e| CarrierError::Memory(e.to_string()))?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
            }
            out
        };

        if notifications.is_empty() {
            return Ok(Vec::new());
        }

        // Mark all as delivered
        conn.execute(
            "UPDATE pending_notifications SET delivered_at = ?1 \
             WHERE sender_id = ?2 AND delivered_at IS NULL AND expires_at > ?1",
            rusqlite::params![now, sender_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;

        Ok(notifications)
    }

    /// Delete expired notifications. Returns the number of rows deleted.
    pub fn purge_expired(&self) -> CarrierResult<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let now = chrono::Utc::now().timestamp();
        let deleted = conn
            .execute(
                "DELETE FROM pending_notifications WHERE expires_at <= ?1",
                rusqlite::params![now],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(deleted)
    }
}

/// Best-effort `(channel, bot_id)` for a sender based on id format.
///
/// This is the **fallback** when no `sender_channels` entry exists — a cold
/// push to someone who never sent an inbound message on this deployment.
/// `source_bot` is reused as the bot_id guess: for a sender with no recorded
/// inbound there is no authoritative bot, and hard-coding a deployment-specific
/// name (the old `"86bus-kf"` / `"default"`) silently broke every other tenant.
/// `wm…` → wecom-kf, `*@im.wechat` → weixin (iLink), bare id → weixin-oa.
pub fn infer_channel_by_prefix(sender_id: &str, source_bot: &str) -> (&'static str, String) {
    if sender_id.starts_with("wm") {
        ("wecom", source_bot.to_string())
    } else if sender_id.contains("@im.wechat") {
        ("weixin", source_bot.to_string())
    } else {
        ("weixin-oa", source_bot.to_string())
    }
}

/// Resolve a recipient's `(channel_type, bot_id, user_id)` for outbound push.
///
/// Authoritative source is the `sender_channels` table (recorded on every
/// inbound via [`CronDeliveryStore::touch_sender_channel`]) — it records the
/// exact bot the sender last used, so it is correct for multi-tenant and
/// cross-channel setups. Only when no entry exists do we fall back to
/// format-based inference ([`infer_channel_by_prefix`]).
pub fn route_recipient(
    sender_id: &str,
    store: &CronDeliveryStore,
    source_bot: &str,
) -> (String, String, String) {
    if let Some(lc) = store.get_last_channel(sender_id).ok().flatten() {
        return (lc.channel_type, lc.bot_id, sender_id.to_string());
    }
    let (channel, bot_id) = infer_channel_by_prefix(sender_id, source_bot);
    (channel.to_string(), bot_id, sender_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> CronDeliveryStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        CronDeliveryStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_touch_and_get_last_channel() {
        let store = setup();
        store
            .touch_sender_channel("user-1", "weixin", "default")
            .unwrap();
        let last = store.get_last_channel("user-1").unwrap().unwrap();
        assert_eq!(last.channel_type, "weixin");
        assert_eq!(last.bot_id, "default");
    }

    #[test]
    fn test_get_last_channel_missing() {
        let store = setup();
        let result = store.get_last_channel("unknown").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_buffer_and_drain() {
        let store = setup();
        store
            .buffer_notification("user-1", "agent-1", "first", "cron", DEFAULT_TTL_SECS)
            .unwrap();
        store
            .buffer_notification("user-1", "agent-1", "second", "cron", DEFAULT_TTL_SECS)
            .unwrap();
        let drained = store.drain_pending("user-1").unwrap();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].message, "first");
        assert_eq!(drained[1].message, "second");
        // Second drain returns nothing
        let drained2 = store.drain_pending("user-1").unwrap();
        assert!(drained2.is_empty());
    }

    #[test]
    fn test_drain_only_affects_target_sender() {
        let store = setup();
        store
            .buffer_notification("user-1", "agent-1", "for-1", "cron", DEFAULT_TTL_SECS)
            .unwrap();
        store
            .buffer_notification("user-2", "agent-1", "for-2", "cron", DEFAULT_TTL_SECS)
            .unwrap();
        let drained = store.drain_pending("user-1").unwrap();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].message, "for-1");
        // user-2 still has theirs
        let drained2 = store.drain_pending("user-2").unwrap();
        assert_eq!(drained2.len(), 1);
        assert_eq!(drained2[0].message, "for-2");
    }

    #[test]
    fn test_expired_not_drained() {
        let store = setup();
        // Use TTL of -1 (already expired)
        store
            .buffer_notification("user-1", "agent-1", "expired", "cron", -1)
            .unwrap();
        let drained = store.drain_pending("user-1").unwrap();
        assert!(drained.is_empty());
    }

    #[test]
    fn test_purge_expired() {
        let store = setup();
        store
            .buffer_notification("user-1", "agent-1", "fresh", "cron", DEFAULT_TTL_SECS)
            .unwrap();
        store
            .buffer_notification("user-1", "agent-1", "stale", "cron", -1)
            .unwrap();
        let deleted = store.purge_expired().unwrap();
        assert_eq!(deleted, 1);
        // Fresh one still drainable
        let drained = store.drain_pending("user-1").unwrap();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].message, "fresh");
    }
}
