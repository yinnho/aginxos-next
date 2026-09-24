//! Cross-source global digest retrieval.

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::CarrierResult;
use carrier_types::memory_tree::{NodeKind, QueryResponse, RetrievalHit, TreeKind};

use crate::tree::tree_global::GLOBAL_SCOPE;
use crate::tree::tree_store::TreeTreeStore;

const DEFAULT_LIMIT: usize = 10;

/// Query the global tree for a time window.
///
/// The global tree is a per-owner daily/weekly/monthly digest that is
/// intentionally cross-user by design, so it does NOT take a `user_id` filter
/// (unlike the other retrieval primitives). It is the only unfiltered path.
pub fn query_global(
    conn: &Arc<Mutex<Connection>>,
    owner_id: &str,
    time_window_days: Option<u32>,
    limit: usize,
) -> CarrierResult<QueryResponse> {
    let limit = if limit == 0 { DEFAULT_LIMIT } else { limit };
    let tree_store = TreeTreeStore::new(conn.clone());

    // Find or create the global tree (owner-shared: user_id = "").
    let global = tree_store.get_or_create_tree(owner_id, "", TreeKind::Global, GLOBAL_SCOPE)?;

    let mut hits: Vec<RetrievalHit> = Vec::new();

    // Walk all summary levels in the global tree (no user filter — cross-user).
    for level in 0..=global.max_level {
        let summaries = tree_store.list_summaries(owner_id, None, &global.id, Some(level), 100)?;
        for node in summaries {
            hits.push(RetrievalHit {
                node_id: node.id,
                node_kind: NodeKind::Summary,
                tree_id: global.id.clone(),
                tree_kind: TreeKind::Global,
                tree_scope: GLOBAL_SCOPE.to_string(),
                level,
                content: node.content,
                entities: node.entities,
                topics: node.topics,
                time_range_start_ms: node.time_range_start_ms,
                time_range_end_ms: node.time_range_end_ms,
                score: node.score,
                child_ids: node.child_ids,
                source_ref: None,
            });
        }
    }

    if let Some(days) = time_window_days {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let window_start_ms = now_ms - (days as i64 * 86_400_000);
        hits.retain(|h| h.time_range_end_ms >= window_start_ms && h.time_range_start_ms <= now_ms);
    }

    let total = hits.len();
    hits.sort_by_key(|h| std::cmp::Reverse(h.time_range_end_ms));
    hits.truncate(limit);

    Ok(QueryResponse {
        hits,
        total,
        truncated: total > limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;
    use tempfile::TempDir;

    fn setup() -> (Arc<Mutex<Connection>>, TempDir) {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let dir = TempDir::new().unwrap();
        (Arc::new(Mutex::new(conn)), dir)
    }

    #[test]
    fn test_empty_owner_returns_empty() -> CarrierResult<()> {
        let (conn, _dir) = setup();
        let resp = query_global(&conn, "owner_x", None, 10)?;
        assert!(resp.hits.is_empty());
        Ok(())
    }
}
