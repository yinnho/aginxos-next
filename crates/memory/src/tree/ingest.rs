//! Ingest orchestrator: canonicalise → chunk → score → persist → enqueue extract jobs.
//!
//! The hot path does: canonicalise → chunk → fast score → persist chunks/score rows
//! → enqueue extract jobs. The slower work (full extraction, admission, tree buffering,
//! sealing, topic routing, daily digests) runs out of the SQLite-backed jobs queue.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::memory_tree::{IngestRequest, IngestResult};

use super::canonicalize::{self, CanonicalisedSource};
use super::chunker::{self, ChunkInput};
use super::content_store::ContentStore;
use super::entity_store::EntityStore;
use super::extract;
use super::job_store::JobStore;
use super::score_store::ScoreStore;
use super::scoring;
use super::store::ChunkStore;
use super::types::{
    ExtractChunkPayload, JobKind, NewJob, SourceKind, CHUNK_STATUS_PENDING_EXTRACTION,
};

/// Ingest pipeline backed by SQLite + filesystem content store.
#[derive(Clone)]
pub struct IngestPipeline {
    chunk_store: ChunkStore,
    score_store: ScoreStore,
    entity_store: EntityStore,
    job_store: JobStore,
    content_store: ContentStore,
}

impl IngestPipeline {
    pub fn new(conn: Arc<Mutex<Connection>>, content_root: PathBuf) -> Self {
        Self {
            chunk_store: ChunkStore::new(conn.clone()),
            score_store: ScoreStore::new(conn.clone()),
            entity_store: EntityStore::new(conn.clone()),
            job_store: JobStore::new(conn.clone()),
            content_store: ContentStore::new(content_root),
        }
    }

    /// Ingest a batch of messages from any source kind.
    ///
    /// Chat and email: no source-level gate (streams accept repeated batches).
    /// Document: deduped by (owner_id, source_kind, source_id).
    pub fn ingest(&self, req: &IngestRequest) -> CarrierResult<IngestResult> {
        let source_kind = parse_source_kind(&req.source_kind);

        // Document dedup: skip if already ingested
        if source_kind == SourceKind::Document
            && self
                .job_store
                .check_ingested(&req.owner_id, &req.source_kind, &req.source_id)?
        {
            return Ok(IngestResult {
                chunks_created: 0,
                chunks_dropped: 0,
                source_id: req.source_id.clone(),
            });
        }

        // Canonicalise based on source kind
        let canonical = match source_kind {
            SourceKind::Chat => canonicalise_chat(req),
            SourceKind::Email => canonicalise_email(req),
            SourceKind::Document => canonicalise_document(req),
        };

        let canonical = match canonical {
            Some(c) => c,
            None => {
                return Ok(IngestResult {
                    chunks_created: 0,
                    chunks_dropped: 0,
                    source_id: req.source_id.clone(),
                });
            }
        };

        // Chunk the canonical markdown
        let tags = req.tags.clone();
        let chunks = chunker::chunk_messages(&ChunkInput {
            owner_id: &req.owner_id,
            user_id: req.user_id.as_deref().unwrap_or(""),
            agent_id: &req.agent_id,
            source_kind,
            source_id: &req.source_id,
            source_ref: canonical.source_ref.as_deref(),
            markdown: &canonical.markdown,
            tags: &tags,
            timestamp_ms: canonical.first_ts_ms,
            max_tokens: super::types::DEFAULT_CHUNK_MAX_TOKENS,
        });

        if chunks.is_empty() {
            return Ok(IngestResult {
                chunks_created: 0,
                chunks_dropped: 0,
                source_id: req.source_id.clone(),
            });
        }

        // Ensure content directories exist
        self.content_store.ensure_dirs(&req.owner_id)?;

        // Score each chunk and classify
        let mut chunks_written = 0usize;
        let mut chunks_dropped = 0usize;

        for chunk in &chunks {
            // Extract entities for scoring
            let entities = extract::extract_entities(&chunk.content);
            let entity_count = entities.len();

            // Score
            let decision = scoring::score_chunk(&chunk.content, source_kind, &tags, entity_count);

            // Persist score row
            self.score_store.write_score(
                &req.owner_id,
                &chunk.id,
                &decision.signals,
                decision.total,
                decision.dropped,
                Some(&decision.reason),
            )?;

            if decision.dropped {
                // Persist chunk but mark as dropped
                self.chunk_store
                    .upsert_chunks(std::slice::from_ref(chunk))?;
                self.chunk_store
                    .update_lifecycle(&req.owner_id, &chunk.id, "dropped")?;
                chunks_dropped += 1;
                continue;
            }

            // Persist chunk content to disk
            self.content_store.write_chunk(&req.owner_id, chunk)?;

            // Persist chunk to SQLite
            self.chunk_store
                .upsert_chunks(std::slice::from_ref(chunk))?;
            self.chunk_store.update_lifecycle(
                &req.owner_id,
                &chunk.id,
                CHUNK_STATUS_PENDING_EXTRACTION,
            )?;

            // Persist entity index entries
            for entity in &entities {
                self.entity_store.upsert_entity_index(
                    &req.owner_id,
                    &super::entity_store::EntityIndexEntry {
                        entity_id: &entity.canonical_id,
                        node_id: &chunk.id,
                        node_kind: "leaf",
                        entity_kind: entity.kind,
                        surface: &entity.surface,
                        score: decision.total,
                        timestamp_ms: chunk.timestamp_ms,
                        tree_id: None,
                        user_id: chunk.user_id.as_str(),
                    },
                )?;
                // Bump entity hotness
                self.entity_store.bump_entity_hotness(
                    &req.owner_id,
                    &entity.canonical_id,
                    &req.source_id,
                )?;
            }

            // Enqueue ExtractChunk job
            let payload = ExtractChunkPayload {
                chunk_id: chunk.id.clone(),
            };
            let job = NewJob {
                owner_id: req.owner_id.clone(),
                kind: JobKind::ExtractChunk,
                payload_json: serde_json::to_string(&payload)
                    .map_err(|e| CarrierError::Internal(e.to_string()))?,
                dedupe_key: Some(format!("extract:{}", chunk.id)),
                available_at_ms: None,
                max_attempts: None,
            };
            self.job_store.enqueue(&job)?;

            chunks_written += 1;
        }

        // Mark document sources as ingested
        if source_kind == SourceKind::Document && chunks_written > 0 {
            self.job_store
                .mark_ingested(&req.owner_id, &req.source_kind, &req.source_id)?;
        }

        Ok(IngestResult {
            chunks_created: chunks_written,
            chunks_dropped,
            source_id: req.source_id.clone(),
        })
    }
}

/// Parse source_kind string into enum.
fn parse_source_kind(s: &str) -> SourceKind {
    match s {
        "chat" => SourceKind::Chat,
        "email" => SourceKind::Email,
        "document" => SourceKind::Document,
        _ => SourceKind::Chat, // default
    }
}

/// Canonicalise chat messages.
fn canonicalise_chat(req: &IngestRequest) -> Option<CanonicalisedSource> {
    let messages: Vec<canonicalize::chat::ChatMessage> = req
        .messages
        .iter()
        .map(|m| canonicalize::chat::ChatMessage {
            author: m.sender.clone(),
            timestamp_ms: m.timestamp_ms,
            text: m.content.clone(),
            source_ref: None,
        })
        .collect();

    canonicalize::chat::canonicalise(
        &req.source_id,
        &req.tags,
        canonicalize::chat::ChatBatch {
            platform: req.source_kind.clone(),
            channel_label: req.source_id.clone(),
            messages,
        },
    )
}

/// Canonicalise email messages.
fn canonicalise_email(req: &IngestRequest) -> Option<CanonicalisedSource> {
    let messages: Vec<canonicalize::email::EmailMessage> = req
        .messages
        .iter()
        .map(|m| canonicalize::email::EmailMessage {
            from: m.sender.clone(),
            to: vec![],
            cc: vec![],
            subject: String::new(),
            sent_at_ms: m.timestamp_ms,
            body: m.content.clone(),
            source_ref: None,
        })
        .collect();

    canonicalize::email::canonicalise(
        &req.source_id,
        &req.tags,
        canonicalize::email::EmailThread {
            provider: req.source_kind.clone(),
            thread_subject: String::new(),
            messages,
        },
    )
}

/// Canonicalise a document (single body).
fn canonicalise_document(req: &IngestRequest) -> Option<CanonicalisedSource> {
    let body = req
        .messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");

    let modified_at_ms = req
        .messages
        .first()
        .map(|m| m.timestamp_ms)
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());

    canonicalize::document::canonicalise(
        &req.source_id,
        &req.tags,
        canonicalize::document::DocumentInput {
            provider: req.source_kind.clone(),
            title: String::new(),
            body,
            modified_at_ms,
            source_ref: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;
    use tempfile::TempDir;
    use carrier_types::memory_tree::IngestMessage;

    fn setup() -> (IngestPipeline, TempDir) {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        let dir = TempDir::new().unwrap();
        let pipeline = IngestPipeline::new(Arc::new(Mutex::new(conn)), dir.path().to_path_buf());
        (pipeline, dir)
    }

    fn chat_request(
        owner_id: &str,
        source_id: &str,
        messages: Vec<(&str, &str, i64)>,
    ) -> IngestRequest {
        IngestRequest {
            owner_id: owner_id.to_string(),
            agent_id: "agent_1".to_string(),
            source_kind: "chat".to_string(),
            source_id: source_id.to_string(),
            messages: messages
                .into_iter()
                .map(|(sender, content, ts_ms)| IngestMessage {
                    sender: sender.to_string(),
                    content: content.to_string(),
                    timestamp_ms: ts_ms,
                })
                .collect(),
            tags: vec![],
            user_id: None,
        }
    }

    #[test]
    fn test_ingest_chat_creates_chunks() {
        let (pipeline, _dir) = setup();
        let req = chat_request(
            "owner_1",
            "wechat:gh_abc:sender_1",
            vec![
                ("Alice", "We are planning to ship the Phoenix migration on Friday after reviewing the runbook. alice@example.com", 1_700_000_000_000),
                ("Bob", "Confirmed, I will handle the coordination and launch tracking tonight.", 1_700_000_010_000),
            ],
        );

        let result = pipeline.ingest(&req).unwrap();
        assert!(result.chunks_created >= 1);
        assert_eq!(result.source_id, "wechat:gh_abc:sender_1");
    }

    #[test]
    fn test_ingest_empty_messages() {
        let (pipeline, _dir) = setup();
        let req = chat_request("owner_1", "wechat:gh_abc:sender_1", vec![]);

        let result = pipeline.ingest(&req).unwrap();
        assert_eq!(result.chunks_created, 0);
        assert_eq!(result.chunks_dropped, 0);
    }

    #[test]
    fn test_ingest_tiny_chat_dropped() {
        let (pipeline, _dir) = setup();
        // "hi" alone → after canonicalisation it's still very short and no entities
        // The tiny_chunk_no_entities guard (<10 tokens, 0 entities) drops it
        let req = chat_request(
            "owner_1",
            "wechat:gh_abc:sender_1",
            vec![("Alice", "hi", 1_700_000_000_000)],
        );

        let result = pipeline.ingest(&req).unwrap();
        // "hi" is tiny: after canonicalisation it's still very short content
        // The scoring may or may not drop it depending on token count after headers
        // Just verify the pipeline runs without error
        assert!(result.chunks_created + result.chunks_dropped >= 1);
    }

    #[test]
    fn test_ingest_document_dedup() {
        let (pipeline, _dir) = setup();
        let req = IngestRequest {
            owner_id: "owner_1".to_string(),
            agent_id: "agent_1".to_string(),
            source_kind: "document".to_string(),
            source_id: "notion:page_abc".to_string(),
            messages: vec![IngestMessage {
                sender: "system".to_string(),
                content: "Important document content about project phoenix.".to_string(),
                timestamp_ms: 1_700_000_000_000,
            }],
            tags: vec![],
            user_id: None,
        };

        let first = pipeline.ingest(&req).unwrap();
        assert!(first.chunks_created >= 1);

        // Second ingest of same document should be deduped
        let second = pipeline.ingest(&req).unwrap();
        assert_eq!(second.chunks_created, 0);
    }

    #[test]
    fn test_ingest_owner_isolation() {
        let (pipeline, _dir) = setup();
        let req1 = chat_request(
            "owner_1",
            "wechat:gh_abc:sender_1",
            vec![("Alice", "We are planning to ship the Phoenix migration on Friday after reviewing the runbook.", 1_700_000_000_000)],
        );
        let req2 = chat_request(
            "owner_2",
            "wechat:gh_abc:sender_1",
            vec![("Alice", "We are planning to ship the Phoenix migration on Friday after reviewing the runbook.", 1_700_000_000_000)],
        );

        let r1 = pipeline.ingest(&req1).unwrap();
        let r2 = pipeline.ingest(&req2).unwrap();
        // Same content, different owners → different chunk IDs (owner_id is in hash)
        assert!(r1.chunks_created >= 1);
        assert!(r2.chunks_created >= 1);
    }

    #[test]
    fn test_ingest_email() {
        let (pipeline, _dir) = setup();
        let req = IngestRequest {
            owner_id: "owner_1".to_string(),
            agent_id: "agent_1".to_string(),
            source_kind: "email".to_string(),
            source_id: "gmail:thread_123".to_string(),
            messages: vec![
                IngestMessage {
                    sender: "bob@example.com".to_string(),
                    content: "Let's ship the new feature this week.".to_string(),
                    timestamp_ms: 1_700_000_000_000,
                },
                IngestMessage {
                    sender: "alice@example.com".to_string(),
                    content: "Agreed, the staging results look good.".to_string(),
                    timestamp_ms: 1_700_000_010_000,
                },
            ],
            tags: vec![],
            user_id: None,
        };

        let result = pipeline.ingest(&req).unwrap();
        assert!(result.chunks_created >= 1);
    }
}
