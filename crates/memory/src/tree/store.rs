//! Chunk CRUD operations for tree memory.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};

use super::types::{Chunk, SourceKind, CHUNK_STATUS_ADMITTED};

/// Chunk store backed by SQLite.
#[derive(Clone)]
pub struct ChunkStore {
    conn: Arc<Mutex<Connection>>,
}

impl ChunkStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Insert or replace chunks in bulk.
    pub fn upsert_chunks(&self, chunks: &[Chunk]) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute("BEGIN", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        for c in chunks {
            let result = conn.execute(
                "INSERT OR REPLACE INTO mem_tree_chunks
                 (id, owner_id, user_id, agent_id, source_kind, source_id, source_ref,
                  timestamp_ms, time_range_start_ms, time_range_end_ms,
                  tags_json, content, token_count, seq_in_source,
                  partial_message, lifecycle_status, created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                rusqlite::params![
                    c.id,
                    c.owner_id,
                    c.user_id,
                    c.agent_id,
                    c.source_kind.as_str(),
                    c.source_id,
                    c.source_ref,
                    c.timestamp_ms,
                    c.time_range_start_ms,
                    c.time_range_end_ms,
                    c.tags_json,
                    c.content,
                    c.token_count,
                    c.seq_in_source,
                    c.partial_message as i32,
                    c.lifecycle_status,
                    c.created_at_ms,
                ],
            );
            if let Err(e) = result {
                let _ = conn.execute("ROLLBACK", []);
                return Err(CarrierError::Memory(e.to_string()));
            }
        }

        conn.execute("COMMIT", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Get a single chunk by ID.
    ///
    /// When `user_id` is `Some(u)`, enforces per-user isolation: the chunk is
    /// returned only if it belongs to `u` or is owner-shared (`user_id = ''`).
    /// `None` skips the filter (internal/write-path lookups by exact id).
    pub fn get_chunk(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        chunk_id: &str,
    ) -> CarrierResult<Option<Chunk>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(owner_id.to_string()),
            Box::new(chunk_id.to_string()),
        ];

        let mut sql = "SELECT id, owner_id, user_id, agent_id, source_kind, source_id, source_ref,
                    timestamp_ms, time_range_start_ms, time_range_end_ms,
                    tags_json, content, token_count, seq_in_source,
                    partial_message, lifecycle_status, created_at_ms
             FROM mem_tree_chunks WHERE owner_id = ?1 AND id = ?2"
            .to_string();

        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let result = conn.query_row(&sql, param_refs.as_slice(), Self::row_to_chunk);

        match result {
            Ok(chunk) => Ok(Some(chunk)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// List chunks for an owner, optionally filtered by user, source and lifecycle status.
    pub fn list_chunks(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        source_kind: Option<&SourceKind>,
        source_id: Option<&str>,
        lifecycle_status: Option<&str>,
        limit: usize,
    ) -> CarrierResult<Vec<Chunk>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(owner_id.to_string())];

        let mut sql = "SELECT id, owner_id, user_id, agent_id, source_kind, source_id, source_ref,
                    timestamp_ms, time_range_start_ms, time_range_end_ms,
                    tags_json, content, token_count, seq_in_source,
                    partial_message, lifecycle_status, created_at_ms
             FROM mem_tree_chunks WHERE owner_id = ?1"
            .to_string();

        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }
        if let Some(sk) = source_kind {
            sql.push_str(" AND source_kind = ?");
            params.push(Box::new(sk.as_str().to_string()));
        }
        if let Some(sid) = source_id {
            sql.push_str(" AND source_id = ?");
            params.push(Box::new(sid.to_string()));
        }
        if let Some(ls) = lifecycle_status {
            sql.push_str(" AND lifecycle_status = ?");
            params.push(Box::new(ls.to_string()));
        }

        sql.push_str(" ORDER BY timestamp_ms ASC LIMIT ?");
        params.push(Box::new(limit as i64));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), Self::row_to_chunk)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut chunks = Vec::new();
        for row in rows {
            chunks.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(chunks)
    }

    /// Update the lifecycle status of a chunk.
    pub fn update_lifecycle(
        &self,
        owner_id: &str,
        chunk_id: &str,
        new_status: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute(
            "UPDATE mem_tree_chunks SET lifecycle_status = ?1 WHERE owner_id = ?2 AND id = ?3",
            rusqlite::params![new_status, owner_id, chunk_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Mark all pending-extraction chunks as admitted (used after entity extraction completes).
    pub fn mark_admitted(&self, owner_id: &str, chunk_ids: &[String]) -> CarrierResult<()> {
        if chunk_ids.is_empty() {
            return Ok(());
        }
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute("BEGIN", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        for cid in chunk_ids {
            if let Err(e) = conn.execute(
                "UPDATE mem_tree_chunks SET lifecycle_status = ?1 WHERE owner_id = ?2 AND id = ?3",
                rusqlite::params![CHUNK_STATUS_ADMITTED, owner_id, cid],
            ) {
                let _ = conn.execute("ROLLBACK", []);
                return Err(CarrierError::Memory(e.to_string()));
            }
        }

        conn.execute("COMMIT", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Count chunks for an owner, optionally filtered by lifecycle status.
    pub fn count_chunks(
        &self,
        owner_id: &str,
        lifecycle_status: Option<&str>,
    ) -> CarrierResult<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let sql = match lifecycle_status {
            Some(_) => {
                "SELECT COUNT(*) FROM mem_tree_chunks WHERE owner_id = ?1 AND lifecycle_status = ?2"
            }
            None => "SELECT COUNT(*) FROM mem_tree_chunks WHERE owner_id = ?1",
        };

        let count: i64 = match lifecycle_status {
            Some(ls) => conn
                .query_row(sql, rusqlite::params![owner_id, ls], |row| row.get(0))
                .map_err(|e| CarrierError::Memory(e.to_string()))?,
            None => conn
                .query_row(sql, rusqlite::params![owner_id], |row| row.get(0))
                .map_err(|e| CarrierError::Memory(e.to_string()))?,
        };

        Ok(count as usize)
    }

    fn row_to_chunk(row: &rusqlite::Row) -> rusqlite::Result<Chunk> {
        let source_kind_str: String = row.get(4)?;
        let source_kind = match source_kind_str.as_str() {
            "chat" => SourceKind::Chat,
            "email" => SourceKind::Email,
            "document" => SourceKind::Document,
            _ => SourceKind::Chat,
        };
        let partial: i32 = row.get(14)?;

        Ok(Chunk {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            user_id: row.get(2)?,
            agent_id: row.get(3)?,
            source_kind,
            source_id: row.get(5)?,
            source_ref: row.get(6)?,
            timestamp_ms: row.get(7)?,
            time_range_start_ms: row.get(8)?,
            time_range_end_ms: row.get(9)?,
            tags_json: row.get(10)?,
            content: row.get(11)?,
            token_count: row.get(12)?,
            seq_in_source: row.get(13)?,
            partial_message: partial != 0,
            lifecycle_status: row.get(15)?,
            created_at_ms: row.get(16)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> ChunkStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        ChunkStore::new(Arc::new(Mutex::new(conn)))
    }

    fn make_chunk(owner: &str, id_suffix: &str, seq: u32) -> Chunk {
        Chunk {
            id: format!("chunk_{id_suffix}"),
            owner_id: owner.to_string(),
            user_id: String::new(),
            agent_id: "agent_1".to_string(),
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1".to_string(),
            source_ref: None,
            timestamp_ms: 1000 + seq as i64 * 1000,
            time_range_start_ms: 1000 + seq as i64 * 1000,
            time_range_end_ms: 2000 + seq as i64 * 1000,
            tags_json: "[]".to_string(),
            content: format!("Hello {id_suffix}"),
            token_count: 5,
            seq_in_source: seq,
            partial_message: false,
            lifecycle_status: "admitted".to_string(),
            created_at_ms: 1000,
        }
    }

    #[test]
    fn test_upsert_and_get() {
        let store = setup();
        let chunk = make_chunk("owner_1", "001", 0);
        store.upsert_chunks(std::slice::from_ref(&chunk)).unwrap();

        let got = store.get_chunk("owner_1", None, "chunk_001").unwrap();
        assert!(got.is_some());
        assert_eq!(got.unwrap().content, "Hello 001");
    }

    #[test]
    fn test_get_missing() {
        let store = setup();
        assert!(store.get_chunk("owner_1", None, "nope").unwrap().is_none());
    }

    #[test]
    fn test_owner_isolation() {
        let store = setup();
        let chunk = make_chunk("owner_1", "001", 0);
        store.upsert_chunks(&[chunk]).unwrap();

        // Different owner should not see owner_1's chunk
        assert!(store
            .get_chunk("owner_2", None, "chunk_001")
            .unwrap()
            .is_none());
    }

    #[test]
    fn test_get_chunk_user_isolation() {
        let store = setup();

        // alice's private chunk
        let mut alice = make_chunk("owner_1", "alice", 0);
        alice.user_id = "alice".to_string();
        store.upsert_chunks(&[alice]).unwrap();

        // bob cannot see alice's chunk; alice can
        assert!(store
            .get_chunk("owner_1", Some("bob"), "chunk_alice")
            .unwrap()
            .is_none());
        assert!(store
            .get_chunk("owner_1", Some("alice"), "chunk_alice")
            .unwrap()
            .is_some());

        // owner-shared chunk (user_id = "") is visible to every user
        let shared = make_chunk("owner_1", "shared", 0);
        store.upsert_chunks(&[shared]).unwrap();
        assert!(store
            .get_chunk("owner_1", Some("alice"), "chunk_shared")
            .unwrap()
            .is_some());
        assert!(store
            .get_chunk("owner_1", Some("bob"), "chunk_shared")
            .unwrap()
            .is_some());
    }

    #[test]
    fn test_list_chunks_with_filter() {
        let store = setup();
        let c1 = make_chunk("owner_1", "001", 0);
        let c2 = make_chunk("owner_1", "002", 1);
        store.upsert_chunks(&[c1, c2]).unwrap();

        let all = store
            .list_chunks("owner_1", None, None, None, None, 100)
            .unwrap();
        assert_eq!(all.len(), 2);

        let filtered = store
            .list_chunks("owner_1", None, Some(&SourceKind::Chat), None, None, 100)
            .unwrap();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_update_lifecycle() {
        let store = setup();
        let mut chunk = make_chunk("owner_1", "001", 0);
        chunk.lifecycle_status = "pending_extraction".to_string();
        store.upsert_chunks(&[chunk]).unwrap();

        store
            .update_lifecycle("owner_1", "chunk_001", "admitted")
            .unwrap();

        let got = store
            .get_chunk("owner_1", None, "chunk_001")
            .unwrap()
            .unwrap();
        assert_eq!(got.lifecycle_status, "admitted");
    }

    #[test]
    fn test_count_chunks() {
        let store = setup();
        store
            .upsert_chunks(&[
                make_chunk("owner_1", "001", 0),
                make_chunk("owner_1", "002", 1),
            ])
            .unwrap();

        assert_eq!(store.count_chunks("owner_1", None).unwrap(), 2);
        assert_eq!(store.count_chunks("owner_1", Some("admitted")).unwrap(), 2);
        assert_eq!(
            store
                .count_chunks("owner_1", Some("pending_extraction"))
                .unwrap(),
            0
        );
    }
}
