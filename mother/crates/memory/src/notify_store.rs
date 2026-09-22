//! Persistent notify route store backed by SQLite.
//!
//! Replaces the previous `notify_routes.json` file.
//! Routes map a notify type (e.g. "urgent", "alarm") to a push target.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};

/// Serialized form of a notify route.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NotifyRouteRow {
    pub name: String,
    pub channel: String,
    pub bot_id: String,
    pub user_id: String,
    pub prefix: Option<String>,
    pub recipients: Option<String>,
}

/// SQLite-backed notify route store.
#[derive(Clone)]
pub struct NotifyRouteStore {
    conn: Arc<Mutex<Connection>>,
}

impl NotifyRouteStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Load all routes.
    pub fn load_all(&self) -> CarrierResult<Vec<NotifyRouteRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT name, channel, bot_id, user_id, prefix, recipients FROM notify_routes")
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(NotifyRouteRow {
                    name: row.get(0)?,
                    channel: row.get(1)?,
                    bot_id: row.get(2)?,
                    user_id: row.get(3)?,
                    prefix: row.get(4)?,
                    recipients: row.get(5)?,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut routes = Vec::new();
        for row in rows {
            routes.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(routes)
    }

    /// Upsert a single route.
    pub fn upsert(&self, row: &NotifyRouteRow) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO notify_routes (name, channel, bot_id, user_id, prefix, recipients) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(name) DO UPDATE SET \
               channel=?2, bot_id=?3, user_id=?4, prefix=?5, recipients=?6",
            rusqlite::params![
                row.name,
                row.channel,
                row.bot_id,
                row.user_id,
                row.prefix,
                row.recipients,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Delete a route by name.
    pub fn delete(&self, name: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM notify_routes WHERE name = ?1",
            rusqlite::params![name],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }
}
