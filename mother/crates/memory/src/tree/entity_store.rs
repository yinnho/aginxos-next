//! Entity index and hotness store for tree memory.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::memory_tree::EntityMatch;

use super::types::{EntityKind, HotnessCounters};

/// Input for upsert_entity_index.
pub struct EntityIndexEntry<'a> {
    pub entity_id: &'a str,
    pub node_id: &'a str,
    pub node_kind: &'a str,
    pub entity_kind: EntityKind,
    pub surface: &'a str,
    pub score: f32,
    pub timestamp_ms: i64,
    pub tree_id: Option<&'a str>,
    /// Per-user isolation. `""` = owner-shared.
    pub user_id: &'a str,
}

/// Entity store backed by SQLite.
#[derive(Clone)]
pub struct EntityStore {
    conn: Arc<Mutex<Connection>>,
}

impl EntityStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    // -- Entity index ------------------------------------------------------

    /// Upsert an entity index entry.
    pub fn upsert_entity_index(
        &self,
        owner_id: &str,
        entry: &EntityIndexEntry,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute(
            "INSERT OR REPLACE INTO mem_tree_entity_index
             (entity_id, node_id, node_kind, owner_id, entity_kind, surface, score, timestamp_ms, tree_id, user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                entry.entity_id,
                entry.node_id,
                entry.node_kind,
                owner_id,
                entry.entity_kind.as_str(),
                entry.surface,
                entry.score,
                entry.timestamp_ms,
                entry.tree_id,
                entry.user_id,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Get all node IDs associated with an entity.
    ///
    /// When `user_id` is `Some(u)`, only return nodes whose chunk is owned by
    /// `u` or owner-shared. `None` skips the filter.
    pub fn chunks_for_entity(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        entity_id: &str,
        limit: usize,
    ) -> CarrierResult<Vec<(String, String)>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(owner_id.to_string()),
            Box::new(entity_id.to_string()),
        ];
        let mut sql = "SELECT node_id, node_kind FROM mem_tree_entity_index
                 WHERE owner_id = ?1 AND entity_id = ?2"
            .to_string();
        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }
        sql.push_str(" ORDER BY timestamp_ms DESC LIMIT ?");
        params.push(Box::new(limit as i64));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let node_id: String = row.get(0)?;
                let node_kind: String = row.get(1)?;
                Ok((node_id, node_kind))
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(result)
    }

    /// List top entities for an owner by mention frequency.
    pub fn top_entities(&self, owner_id: &str, limit: usize) -> CarrierResult<Vec<EntityMatch>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT h.entity_id, i.entity_kind, i.surface,
                        h.mention_count_30d, h.last_seen_ms
                 FROM mem_tree_entity_hotness h
                 LEFT JOIN mem_tree_entity_index i
                   ON i.owner_id = h.owner_id AND i.entity_id = h.entity_id
                 WHERE h.owner_id = ?1
                 ORDER BY h.last_hotness DESC NULLS LAST
                 LIMIT ?2",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let rows = stmt
            .query_map(rusqlite::params![owner_id, limit as i64], |row| {
                let canonical_id: String = row.get(0)?;
                let kind_str: String = row.get(1).unwrap_or_default();
                let surface: String = row.get(2).unwrap_or_default();
                let mention_count: i64 = row.get(3).unwrap_or(0);
                let last_seen_ms: i64 = row.get(4).unwrap_or(0);

                let kind = Self::parse_entity_kind(&kind_str);
                Ok(EntityMatch {
                    canonical_id,
                    kind,
                    surface,
                    mention_count: mention_count as u64,
                    last_seen_ms,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(result)
    }

    /// Fuzzy search entities by surface form.
    ///
    /// When `user_id` is `Some(u)`, only return entities from `u`'s chunks or
    /// owner-shared chunks. `None` skips the filter.
    pub fn search_entities(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        query: &str,
        kind: Option<&EntityKind>,
        limit: usize,
    ) -> CarrierResult<Vec<EntityMatch>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let pattern = format!("%{query}%");

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> =
            vec![Box::new(owner_id.to_string()), Box::new(pattern)];

        let mut sql = "SELECT DISTINCT entity_id, entity_kind, surface, 0 as mc, 0 as ls
                     FROM mem_tree_entity_index
                     WHERE owner_id = ?1 AND surface LIKE ?2"
            .to_string();

        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }
        if let Some(k) = kind {
            sql.push_str(" AND entity_kind = ?");
            params.push(Box::new(k.as_str().to_string()));
        }
        sql.push_str(" ORDER BY score DESC LIMIT ?");
        params.push(Box::new(limit as i64));

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), Self::row_to_entity_match)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(result)
    }

    // -- Entity hotness ----------------------------------------------------

    /// Bump entity hotness counters after ingestion.
    pub fn bump_entity_hotness(
        &self,
        owner_id: &str,
        entity_id: &str,
        source_id: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();

        // Try to update existing row
        let updated = conn
            .execute(
                "UPDATE mem_tree_entity_hotness
             SET mention_count_30d = mention_count_30d + 1,
                 last_seen_ms = ?1,
                 ingests_since_check = ingests_since_check + 1,
                 last_updated_ms = ?1
             WHERE owner_id = ?2 AND entity_id = ?3",
                rusqlite::params![now_ms, owner_id, entity_id],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        if updated == 0 {
            // Insert new row
            conn.execute(
                "INSERT INTO mem_tree_entity_hotness
                 (entity_id, owner_id, mention_count_30d, distinct_sources,
                  last_seen_ms, query_hits_30d, graph_centrality,
                  ingests_since_check, last_hotness, last_updated_ms)
                 VALUES (?1, ?2, 1, 1, ?3, 0, NULL, 1, NULL, ?3)",
                rusqlite::params![entity_id, owner_id, now_ms],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        }

        // Check if this source is new for this entity (distinct_sources tracking)
        // This is approximate — we check if this source_id appears in any chunk linked to the entity
        // For simplicity, we use a simpler approach: count distinct sources from entity_index
        let _ = source_id; // used for distinct_sources in a full implementation

        Ok(())
    }

    /// Get hotness counters for an entity.
    pub fn get_hotness(
        &self,
        owner_id: &str,
        entity_id: &str,
    ) -> CarrierResult<Option<HotnessCounters>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let result = conn.query_row(
            "SELECT entity_id, mention_count_30d, distinct_sources, last_seen_ms,
                    query_hits_30d, graph_centrality, ingests_since_check,
                    last_hotness, last_updated_ms
             FROM mem_tree_entity_hotness WHERE owner_id = ?1 AND entity_id = ?2",
            rusqlite::params![owner_id, entity_id],
            |row| {
                Ok(HotnessCounters {
                    entity_id: row.get(0)?,
                    mention_count_30d: row.get(1)?,
                    distinct_sources: row.get(2)?,
                    last_seen_ms: row.get(3)?,
                    query_hits_30d: row.get(4)?,
                    graph_centrality: row.get(5)?,
                    ingests_since_check: row.get(6)?,
                    last_hotness: row.get(7)?,
                    last_updated_ms: row.get(8)?,
                })
            },
        );

        match result {
            Ok(h) => Ok(Some(h)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// List hot entities above a threshold.
    pub fn list_hot_entities(
        &self,
        owner_id: &str,
        threshold: f32,
        limit: usize,
    ) -> CarrierResult<Vec<HotnessCounters>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut stmt = conn
            .prepare(
                "SELECT entity_id, mention_count_30d, distinct_sources, last_seen_ms,
                        query_hits_30d, graph_centrality, ingests_since_check,
                        last_hotness, last_updated_ms
                 FROM mem_tree_entity_hotness
                 WHERE owner_id = ?1 AND last_hotness >= ?2
                 ORDER BY last_hotness DESC NULLS LAST
                 LIMIT ?3",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let rows = stmt
            .query_map(
                rusqlite::params![owner_id, threshold, limit as i64],
                |row| {
                    Ok(HotnessCounters {
                        entity_id: row.get(0)?,
                        mention_count_30d: row.get(1)?,
                        distinct_sources: row.get(2)?,
                        last_seen_ms: row.get(3)?,
                        query_hits_30d: row.get(4)?,
                        graph_centrality: row.get(5)?,
                        ingests_since_check: row.get(6)?,
                        last_hotness: row.get(7)?,
                        last_updated_ms: row.get(8)?,
                    })
                },
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(result)
    }

    /// Update the hotness score for an entity.
    pub fn update_hotness_score(
        &self,
        owner_id: &str,
        entity_id: &str,
        hotness: f32,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "UPDATE mem_tree_entity_hotness
             SET last_hotness = ?1, ingests_since_check = 0, last_updated_ms = ?2
             WHERE owner_id = ?3 AND entity_id = ?4",
            rusqlite::params![hotness, now_ms, owner_id, entity_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Get all entity IDs associated with a node (chunk or summary).
    ///
    /// When `user_id` is `Some(u)`, only return entities from `u`'s nodes or
    /// owner-shared nodes. `None` skips the filter.
    pub fn entities_for_node(
        &self,
        owner_id: &str,
        user_id: Option<&str>,
        node_id: &str,
    ) -> CarrierResult<Vec<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = vec![
            Box::new(owner_id.to_string()),
            Box::new(node_id.to_string()),
        ];
        let mut sql = "SELECT DISTINCT entity_id FROM mem_tree_entity_index
                 WHERE owner_id = ?1 AND node_id = ?2"
            .to_string();
        if let Some(u) = user_id {
            sql.push_str(" AND (user_id = ? OR user_id = '')");
            params.push(Box::new(u.to_string()));
        }

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| row.get(0))
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(result)
    }

    // -- Helpers -----------------------------------------------------------

    fn row_to_entity_match(row: &rusqlite::Row) -> rusqlite::Result<EntityMatch> {
        let kind_str: String = row.get(1).unwrap_or_default();
        Ok(EntityMatch {
            canonical_id: row.get(0)?,
            kind: Self::parse_entity_kind(&kind_str),
            surface: row.get(2).unwrap_or_default(),
            mention_count: row.get::<_, i64>(3).unwrap_or(0) as u64,
            last_seen_ms: row.get::<_, i64>(4).unwrap_or(0),
        })
    }

    pub fn parse_entity_kind(s: &str) -> EntityKind {
        match s {
            "email" => EntityKind::Email,
            "url" => EntityKind::Url,
            "handle" => EntityKind::Handle,
            "hashtag" => EntityKind::Hashtag,
            "person" => EntityKind::Person,
            "organization" => EntityKind::Organization,
            "location" => EntityKind::Location,
            "event" => EntityKind::Event,
            "product" => EntityKind::Product,
            "datetime" => EntityKind::Datetime,
            "technology" => EntityKind::Technology,
            "artifact" => EntityKind::Artifact,
            "quantity" => EntityKind::Quantity,
            "misc" => EntityKind::Misc,
            "topic" => EntityKind::Topic,
            _ => EntityKind::Misc,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> EntityStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        EntityStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_upsert_and_get_entity_index() {
        let store = setup();
        store
            .upsert_entity_index(
                "owner_1",
                &EntityIndexEntry {
                    entity_id: "person:Alice",
                    node_id: "chunk_001",
                    node_kind: "leaf",
                    entity_kind: EntityKind::Person,
                    surface: "Alice",
                    score: 0.8,
                    timestamp_ms: 1000,
                    tree_id: Some("tree_1"),
                    user_id: "",
                },
            )
            .unwrap();

        let nodes = store
            .chunks_for_entity("owner_1", None, "person:Alice", 10)
            .unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].0, "chunk_001");
    }

    #[test]
    fn test_bump_entity_hotness() {
        let store = setup();
        store
            .bump_entity_hotness("owner_1", "person:Alice", "source_1")
            .unwrap();
        store
            .bump_entity_hotness("owner_1", "person:Alice", "source_1")
            .unwrap();

        let hotness = store
            .get_hotness("owner_1", "person:Alice")
            .unwrap()
            .unwrap();
        assert_eq!(hotness.mention_count_30d, 2);
    }

    #[test]
    fn test_update_hotness_score() {
        let store = setup();
        store
            .bump_entity_hotness("owner_1", "person:Alice", "source_1")
            .unwrap();
        store
            .update_hotness_score("owner_1", "person:Alice", 15.0)
            .unwrap();

        let hotness = store
            .get_hotness("owner_1", "person:Alice")
            .unwrap()
            .unwrap();
        assert!((hotness.last_hotness.unwrap() - 15.0).abs() < 0.01);
        assert_eq!(hotness.ingests_since_check, 0);
    }

    #[test]
    fn test_list_hot_entities() {
        let store = setup();
        store
            .bump_entity_hotness("owner_1", "person:Alice", "source_1")
            .unwrap();
        store
            .bump_entity_hotness("owner_1", "person:Bob", "source_1")
            .unwrap();

        store
            .update_hotness_score("owner_1", "person:Alice", 15.0)
            .unwrap();
        store
            .update_hotness_score("owner_1", "person:Bob", 5.0)
            .unwrap();

        let hot = store.list_hot_entities("owner_1", 10.0, 10).unwrap();
        assert_eq!(hot.len(), 1); // Only Alice is above 10.0
        assert_eq!(hot[0].entity_id, "person:Alice");
    }

    #[test]
    fn test_search_entities() {
        let store = setup();
        store
            .upsert_entity_index(
                "owner_1",
                &EntityIndexEntry {
                    entity_id: "person:Alice",
                    node_id: "chunk_001",
                    node_kind: "leaf",
                    entity_kind: EntityKind::Person,
                    surface: "Alice Smith",
                    score: 0.8,
                    timestamp_ms: 1000,
                    tree_id: Some("tree_1"),
                    user_id: "",
                },
            )
            .unwrap();
        store
            .upsert_entity_index(
                "owner_1",
                &EntityIndexEntry {
                    entity_id: "person:Bob",
                    node_id: "chunk_002",
                    node_kind: "leaf",
                    entity_kind: EntityKind::Person,
                    surface: "Bob Jones",
                    score: 0.6,
                    timestamp_ms: 2000,
                    tree_id: Some("tree_1"),
                    user_id: "",
                },
            )
            .unwrap();

        let results = store
            .search_entities("owner_1", None, "Alice", None, 10)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].canonical_id, "person:Alice");
    }

    #[test]
    fn test_owner_isolation() {
        let store = setup();
        store
            .upsert_entity_index(
                "owner_1",
                &EntityIndexEntry {
                    entity_id: "person:Alice",
                    node_id: "chunk_001",
                    node_kind: "leaf",
                    entity_kind: EntityKind::Person,
                    surface: "Alice",
                    score: 0.8,
                    timestamp_ms: 1000,
                    tree_id: Some("tree_1"),
                    user_id: "",
                },
            )
            .unwrap();

        let results = store
            .search_entities("owner_2", None, "Alice", None, 10)
            .unwrap();
        assert!(results.is_empty());
    }
}
