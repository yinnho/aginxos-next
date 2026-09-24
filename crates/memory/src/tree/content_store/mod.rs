//! Content store for Obsidian-compatible .md file storage.
//!
//! Provides atomic writes, path generation, frontmatter composition, and
//! read/parse operations for chunk and summary .md files.

pub mod atomic;
pub mod compose;
pub mod paths;
pub mod read;

use std::path::PathBuf;

use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::memory_tree::TreeKind;

use super::types::{Chunk, SummaryNode};

/// Content store managing .md files on disk.
#[derive(Clone)]
pub struct ContentStore {
    content_root: PathBuf,
}

impl ContentStore {
    pub fn new(content_root: PathBuf) -> Self {
        Self { content_root }
    }

    /// Ensure the content root directory and .obsidian directory exist.
    pub fn ensure_dirs(&self, owner_id: &str) -> CarrierResult<()> {
        let owner_root = self.owner_root(owner_id);
        fs::create_dir_all(&owner_root)
            .map_err(|e| CarrierError::Internal(format!("mkdir {}: {e}", owner_root.display())))?;

        let obsidian_dir = owner_root.join(".obsidian");
        fs::create_dir_all(&obsidian_dir).map_err(|e| {
            CarrierError::Internal(format!("mkdir {}: {e}", obsidian_dir.display()))
        })?;

        Ok(())
    }

    /// Write a chunk to disk as an Obsidian-compatible .md file.
    pub fn write_chunk(&self, owner_id: &str, chunk: &Chunk) -> CarrierResult<bool> {
        let path = paths::chunk_abs_path(
            &self.owner_root(owner_id),
            chunk.source_kind.as_str(),
            &chunk.source_id,
            &chunk.id,
        );

        let tags: Vec<String> = serde_json::from_str(&chunk.tags_json).unwrap_or_default();
        let md = compose::compose_chunk_md(&compose::ChunkMdInput {
            source_kind: chunk.source_kind.as_str(),
            source_id: &chunk.source_id,
            owner_id: &chunk.owner_id,
            seq: chunk.seq_in_source,
            timestamp_ms: chunk.timestamp_ms,
            time_range_start_ms: chunk.time_range_start_ms,
            time_range_end_ms: chunk.time_range_end_ms,
            tags: &tags,
            body: &chunk.content,
        });

        atomic::write_if_new(&path, &md)
    }

    /// Write a summary node to disk as an Obsidian-compatible .md file.
    pub fn write_summary(&self, owner_id: &str, summary: &SummaryNode) -> CarrierResult<bool> {
        let scope_slug = paths::slugify_source_id(
            &self.scope_from_tree_kind(&summary.tree_kind, &summary.tree_id),
        );

        let path = paths::summary_abs_path(
            &self.owner_root(owner_id),
            summary.tree_kind.as_str(),
            &scope_slug,
            summary.level,
            &summary.id,
        );

        let md = compose::compose_summary_md(&compose::SummaryMdInput {
            summary_id: &summary.id,
            tree_kind: summary.tree_kind.as_str(),
            tree_id: &summary.tree_id,
            tree_scope: "", // scope passed via path
            level: summary.level,
            child_ids: &summary.child_ids,
            entities: &summary.entities,
            topics: &summary.topics,
            time_range_start_ms: summary.time_range_start_ms,
            time_range_end_ms: summary.time_range_end_ms,
            sealed_at_ms: summary.sealed_at_ms,
            body: &summary.content,
        });

        atomic::write_if_new(&path, &md)
    }

    /// Read the body content of a chunk.
    pub fn read_chunk_body(
        &self,
        owner_id: &str,
        source_kind: &str,
        source_id: &str,
        chunk_id: &str,
    ) -> CarrierResult<String> {
        let path =
            paths::chunk_abs_path(&self.owner_root(owner_id), source_kind, source_id, chunk_id);
        read::read_body(&path)
    }

    /// Get the content root for a specific owner.
    fn owner_root(&self, owner_id: &str) -> PathBuf {
        self.content_root.join(owner_id)
    }

    /// Derive a scope string from tree_kind and tree_id.
    fn scope_from_tree_kind(&self, kind: &TreeKind, tree_id: &str) -> String {
        match kind {
            TreeKind::Global => "global".to_string(),
            _ => tree_id.to_string(),
        }
    }
}

use std::fs;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    use crate::tree::types::SourceKind;

    fn setup() -> (ContentStore, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let store = ContentStore::new(root.clone());
        (store, root)
    }

    fn make_chunk() -> Chunk {
        Chunk {
            id: "chunk_001".to_string(),
            owner_id: "owner_1".to_string(),
            user_id: String::new(),
            agent_id: "agent_1".to_string(),
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1".to_string(),
            source_ref: None,
            timestamp_ms: 1_700_000_000_000,
            time_range_start_ms: 1_700_000_000_000,
            time_range_end_ms: 1_700_000_060_000,
            tags_json: r#"["person:Alice"]"#.to_string(),
            content: "Alice: Hello world".to_string(),
            token_count: 5,
            seq_in_source: 0,
            partial_message: false,
            lifecycle_status: "admitted".to_string(),
            created_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn test_write_and_read_chunk() {
        let (store, _root) = setup();
        let chunk = make_chunk();

        let written = store.write_chunk("owner_1", &chunk).unwrap();
        assert!(written);

        let body = store
            .read_chunk_body("owner_1", "chat", &chunk.source_id, &chunk.id)
            .unwrap();
        assert_eq!(body, "Alice: Hello world");
    }

    #[test]
    fn test_write_chunk_idempotent() {
        let (store, _root) = setup();
        let chunk = make_chunk();

        store.write_chunk("owner_1", &chunk).unwrap();
        let written = store.write_chunk("owner_1", &chunk).unwrap();
        assert!(!written); // Already exists
    }

    #[test]
    fn test_ensure_dirs() {
        let (store, root) = setup();
        store.ensure_dirs("owner_1").unwrap();

        assert!(root.join("owner_1").exists());
        assert!(root.join("owner_1/.obsidian").exists());
    }

    #[test]
    fn test_write_and_read_summary() {
        let (store, _root) = setup();
        let summary = SummaryNode {
            id: "sum_001".to_string(),
            tree_id: "tree_abc".to_string(),
            user_id: String::new(),
            tree_kind: TreeKind::Source,
            level: 1,
            parent_id: None,
            child_ids: vec!["chunk_001".to_string()],
            content: "Summary content".to_string(),
            token_count: 10,
            entities: vec!["person:Alice".to_string()],
            topics: vec!["project-phoenix".to_string()],
            time_range_start_ms: 1_700_000_000_000,
            time_range_end_ms: 1_700_000_060_000,
            score: 0.85,
            sealed_at_ms: 1_700_000_120_000,
            deleted: false,
            embedding: None,
        };

        let written = store.write_summary("owner_1", &summary).unwrap();
        assert!(written);
    }
}
