//! Channel followers ledger backed by SQLite (automation Phase 2).
//!
//! Records every follower of a channel bot (weixin-oa: the OA account) —
//! follow/unfollow lifecycle plus `last_seen` refreshed on ANY inbound message.
//! `last_seen` is what makes `list_pushable` honest: the OA customer-service
//! API can only deliver within 48h of the user's last interaction, so a
//! scheduled push to "followers" can only target the recently-active subset.
//!
//! Mirrors `AutomationRuleStore` (`automation_store.rs`): shared
//! `Arc<Mutex<Connection>>` with sync rusqlite bodies. The substrate wraps
//! these in `spawn_blocking` async fns.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::{CarrierError, CarrierResult};

/// A follower row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Follower {
    pub channel: String,
    pub app_id: String,
    pub openid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unionid: Option<String>,
    /// RFC3339 when the follow happened (first seen as a follower).
    pub followed_at: String,
    /// RFC3339 when the user unfollowed; `None` = still a follower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfollowed_at: Option<String>,
    /// RFC3339 of the last inbound message from this user (any type).
    pub last_seen_at: String,
    /// Last QR scene / menu key (diagnostics for scan-triggered follows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_scene: Option<String>,
}

/// Growth summary over a window (for `FollowerReport` cron digests).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct FollowerStats {
    /// Follows recorded since the window start.
    pub new_followers: usize,
    /// Unfollows since the window start.
    pub unfollows: usize,
    /// Currently-active followers (never unfollowed).
    pub active: usize,
    /// Active followers whose `last_seen` is within the push window.
    pub pushable: usize,
}

/// SQLite-backed followers ledger.
#[derive(Clone)]
pub struct FollowerStore {
    conn: Arc<Mutex<Connection>>,
}

impl FollowerStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Follow (or re-follow): upsert the row, refreshing `followed_at` only on
    /// first insert. A re-follow after unfollow clears `unfollowed_at`.
    pub fn record_follow(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
        unionid: Option<&str>,
        scene: Option<&str>,
        now_rfc3339: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO channel_followers (channel, app_id, openid, unionid, followed_at, \
                                            unfollowed_at, last_seen_at, last_scene) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?5, ?6) \
             ON CONFLICT(channel, app_id, openid) DO UPDATE SET \
               unfollowed_at=NULL, last_seen_at=?5, \
               unionid=COALESCE(?4, unionid), last_scene=COALESCE(?6, last_scene)",
            rusqlite::params![channel, app_id, openid, unionid, now_rfc3339, scene],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Any inbound message: refresh `last_seen_at` (creates the row too — an OA
    /// user can only message after following, so a message implies a follow we
    /// may have missed).
    pub fn touch(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
        now_rfc3339: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO channel_followers (channel, app_id, openid, followed_at, \
                                            last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, ?4) \
             ON CONFLICT(channel, app_id, openid) DO UPDATE SET last_seen_at=?4",
            rusqlite::params![channel, app_id, openid, now_rfc3339],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Unfollow: stamp `unfollowed_at` (row kept — history for growth stats).
    pub fn mark_unfollowed(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
        now_rfc3339: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "UPDATE channel_followers SET unfollowed_at=?4 \
             WHERE channel=?1 AND app_id=?2 AND openid=?3",
            rusqlite::params![channel, app_id, openid, now_rfc3339],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Active (never unfollowed) followers whose `last_seen_at` is at or after
    /// `since_rfc3339` — the deliverable audience for a scheduled push
    /// (caller passes the 48h customer-service window, minus a margin).
    pub fn list_pushable_since(
        &self,
        channel: &str,
        app_id: &str,
        since_rfc3339: &str,
    ) -> CarrierResult<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT openid FROM channel_followers \
                 WHERE channel=?1 AND app_id=?2 AND unfollowed_at IS NULL \
                   AND last_seen_at >= ?3 \
                 ORDER BY last_seen_at DESC",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![channel, app_id, since_rfc3339], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(out)
    }

    /// Growth summary for `FollowerReport` digests. `since_rfc3339` bounds the
    /// new-follower/unfollow counts (RFC3339 strings compare lexicographically
    /// for identical timezone/format — the webhook writes UTC RFC3339).
    pub fn stats_since(
        &self,
        channel: &str,
        app_id: &str,
        since_rfc3339: &str,
        push_window_since_rfc3339: &str,
    ) -> CarrierResult<FollowerStats> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let count = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> CarrierResult<usize> {
            conn.query_row(sql, params, |row| row.get::<_, i64>(0))
                .map(|n| n as usize)
                .map_err(|e| CarrierError::Memory(e.to_string()))
        };
        Ok(FollowerStats {
            new_followers: count(
                "SELECT COUNT(*) FROM channel_followers \
                 WHERE channel=?1 AND app_id=?2 AND followed_at >= ?3",
                &[&channel, &app_id, &since_rfc3339],
            )?,
            unfollows: count(
                "SELECT COUNT(*) FROM channel_followers \
                 WHERE channel=?1 AND app_id=?2 AND unfollowed_at IS NOT NULL \
                   AND unfollowed_at >= ?3",
                &[&channel, &app_id, &since_rfc3339],
            )?,
            active: count(
                "SELECT COUNT(*) FROM channel_followers \
                 WHERE channel=?1 AND app_id=?2 AND unfollowed_at IS NULL",
                &[&channel, &app_id],
            )?,
            pushable: count(
                "SELECT COUNT(*) FROM channel_followers \
                 WHERE channel=?1 AND app_id=?2 AND unfollowed_at IS NULL \
                   AND last_seen_at >= ?3",
                &[&channel, &app_id, &push_window_since_rfc3339],
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn test_store() -> FollowerStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        FollowerStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn follow_touch_unfollow_lifecycle() {
        let store = test_store();
        store
            .record_follow(
                "weixin-oa",
                "wx1",
                "oA",
                Some("uA"),
                Some("qrscene_bus"),
                "2026-08-17T01:00:00Z",
            )
            .unwrap();
        store
            .record_follow("weixin-oa", "wx1", "oB", None, None, "2026-08-17T02:00:00Z")
            .unwrap();
        // Later interaction refreshes last_seen only.
        store
            .touch("weixin-oa", "wx1", "oA", "2026-08-17T03:00:00Z")
            .unwrap();

        let pushable = store
            .list_pushable_since("weixin-oa", "wx1", "2026-08-17T02:30:00Z")
            .unwrap();
        assert_eq!(pushable, vec!["oA".to_string()]); // oB last seen 02:00 < cutoff

        // Unfollow removes oA from pushable but keeps the row for stats.
        store
            .mark_unfollowed("weixin-oa", "wx1", "oA", "2026-08-17T04:00:00Z")
            .unwrap();
        assert!(store
            .list_pushable_since("weixin-oa", "wx1", "2026-08-16T00:00:00Z")
            .unwrap()
            .iter()
            .all(|x| x != "oA"));

        let stats = store
            .stats_since(
                "weixin-oa",
                "wx1",
                "2026-08-17T00:00:00Z",
                "2026-08-17T03:30:00Z",
            )
            .unwrap();
        assert_eq!(stats.new_followers, 2);
        assert_eq!(stats.unfollows, 1);
        assert_eq!(stats.active, 1); // oB
        assert_eq!(stats.pushable, 0); // oB last seen 02:00 < 03:30

        // Re-follow clears unfollowed_at.
        store
            .record_follow("weixin-oa", "wx1", "oA", None, None, "2026-08-17T05:00:00Z")
            .unwrap();
        let stats = store
            .stats_since(
                "weixin-oa",
                "wx1",
                "2026-08-17T05:00:00Z",
                "2026-08-17T00:00:00Z",
            )
            .unwrap();
        assert_eq!(stats.active, 2);
    }

    #[test]
    fn touch_creates_missing_row() {
        let store = test_store();
        // A user who messages without a recorded subscribe still gets a row
        // (OA only allows followers to message — the follow event was missed).
        store
            .touch("weixin-oa", "wx1", "oC", "2026-08-17T06:00:00Z")
            .unwrap();
        let pushable = store
            .list_pushable_since("weixin-oa", "wx1", "2026-08-17T00:00:00Z")
            .unwrap();
        assert_eq!(pushable, vec!["oC".to_string()]);
    }
}
