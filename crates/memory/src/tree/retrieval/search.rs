//! Fuzzy entity search over the entity index.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::CarrierResult;
use carrier_types::memory_tree::EntityMatch;

use crate::tree::entity_store::EntityStore;
use crate::tree::types::EntityKind;

const DEFAULT_LIMIT: usize = 5;
const MAX_LIMIT: usize = 100;

/// Search entities by substring match on canonical_id or surface form.
///
/// When `user_id` is `Some(u)`, only entities from `u`'s chunks or
/// owner-shared chunks are returned.
pub fn search_entities(
    conn: &Arc<Mutex<Connection>>,
    owner_id: &str,
    user_id: Option<&str>,
    query: &str,
    kind: Option<EntityKind>,
    limit: usize,
) -> CarrierResult<Vec<EntityMatch>> {
    let limit = if limit == 0 {
        DEFAULT_LIMIT
    } else {
        limit.min(MAX_LIMIT)
    };
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }

    let entity_store = EntityStore::new(conn.clone());
    entity_store.search_entities(owner_id, user_id, query, kind.as_ref(), limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;
    use crate::tree::entity_store::EntityIndexEntry;
    use tempfile::TempDir;

    fn setup() -> (Arc<Mutex<Connection>>, TempDir) {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let dir = TempDir::new().unwrap();
        (Arc::new(Mutex::new(conn)), dir)
    }

    #[test]
    fn test_empty_query_returns_empty() -> CarrierResult<()> {
        let (conn, _dir) = setup();
        let result = search_entities(&conn, "owner_1", None, "", None, 10)?;
        assert!(result.is_empty());
        Ok(())
    }

    #[test]
    fn test_search_after_index() -> CarrierResult<()> {
        let (conn, _dir) = setup();
        let entity_store = EntityStore::new(conn.clone());
        let entry = EntityIndexEntry {
            entity_id: "email:alice@example.com",
            node_id: "chunk_1",
            node_kind: "leaf",
            entity_kind: EntityKind::Email,
            surface: "alice@example.com",
            score: 0.5,
            timestamp_ms: 1_700_000_000_000,
            tree_id: None,
            user_id: "",
        };
        entity_store.upsert_entity_index("owner_1", &entry)?;

        let result = search_entities(&conn, "owner_1", None, "alice", None, 10)?;
        assert!(!result.is_empty());
        assert!(result[0].canonical_id.contains("alice"));
        Ok(())
    }
}
