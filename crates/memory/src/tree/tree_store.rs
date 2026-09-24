//! Tree, SummaryNode, and Buffer CRUD operations.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::memory_tree::{TreeKind, TreeSummary};

use super::types::{Buffer, SummaryNode, Tree, TreeStatus};

/// Tree + summary + buffer store backed by SQLite.
#[derive(Clone)]
pub struct TreeTreeStore {
    conn: Arc<Mutex<Connection>>,
}

impl TreeTreeStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    // -- Tree operations ---------------------------------------------------

    /// Get or create a tree for (owner_id, kind, scope). Returns the tree.
    ///
    /// `user_id` is persisted on creation (source trees carry the sender;
    /// topic/global trees pass `""` for owner-shared). The lookup itself keys on
    /// (owner_id, kind, scope) — source-tree scopes already encode the sender.
    pub fn get_or_create_tree(
        &self,
        owner_id: &str,
        user_id: &str,
        kind: TreeKind,
        scope: &str,
    ) -> CarrierResult<Tree> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        // Try to find existing
        let result = conn.query_row(
            "SELECT id, owner_id, kind, scope, root_id, max_level, status, created_at_ms, last_sealed_at_ms, user_id
             FROM mem_tree_trees WHERE owner_id = ?1 AND kind = ?2 AND scope = ?3",
            rusqlite::params![owner_id, kind.as_str(), scope],
            Self::row_to_tree,
        );

        match result {
            Ok(tree) => Ok(tree),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                let now_ms = chrono::Utc::now().timestamp_millis();
                let id = format!("tree_{}", uuid::Uuid::new_v4().simple());

                conn.execute(
                    "INSERT INTO mem_tree_trees (id, owner_id, user_id, kind, scope, root_id, max_level, status, created_at_ms, last_sealed_at_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, NULL, 0, 'active', ?6, NULL)",
                    rusqlite::params![id, owner_id, user_id, kind.as_str(), scope, now_ms],
                )
                .map_err(|e| CarrierError::Memory(e.to_string()))?;

                Ok(Tree {
                    id,
                    owner_id: owner_id.to_string(),
                    user_id: user_id.to_string(),
                    kind,
                    scope: scope.to_string(),
                    root_id: None,
                    max_level: 0,
                    status: TreeStatus::Active,
                    created_at_ms: now_ms,
                    last_sealed_at_ms: None,
                })
            }
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Get a tree by ID.
    ///
    /// When `user_id` is `Some(u)`, only return the tree if it belongs to `u`
    /// or is owner-shared (`user_id = ''`). `None` skips the filter.
    pub fn get_tree(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        tree_id: &str,
    ) -> CarrierResult<Option<Tree>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(owner_id.to_string()),
            Box::new(tree_id.to_string()),
        ];
        let mut sql = "SELECT id, owner_id, kind, scope, root_id, max_level, status, created_at_ms, last_sealed_at_ms, user_id
             FROM mem_tree_trees WHERE owner_id = ?1 AND id = ?2".to_string();
        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let result = conn.query_row(&sql, param_refs.as_slice(), Self::row_to_tree);

        match result {
            Ok(tree) => Ok(Some(tree)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// List all trees for an owner, optionally filtered by user and kind.
    pub fn list_trees(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        kind: Option<TreeKind>,
        limit: usize,
    ) -> CarrierResult<Vec<TreeSummary>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(owner_id.to_string())];

        let mut sql = "SELECT t.id, t.kind, t.scope, t.status, t.max_level,
                               0 as chunk_count,
                               COALESCE(s.cnt, 0) as summary_count,
                               t.last_sealed_at_ms
                        FROM mem_tree_trees t
                        LEFT JOIN (SELECT tree_id, COUNT(*) as cnt FROM mem_tree_summaries WHERE owner_id = ?1 AND deleted = 0 GROUP BY tree_id) s ON s.tree_id = t.id
                        WHERE t.owner_id = ?1".to_string();

        if let Some(u) = user_id {
            sql.push_str(" AND (t.user_id = ? OR t.user_id = '')");
            params.push(Box::new(u.to_string()));
        }
        if let Some(k) = kind {
            sql.push_str(" AND t.kind = ?");
            params.push(Box::new(k.as_str().to_string()));
        }
        sql.push_str(" ORDER BY t.created_at_ms DESC LIMIT ?");
        params.push(Box::new(limit as i64));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), Self::row_to_tree_summary)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut trees = Vec::new();
        for row in rows {
            trees.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(trees)
    }

    /// Update tree max_level, root_id and last_sealed_at_ms after a seal.
    pub fn update_tree_after_seal(
        &self,
        owner_id: &str,
        tree_id: &str,
        new_max_level: u32,
        sealed_at_ms: i64,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute(
            "UPDATE mem_tree_trees SET max_level = ?1, last_sealed_at_ms = ?2,
             root_id = COALESCE(root_id, (
                SELECT id FROM mem_tree_summaries
                WHERE owner_id = ?3 AND tree_id = ?4 AND deleted = 0
                ORDER BY level DESC, sealed_at_ms DESC LIMIT 1
             ))
             WHERE owner_id = ?3 AND id = ?4",
            rusqlite::params![new_max_level, sealed_at_ms, owner_id, tree_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    // -- Summary operations ------------------------------------------------

    /// Insert a summary node.
    pub fn insert_summary(&self, owner_id: &str, summary: &SummaryNode) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let child_ids_json = serde_json::to_string(&summary.child_ids)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let entities_json = serde_json::to_string(&summary.entities)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let topics_json = serde_json::to_string(&summary.topics)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO mem_tree_summaries
             (id, owner_id, user_id, tree_id, tree_kind, level, parent_id, child_ids_json,
              content, token_count, entities_json, topics_json,
              time_range_start_ms, time_range_end_ms, score, sealed_at_ms, deleted, embedding)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            rusqlite::params![
                summary.id,
                owner_id,
                summary.user_id,
                summary.tree_id,
                summary.tree_kind.as_str(),
                summary.level,
                summary.parent_id,
                child_ids_json,
                summary.content,
                summary.token_count,
                entities_json,
                topics_json,
                summary.time_range_start_ms,
                summary.time_range_end_ms,
                summary.score,
                summary.sealed_at_ms,
                summary.deleted as i32,
                summary.embedding.as_ref().map(|e| serde_json::to_vec(e).unwrap()),
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Get a summary node by ID.
    ///
    /// When `user_id` is `Some(u)`, enforce per-user isolation (own summary or
    /// owner-shared). `None` skips the filter (write-path lookups by exact id).
    pub fn get_summary(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        summary_id: &str,
    ) -> CarrierResult<Option<SummaryNode>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(owner_id.to_string()),
            Box::new(summary_id.to_string()),
        ];
        let mut sql = "SELECT id, tree_id, tree_kind, level, parent_id, child_ids_json,
                    content, token_count, entities_json, topics_json,
                    time_range_start_ms, time_range_end_ms, score, sealed_at_ms, deleted, embedding, user_id
             FROM mem_tree_summaries WHERE owner_id = ?1 AND id = ?2".to_string();
        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        let result = conn.query_row(&sql, param_refs.as_slice(), Self::row_to_summary);

        match result {
            Ok(s) => Ok(Some(s)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// List summary nodes for a tree at a given level.
    ///
    /// When `user_id` is `Some(u)`, only return summaries belonging to `u` or
    /// owner-shared. `None` skips the filter.
    pub fn list_summaries(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        tree_id: &str,
        level: Option<u32>,
        limit: usize,
    ) -> CarrierResult<Vec<SummaryNode>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(owner_id.to_string()),
            Box::new(tree_id.to_string()),
        ];

        let mut sql = "SELECT id, tree_id, tree_kind, level, parent_id, child_ids_json,
                              content, token_count, entities_json, topics_json,
                              time_range_start_ms, time_range_end_ms, score, sealed_at_ms, deleted, embedding, user_id
                       FROM mem_tree_summaries
                       WHERE owner_id = ?1 AND tree_id = ?2 AND deleted = 0".to_string();

        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }
        if let Some(l) = level {
            sql.push_str(" AND level = ?");
            params.push(Box::new(l));
        }
        sql.push_str(" ORDER BY sealed_at_ms ASC LIMIT ?");
        params.push(Box::new(limit as i64));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), Self::row_to_summary)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(summaries)
    }

    /// Soft-delete a summary node.
    pub fn delete_summary(&self, owner_id: &str, summary_id: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute(
            "UPDATE mem_tree_summaries SET deleted = 1 WHERE owner_id = ?1 AND id = ?2",
            rusqlite::params![owner_id, summary_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    // -- Buffer operations -------------------------------------------------

    /// Get the buffer for a tree at a given level.
    pub fn get_buffer(
        &self,
        owner_id: &str,
        tree_id: &str,
        level: u32,
    ) -> CarrierResult<Option<Buffer>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let result = conn.query_row(
            "SELECT tree_id, level, item_ids_json, token_sum, oldest_at_ms
             FROM mem_tree_buffers WHERE owner_id = ?1 AND tree_id = ?2 AND level = ?3",
            rusqlite::params![owner_id, tree_id, level],
            |row| {
                let tree_id: String = row.get(0)?;
                let level: u32 = row.get(1)?;
                let item_ids_json: String = row.get(2)?;
                let token_sum: i64 = row.get(3)?;
                let oldest_at_ms: Option<i64> = row.get(4)?;

                let item_ids: Vec<String> =
                    serde_json::from_str(&item_ids_json).unwrap_or_default();

                Ok(Buffer {
                    tree_id,
                    level,
                    item_ids,
                    token_sum,
                    oldest_at_ms,
                })
            },
        );

        match result {
            Ok(buf) => Ok(Some(buf)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Upsert a buffer (insert or replace).
    pub fn upsert_buffer(&self, owner_id: &str, buffer: &Buffer) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let item_ids_json = serde_json::to_string(&buffer.item_ids)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let now_ms = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT OR REPLACE INTO mem_tree_buffers
             (tree_id, level, owner_id, item_ids_json, token_sum, oldest_at_ms, updated_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                buffer.tree_id,
                buffer.level,
                owner_id,
                item_ids_json,
                buffer.token_sum,
                buffer.oldest_at_ms,
                now_ms,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Clear a buffer (remove all items) after a seal.
    pub fn clear_buffer(&self, owner_id: &str, tree_id: &str, level: u32) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE mem_tree_buffers SET item_ids_json = '[]', token_sum = 0, oldest_at_ms = NULL, updated_at_ms = ?1
             WHERE owner_id = ?2 AND tree_id = ?3 AND level = ?4",
            rusqlite::params![now_ms, owner_id, tree_id, level],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// List buffers with items older than the cutoff timestamp.
    pub fn list_stale_buffers(&self, owner_id: &str, cutoff_ms: i64) -> CarrierResult<Vec<Buffer>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT tree_id, level, item_ids_json, token_sum, oldest_at_ms
                 FROM mem_tree_buffers
                 WHERE owner_id = ?1
                   AND oldest_at_ms IS NOT NULL
                   AND oldest_at_ms <= ?2
                   AND item_ids_json != '[]'",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![owner_id, cutoff_ms], |row| {
                let tree_id: String = row.get(0)?;
                let level: u32 = row.get(1)?;
                let item_ids_json: String = row.get(2)?;
                let token_sum: i64 = row.get(3)?;
                let oldest_at_ms: Option<i64> = row.get(4)?;

                let item_ids: Vec<String> =
                    serde_json::from_str(&item_ids_json).unwrap_or_default();

                Ok(Buffer {
                    tree_id,
                    level,
                    item_ids,
                    token_sum,
                    oldest_at_ms,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(result)
    }

    // -- Row mappers -------------------------------------------------------

    fn row_to_tree(row: &rusqlite::Row) -> rusqlite::Result<Tree> {
        let kind_str: String = row.get(2)?;
        let kind = match kind_str.as_str() {
            "source" => TreeKind::Source,
            "topic" => TreeKind::Topic,
            "global" => TreeKind::Global,
            _ => TreeKind::Source,
        };
        let status_str: String = row.get(6)?;
        let status = match status_str.as_str() {
            "archived" => TreeStatus::Archived,
            _ => TreeStatus::Active,
        };

        Ok(Tree {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            kind,
            scope: row.get(3)?,
            root_id: row.get(4)?,
            max_level: row.get(5)?,
            status,
            created_at_ms: row.get(7)?,
            last_sealed_at_ms: row.get(8)?,
            user_id: row.get(9)?,
        })
    }

    fn row_to_summary(row: &rusqlite::Row) -> rusqlite::Result<SummaryNode> {
        let tree_kind_str: String = row.get(2)?;
        let tree_kind = match tree_kind_str.as_str() {
            "source" => TreeKind::Source,
            "topic" => TreeKind::Topic,
            "global" => TreeKind::Global,
            _ => TreeKind::Source,
        };
        let child_ids_json: String = row.get(5)?;
        let entities_json: String = row.get(8)?;
        let topics_json: String = row.get(9)?;
        let sealed_at_ms: i64 = row.get(13)?;
        let deleted: i32 = row.get(14)?;
        let embedding_blob: Option<Vec<u8>> = row.get(15)?;

        let embedding = embedding_blob.and_then(|b| serde_json::from_slice::<Vec<f32>>(&b).ok());

        Ok(SummaryNode {
            id: row.get(0)?,
            tree_id: row.get(1)?,
            tree_kind,
            level: row.get(3)?,
            parent_id: row.get(4)?,
            child_ids: serde_json::from_str(&child_ids_json).unwrap_or_default(),
            content: row.get(6)?,
            token_count: row.get(7)?,
            entities: serde_json::from_str(&entities_json).unwrap_or_default(),
            topics: serde_json::from_str(&topics_json).unwrap_or_default(),
            time_range_start_ms: row.get(10)?,
            time_range_end_ms: row.get(11)?,
            score: row.get(12)?,
            sealed_at_ms,
            deleted: deleted != 0,
            embedding,
            user_id: row.get(16)?,
        })
    }

    fn row_to_tree_summary(row: &rusqlite::Row) -> rusqlite::Result<TreeSummary> {
        Ok(TreeSummary {
            tree_id: row.get(0)?,
            kind: row.get(1)?,
            scope: row.get(2)?,
            status: row.get(3)?,
            max_level: row.get(4)?,
            chunk_count: row.get(5)?,
            summary_count: row.get(6)?,
            last_sealed_at_ms: row.get(7)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> TreeTreeStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        TreeTreeStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_get_or_create_tree() {
        let store = setup();
        let tree = store
            .get_or_create_tree("owner_1", "", TreeKind::Source, "wechat:gh_abc:sender_1")
            .unwrap();
        assert_eq!(tree.owner_id, "owner_1");
        assert_eq!(tree.kind, TreeKind::Source);
        assert_eq!(tree.scope, "wechat:gh_abc:sender_1");
        assert_eq!(tree.status, TreeStatus::Active);

        // Same call returns same tree
        let tree2 = store
            .get_or_create_tree("owner_1", "", TreeKind::Source, "wechat:gh_abc:sender_1")
            .unwrap();
        assert_eq!(tree.id, tree2.id);
    }

    #[test]
    fn test_list_trees() {
        let store = setup();
        store
            .get_or_create_tree("owner_1", "", TreeKind::Source, "source_1")
            .unwrap();
        store
            .get_or_create_tree("owner_1", "", TreeKind::Source, "source_2")
            .unwrap();
        store
            .get_or_create_tree("owner_1", "", TreeKind::Global, "global")
            .unwrap();

        let all = store.list_trees("owner_1", None, None, 100).unwrap();
        assert_eq!(all.len(), 3);

        let sources = store
            .list_trees("owner_1", None, Some(TreeKind::Source), 100)
            .unwrap();
        assert_eq!(sources.len(), 2);

        // Different owner sees nothing
        let empty = store.list_trees("owner_2", None, None, 100).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_buffer_upsert_and_get() {
        let store = setup();
        let tree = store
            .get_or_create_tree("owner_1", "", TreeKind::Source, "source_1")
            .unwrap();

        let buf = Buffer {
            tree_id: tree.id.clone(),
            level: 0,
            item_ids: vec!["chunk_1".to_string(), "chunk_2".to_string()],
            token_sum: 1500,
            oldest_at_ms: Some(1000),
        };
        store.upsert_buffer("owner_1", &buf).unwrap();

        let got = store.get_buffer("owner_1", &tree.id, 0).unwrap().unwrap();
        assert_eq!(got.item_ids.len(), 2);
        assert_eq!(got.token_sum, 1500);
    }

    #[test]
    fn test_buffer_clear() {
        let store = setup();
        let tree = store
            .get_or_create_tree("owner_1", "", TreeKind::Source, "source_1")
            .unwrap();

        let buf = Buffer {
            tree_id: tree.id.clone(),
            level: 0,
            item_ids: vec!["chunk_1".to_string()],
            token_sum: 500,
            oldest_at_ms: Some(1000),
        };
        store.upsert_buffer("owner_1", &buf).unwrap();
        store.clear_buffer("owner_1", &tree.id, 0).unwrap();

        let got = store.get_buffer("owner_1", &tree.id, 0).unwrap().unwrap();
        assert!(got.item_ids.is_empty());
        assert_eq!(got.token_sum, 0);
    }

    #[test]
    fn test_insert_and_get_summary() {
        let store = setup();
        let tree = store
            .get_or_create_tree("owner_1", "", TreeKind::Source, "source_1")
            .unwrap();

        let summary = SummaryNode {
            id: "sum_001".to_string(),
            tree_id: tree.id.clone(),
            user_id: String::new(),
            tree_kind: TreeKind::Source,
            level: 1,
            parent_id: None,
            child_ids: vec!["chunk_1".to_string()],
            content: "Summary of conversation".to_string(),
            token_count: 50,
            entities: vec!["person:Alice".to_string()],
            topics: vec!["project-phoenix".to_string()],
            time_range_start_ms: 1000,
            time_range_end_ms: 5000,
            score: 0.85,
            sealed_at_ms: 6000,
            deleted: false,
            embedding: None,
        };
        store.insert_summary("owner_1", &summary).unwrap();

        let got = store
            .get_summary("owner_1", None, "sum_001")
            .unwrap()
            .unwrap();
        assert_eq!(got.content, "Summary of conversation");
        assert_eq!(got.entities.len(), 1);
    }
}
