//! Walk summary children (BFS expansion).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::CarrierResult;
use carrier_types::memory_tree::{NodeKind, RetrievalHit, TreeKind};

use crate::tree::store::ChunkStore;
use crate::tree::tree_store::TreeTreeStore;

/// Drill down from a summary node, returning its children.
/// BFS traversal up to `max_depth` levels.
///
/// When `user_id` is `Some(u)`, only nodes belonging to `u` or owner-shared
/// are traversed.
pub fn drill_down(
    conn: &Arc<Mutex<Connection>>,
    owner_id: &str,
    user_id: Option<&str>,
    node_id: &str,
    max_depth: u32,
    limit: Option<usize>,
) -> CarrierResult<Vec<RetrievalHit>> {
    if max_depth == 0 {
        return Ok(Vec::new());
    }

    let tree_store = TreeTreeStore::new(conn.clone());
    let chunk_store = ChunkStore::new(conn.clone());

    // Get the root summary to find its children
    let root = tree_store.get_summary(owner_id, user_id, node_id)?;
    let start_children: Vec<String> = match root {
        Some(ref s) => s.child_ids.clone(),
        None => {
            // It's a leaf — no children
            if chunk_store.get_chunk(owner_id, user_id, node_id)?.is_some() {
                return Ok(Vec::new());
            }
            return Ok(Vec::new());
        }
    };

    let root_tree_scope = root
        .as_ref()
        .and_then(|s| {
            tree_store
                .get_tree(owner_id, user_id, &s.tree_id)
                .ok()
                .flatten()
                .map(|t| t.scope)
        })
        .unwrap_or_default();

    let mut hits: Vec<RetrievalHit> = Vec::new();
    let mut frontier: VecDeque<(String, u32)> =
        start_children.into_iter().map(|id| (id, 1u32)).collect();

    while let Some((id, depth)) = frontier.pop_front() {
        if depth > max_depth {
            continue;
        }

        // Try as summary
        if let Some(node) = tree_store.get_summary(owner_id, user_id, &id)? {
            let scope = tree_store
                .get_tree(owner_id, user_id, &node.tree_id)?
                .map(|t| t.scope)
                .unwrap_or_else(|| root_tree_scope.clone());
            let child_ids = node.child_ids.clone();
            hits.push(RetrievalHit {
                node_id: node.id,
                node_kind: NodeKind::Summary,
                tree_id: node.tree_id,
                tree_kind: node.tree_kind,
                tree_scope: scope,
                level: node.level,
                content: node.content,
                entities: node.entities,
                topics: node.topics,
                time_range_start_ms: node.time_range_start_ms,
                time_range_end_ms: node.time_range_end_ms,
                score: node.score,
                child_ids: node.child_ids,
                source_ref: None,
            });
            if depth < max_depth {
                for next in child_ids {
                    frontier.push_back((next, depth + 1));
                }
            }
            continue;
        }

        // Try as chunk (leaf)
        if let Some(chunk) = chunk_store.get_chunk(owner_id, user_id, &id)? {
            hits.push(RetrievalHit {
                node_id: chunk.id,
                node_kind: NodeKind::Leaf,
                tree_id: String::new(),
                tree_kind: TreeKind::Source,
                tree_scope: chunk.source_id.clone(),
                level: 0,
                content: chunk.content,
                entities: Vec::new(),
                topics: Vec::new(),
                time_range_start_ms: chunk.time_range_start_ms,
                time_range_end_ms: chunk.time_range_end_ms,
                score: 0.0,
                child_ids: Vec::new(),
                source_ref: chunk.source_ref,
            });
        }
    }

    if let Some(n) = limit {
        hits.truncate(n);
    }

    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;
    use crate::tree::bucket_seal::BucketSealEngine;
    use crate::tree::store::ChunkStore;
    use crate::tree::summariser::inert::InertSummariser;
    use crate::tree::types::{Chunk, SourceKind};
    use tempfile::TempDir;

    fn setup() -> (Arc<Mutex<Connection>>, TempDir) {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let dir = TempDir::new().unwrap();
        (Arc::new(Mutex::new(conn)), dir)
    }

    #[test]
    fn test_depth_zero_returns_empty() -> CarrierResult<()> {
        let (conn, _dir) = setup();
        let result = drill_down(&conn, "owner_1", None, "nonexistent", 0, None)?;
        assert!(result.is_empty());
        Ok(())
    }

    #[test]
    fn test_invalid_id_returns_empty() -> CarrierResult<()> {
        let (conn, _dir) = setup();
        let result = drill_down(&conn, "owner_1", None, "nonexistent", 1, None)?;
        assert!(result.is_empty());
        Ok(())
    }

    #[test]
    fn test_drill_from_sealed_tree() -> CarrierResult<()> {
        let (conn, _dir) = setup();
        let tree_store = TreeTreeStore::new(conn.clone());
        let chunk_store = ChunkStore::new(conn.clone());

        let tree =
            tree_store.get_or_create_tree("owner_1", "", TreeKind::Source, "wechat:test:sender")?;

        // Insert enough chunks and force seal
        for i in 0..10 {
            let chunk = Chunk {
                id: format!("chunk_dd_{i}"),
                owner_id: "owner_1".to_string(),
                user_id: String::new(),
                agent_id: "agent_1".to_string(),
                source_kind: SourceKind::Chat,
                source_id: "wechat:test:sender".to_string(),
                source_ref: None,
                timestamp_ms: 1_700_000_000_000,
                time_range_start_ms: 1_700_000_000_000,
                time_range_end_ms: 1_700_000_000_000,
                tags_json: "[]".to_string(),
                content: "test content for drill down".to_string(),
                token_count: 6000,
                seq_in_source: i,
                partial_message: false,
                lifecycle_status: "admitted".to_string(),
                created_at_ms: 1_700_000_000_000,
            };
            chunk_store.upsert_chunks(&[chunk])?;
        }

        let seal_engine = BucketSealEngine::new(
            conn.clone(),
            _dir.path().to_path_buf(),
            Arc::new(InertSummariser),
        );
        for i in 0..10 {
            seal_engine.append_to_buffer(
                "owner_1",
                &tree.id,
                0,
                &format!("chunk_dd_{i}"),
                6000,
                1_700_000_000_000,
            )?;
        }
        seal_engine.cascade_seals("owner_1", &tree, 0, false)?;

        // Get the root summary
        let refreshed = tree_store.get_tree("owner_1", None, &tree.id)?.unwrap();
        let root_id = refreshed.root_id.unwrap();

        let result = drill_down(&conn, "owner_1", None, &root_id, 1, None)?;
        assert!(
            !result.is_empty(),
            "drill_down from sealed L1 should return leaf children"
        );
        Ok(())
    }
}
