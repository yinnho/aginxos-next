//! SQLite system KV store for agent persistence and system key-value pairs.

use chrono::Utc;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::agent::{AgentEntry, AgentId};
use carrier_types::error::{CarrierError, CarrierResult};

/// System KV store backed by SQLite for agent entries and system key-value storage.
#[derive(Clone)]
pub struct SystemKV {
    conn: Arc<Mutex<Connection>>,
}

impl SystemKV {
    /// Create a new system KV store wrapping the given connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Get a value from the key-value store.
    pub fn get(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<Option<serde_json::Value>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT value FROM kv_store WHERE agent_id = ?1 AND owner_id = ?2 AND user_id = ?3 AND key = ?4",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let result = stmt.query_row(rusqlite::params![agent_id, owner_id, user_id, key], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        });
        match result {
            Ok(blob) => {
                let value: serde_json::Value = serde_json::from_slice(&blob)
                    .map_err(|e| CarrierError::Serialization(e.to_string()))?;
                Ok(Some(value))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Set a value in the key-value store.
    ///
    /// **Immutability guarantee**: before overwriting, the previous value is
    /// archived in `kv_history` so no memory is ever lost. The `kv_store`
    /// table always holds the latest value for fast lookup.
    pub fn set(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let blob =
            serde_json::to_vec(&value).map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let now = Utc::now().to_rfc3339();

        // Wrap archive + upsert in a transaction for atomicity
        conn.execute("BEGIN", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        // Archive the old value before overwriting (memory immutability)
        let old: Option<(Vec<u8>, i64)> = conn
            .query_row(
                "SELECT value, version FROM kv_store WHERE agent_id = ?1 AND owner_id = ?2 AND user_id = ?3 AND key = ?4",
                rusqlite::params![agent_id, owner_id, user_id, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        if let Some((old_blob, old_version)) = old {
            conn.execute(
                "INSERT INTO kv_history (agent_id, owner_id, user_id, key, value, version, archived_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![agent_id, owner_id, user_id, key, old_blob, old_version, now],
            )
            .map_err(|e| {
                let _ = conn.execute("ROLLBACK", []);
                CarrierError::Memory(e.to_string())
            })?;
        }

        conn.execute(
            "INSERT INTO kv_store (agent_id, owner_id, user_id, key, value, version, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)
             ON CONFLICT(agent_id, owner_id, user_id, key) DO UPDATE SET value = ?5, version = version + 1, updated_at = ?6",
            rusqlite::params![agent_id, owner_id, user_id, key, blob, now],
        )
        .map_err(|e| {
            let _ = conn.execute("ROLLBACK", []);
            CarrierError::Memory(e.to_string())
        })?;

        conn.execute("COMMIT", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Delete a value from the key-value store.
    ///
    /// **Immutability guarantee**: the value is archived to `kv_history`
    /// before deletion, so no memory is ever truly lost.
    pub fn delete(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let now = Utc::now().to_rfc3339();

        // Wrap archive + delete in a transaction for atomicity
        conn.execute("BEGIN", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        // Archive before deleting
        let old: Option<(Vec<u8>, i64)> = conn
            .query_row(
                "SELECT value, version FROM kv_store WHERE agent_id = ?1 AND owner_id = ?2 AND user_id = ?3 AND key = ?4",
                rusqlite::params![agent_id, owner_id, user_id, key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        if let Some((old_blob, old_version)) = old {
            conn.execute(
                "INSERT INTO kv_history (agent_id, owner_id, user_id, key, value, version, archived_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![agent_id, owner_id, user_id, key, old_blob, old_version, now],
            )
            .map_err(|e| {
                let _ = conn.execute("ROLLBACK", []);
                CarrierError::Memory(e.to_string())
            })?;
        }

        conn.execute(
            "DELETE FROM kv_store WHERE agent_id = ?1 AND owner_id = ?2 AND user_id = ?3 AND key = ?4",
            rusqlite::params![agent_id, owner_id, user_id, key],
        )
        .map_err(|e| {
            let _ = conn.execute("ROLLBACK", []);
            CarrierError::Memory(e.to_string())
        })?;

        conn.execute("COMMIT", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// List all key-value pairs for an agent.
    pub fn list_kv(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
    ) -> CarrierResult<Vec<(String, serde_json::Value)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare("SELECT key, value FROM kv_store WHERE agent_id = ?1 AND owner_id = ?2 AND user_id = ?3 ORDER BY key")
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![agent_id, owner_id, user_id], |row| {
                let key: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((key, blob))
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut pairs = Vec::new();
        for row in rows {
            let (key, blob) = row.map_err(|e| CarrierError::Memory(e.to_string()))?;
            let value: serde_json::Value = serde_json::from_slice(&blob).unwrap_or_else(|_| {
                // Fallback: try as UTF-8 string
                String::from_utf8(blob)
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null)
            });
            pairs.push((key, value));
        }
        Ok(pairs)
    }

    /// Save an agent entry to the database.
    pub fn save_agent(&self, entry: &AgentEntry) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        // Use named-field encoding so new fields with #[serde(default)] are
        // handled gracefully when the struct evolves between versions.
        let manifest_blob = rmp_serde::to_vec_named(&entry.manifest)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let state_str = serde_json::to_string(&entry.state)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let now = Utc::now().to_rfc3339();

        let identity_json = serde_json::to_string(&entry.identity)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;

        conn.execute(
            "INSERT INTO agents (id, name, manifest, state, created_at, updated_at, session_id, identity)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET name = ?2, manifest = ?3, state = ?4, updated_at = ?6, session_id = ?7, identity = ?8",
            rusqlite::params![
                entry.id.0.to_string(),
                entry.name,
                manifest_blob,
                state_str,
                entry.created_at.to_rfc3339(),
                now,
                entry.session_id.0.to_string(),
                identity_json,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Load an agent entry from the database.
    pub fn load_agent(&self, agent_id: AgentId) -> CarrierResult<Option<AgentEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut stmt = conn
            .prepare("SELECT id, name, manifest, state, created_at, updated_at, session_id, identity FROM agents WHERE id = ?1")
            .or_else(|_| {
                conn.prepare("SELECT id, name, manifest, state, created_at, updated_at, session_id FROM agents WHERE id = ?1")
                    .or_else(|_| {
                        // Fallback without session_id column for old DBs
                        conn.prepare("SELECT id, name, manifest, state, created_at, updated_at FROM agents WHERE id = ?1")
                    })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let col_count = stmt.column_count();
        let result = stmt.query_row(rusqlite::params![agent_id.to_string()], |row| {
            let manifest_blob: Vec<u8> = row.get(2)?;
            let state_str: String = row.get(3)?;
            let created_str: String = row.get(4)?;
            let name: String = row.get(1)?;
            let session_id_str: Option<String> = if col_count >= 7 {
                row.get(6).ok()
            } else {
                None
            };
            let identity_str: Option<String> = if col_count >= 8 {
                row.get(7).ok()
            } else {
                None
            };
            Ok((
                name,
                manifest_blob,
                state_str,
                created_str,
                session_id_str,
                identity_str,
            ))
        });

        match result {
            Ok((name, manifest_blob, state_str, created_str, session_id_str, identity_str)) => {
                let manifest = rmp_serde::from_slice(&manifest_blob)
                    .map_err(|e| CarrierError::Serialization(e.to_string()))?;
                let state = serde_json::from_str(&state_str)
                    .map_err(|e| CarrierError::Serialization(e.to_string()))?;
                let created_at = chrono::DateTime::parse_from_rfc3339(&created_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                let session_id = session_id_str
                    .and_then(|s| uuid::Uuid::parse_str(&s).ok())
                    .map(carrier_types::agent::SessionId)
                    .unwrap_or_else(carrier_types::agent::SessionId::new);
                let identity = identity_str
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default();
                Ok(Some(AgentEntry {
                    id: agent_id,
                    name,
                    manifest,
                    state,
                    mode: Default::default(),
                    created_at,
                    last_active: Utc::now(),
                    parent: None,
                    children: vec![],
                    session_id,
                    tags: vec![],
                    identity,
                    onboarding_completed: false,
                    onboarding_completed_at: None,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Remove an agent from the database.
    pub fn remove_agent(&self, agent_id: AgentId) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM agents WHERE id = ?1",
            rusqlite::params![agent_id.to_string()],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Load all agent entries from the database.
    ///
    /// Uses lenient deserialization (via `serde_compat`) to handle schema-mismatched
    /// fields gracefully. When an agent is loaded with lenient defaults, it is
    /// automatically re-saved to upgrade the stored blob. Duplicate agent names
    /// are deduplicated (first occurrence wins).
    pub fn load_all_agents(&self) -> CarrierResult<Vec<AgentEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        // Try with full columns first, fall back gracefully
        let mut stmt = if let Ok(s) = conn.prepare(
            "SELECT id, name, manifest, state, created_at, updated_at, session_id, identity FROM agents",
        ) {
            s
        } else if let Ok(s) = conn.prepare(
            "SELECT id, name, manifest, state, created_at, updated_at, session_id FROM agents",
        ) {
            s
        } else {
            conn.prepare(
                "SELECT id, name, manifest, state, created_at, updated_at FROM agents",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?
        };

        let col_count = stmt.column_count();

        #[allow(clippy::type_complexity)]
        let row_data: Vec<
            rusqlite::Result<(
                String,
                String,
                Vec<u8>,
                String,
                String,
                Option<String>,
                Option<String>,
            )>,
        > = stmt
            .query_map([], |row| Self::row_to_agent_parts(row, col_count))
            .map_err(|e| CarrierError::Memory(e.to_string()))?
            .collect();

        let mut agents = Vec::new();
        let mut seen_names = std::collections::HashSet::new();
        let mut repair_queue: Vec<(String, Vec<u8>, String)> = Vec::new();

        for row in row_data {
            let (id_str, name, manifest_blob, state_str, created_str, session_id_str, identity_str) =
                match row {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("Skipping agent row with read error: {e}");
                        continue;
                    }
                };

            // Deduplicate: skip agents with same name we've already seen
            let name_lower = name.to_lowercase();
            if !seen_names.insert(name_lower.clone()) {
                tracing::info!(agent = %name, id = %id_str, "Skipping duplicate agent name");
                continue;
            }

            let agent_id = match uuid::Uuid::parse_str(&id_str).map(carrier_types::agent::AgentId) {
                Ok(id) => id,
                Err(e) => {
                    tracing::warn!(agent = %name, "Skipping agent with bad UUID '{id_str}': {e}");
                    continue;
                }
            };

            let manifest: carrier_types::agent::AgentManifest = match rmp_serde::from_slice(&manifest_blob)
            {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(
                        agent = %name, id = %id_str,
                        "Skipping agent with incompatible manifest (schema may have changed): {e}"
                    );
                    continue;
                }
            };

            // Auto-repair: re-serialize with current schema and queue for update.
            // This upgrades the stored blob so future boots don't hit lenient paths.
            let new_blob = rmp_serde::to_vec_named(&manifest)
                .map_err(|e| CarrierError::Serialization(e.to_string()))?;
            if new_blob != manifest_blob {
                tracing::info!(
                    agent = %name, id = %id_str,
                    "Auto-repaired agent manifest (schema upgraded)"
                );
                repair_queue.push((id_str.clone(), new_blob, name.clone()));
            }

            let state = match serde_json::from_str(&state_str) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(agent = %name, "Skipping agent with bad state: {e}");
                    continue;
                }
            };
            let created_at = chrono::DateTime::parse_from_rfc3339(&created_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let session_id = session_id_str
                .and_then(|s| uuid::Uuid::parse_str(&s).ok())
                .map(carrier_types::agent::SessionId)
                .unwrap_or_else(carrier_types::agent::SessionId::new);

            let identity = identity_str
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();

            agents.push(AgentEntry {
                id: agent_id,
                name,
                manifest,
                state,
                mode: Default::default(),
                created_at,
                last_active: Utc::now(),
                parent: None,
                children: vec![],
                session_id,
                tags: vec![],
                identity,
                onboarding_completed: false,
                onboarding_completed_at: None,
            });
        }

        // Apply queued repairs (re-save upgraded blobs)
        for (id_str, new_blob, name) in repair_queue {
            if let Err(e) = conn.execute(
                "UPDATE agents SET manifest = ?1 WHERE id = ?2",
                rusqlite::params![new_blob, id_str],
            ) {
                tracing::warn!(agent = %name, "Failed to auto-repair agent blob: {e}");
            }
        }

        Ok(agents)
    }

    /// Extract agent row parts into a tuple for both load_agent and load_all_agents.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn row_to_agent_parts(
        row: &rusqlite::Row,
        col_count: usize,
    ) -> rusqlite::Result<(
        String,
        String,
        Vec<u8>,
        String,
        String,
        Option<String>,
        Option<String>,
    )> {
        let id_str: String = row.get(0)?;
        let name: String = row.get(1)?;
        let manifest_blob: Vec<u8> = row.get(2)?;
        let state_str: String = row.get(3)?;
        let created_str: String = row.get(4)?;
        let session_id_str: Option<String> = if col_count >= 7 {
            row.get(6).ok()
        } else {
            None
        };
        let identity_str: Option<String> = if col_count >= 8 {
            row.get(7).ok()
        } else {
            None
        };
        Ok((
            id_str,
            name,
            manifest_blob,
            state_str,
            created_str,
            session_id_str,
            identity_str,
        ))
    }

    /// List all agents in the database.
    pub fn list_agents(&self) -> CarrierResult<Vec<(String, String, String)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let sql = "SELECT id, name, state FROM agents";
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let row_data: Vec<rusqlite::Result<(String, String, String)>> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?
            .collect();
        let mut agents = Vec::new();
        for row in row_data {
            agents.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(agents)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> SystemKV {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        SystemKV::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_kv_set_get() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        store
            .set(
                &agent_id,
                "user1",
                "user1",
                "test_key",
                serde_json::json!("test_value"),
            )
            .unwrap();
        let value = store.get(&agent_id, "user1", "user1", "test_key").unwrap();
        assert_eq!(value, Some(serde_json::json!("test_value")));
    }

    #[test]
    fn test_kv_get_missing() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        let value = store
            .get(&agent_id, "user1", "user1", "nonexistent")
            .unwrap();
        assert!(value.is_none());
    }

    #[test]
    fn test_kv_delete() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        store
            .set(
                &agent_id,
                "user1",
                "user1",
                "to_delete",
                serde_json::json!(42),
            )
            .unwrap();
        store
            .delete(&agent_id, "user1", "user1", "to_delete")
            .unwrap();
        let value = store.get(&agent_id, "user1", "user1", "to_delete").unwrap();
        assert!(value.is_none());
    }

    #[test]
    fn test_kv_update() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        store
            .set(&agent_id, "user1", "user1", "key", serde_json::json!("v1"))
            .unwrap();
        store
            .set(&agent_id, "user1", "user1", "key", serde_json::json!("v2"))
            .unwrap();
        let value = store.get(&agent_id, "user1", "user1", "key").unwrap();
        assert_eq!(value, Some(serde_json::json!("v2")));
    }

    #[test]
    fn test_kv_per_user_isolation() {
        let store = setup();
        let agent_id = "test-agent".to_string();
        store
            .set(
                &agent_id,
                "user_a",
                "user_a",
                "pref",
                serde_json::json!("dark mode"),
            )
            .unwrap();
        store
            .set(
                &agent_id,
                "user_b",
                "user_b",
                "pref",
                serde_json::json!("light mode"),
            )
            .unwrap();

        // Each user sees their own value
        assert_eq!(
            store.get(&agent_id, "user_a", "user_a", "pref").unwrap(),
            Some(serde_json::json!("dark mode"))
        );
        assert_eq!(
            store.get(&agent_id, "user_b", "user_b", "pref").unwrap(),
            Some(serde_json::json!("light mode"))
        );

        // list_kv is per-user
        let a_keys = store.list_kv(&agent_id, "user_a", "user_a").unwrap();
        let b_keys = store.list_kv(&agent_id, "user_b", "user_b").unwrap();
        assert_eq!(a_keys.len(), 1);
        assert_eq!(b_keys.len(), 1);
    }
}
