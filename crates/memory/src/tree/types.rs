//! Core data types for the tree memory system.
//!
//! Adapted from OpenHuman's memory tree with multi-tenancy (owner_id) throughout.
//!
//! Shared types (NodeKind, TreeKind, EntityKind, RetrievalHit, QueryResponse,
//! EntityMatch) live in `carrier_types::memory_tree` and are re-exported here for
//! convenience within the memory crate.

use serde::{Deserialize, Serialize};

// Re-export shared types from the types crate (avoids circular dependency).
pub use carrier_types::memory_tree::{
    EntityKind, EntityMatch, NodeKind, QueryResponse, RetrievalHit, TreeKind,
};

// ---------------------------------------------------------------------------
// Enums (memory-internal, not shared)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Chat,
    Email,
    Document,
}

impl SourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Email => "email",
            Self::Document => "document",
        }
    }
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSource {
    WeChat,
    Feishu,
    WeCom,
    DingTalk,
    Api,
}

impl ChannelSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WeChat => "wechat",
            Self::Feishu => "feishu",
            Self::WeCom => "wecom",
            Self::DingTalk => "dingtalk",
            Self::Api => "api",
        }
    }

    pub fn source_kind(&self) -> SourceKind {
        SourceKind::Chat
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeStatus {
    Active,
    Archived,
}

impl TreeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    ExtractChunk,
    AppendBuffer,
    Seal,
    TopicRoute,
    DigestDaily,
    FlushStale,
}

impl JobKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExtractChunk => "extract_chunk",
            Self::AppendBuffer => "append_buffer",
            Self::Seal => "seal",
            Self::TopicRoute => "topic_route",
            Self::DigestDaily => "digest_daily",
            Self::FlushStale => "flush_stale",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Ready,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

// ---------------------------------------------------------------------------
// Lifecycle status constants
// ---------------------------------------------------------------------------

pub const CHUNK_STATUS_PENDING_EXTRACTION: &str = "pending_extraction";
pub const CHUNK_STATUS_ADMITTED: &str = "admitted";
// ---------------------------------------------------------------------------
// Core structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub owner_id: String,
    /// Per-user isolation within an owner. `""` = owner-shared (legacy /
    /// non-chat sources). See migration v27.
    #[serde(default)]
    pub user_id: String,
    pub agent_id: String,
    pub source_kind: SourceKind,
    pub source_id: String,
    pub source_ref: Option<String>,
    pub timestamp_ms: i64,
    pub time_range_start_ms: i64,
    pub time_range_end_ms: i64,
    pub tags_json: String,
    pub content: String,
    pub token_count: u32,
    pub seq_in_source: u32,
    #[serde(default)]
    pub partial_message: bool,
    pub lifecycle_status: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tree {
    pub id: String,
    pub owner_id: String,
    /// Per-user isolation within an owner. `""` = owner-shared (topic/global
    /// trees, legacy source trees). See migration v27.
    #[serde(default)]
    pub user_id: String,
    pub kind: TreeKind,
    pub scope: String,
    pub root_id: Option<String>,
    pub max_level: u32,
    pub status: TreeStatus,
    pub created_at_ms: i64,
    pub last_sealed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryNode {
    pub id: String,
    pub tree_id: String,
    /// Per-user isolation within an owner. `""` = owner-shared (topic/global
    /// summaries, legacy). See migration v27.
    #[serde(default)]
    pub user_id: String,
    pub tree_kind: TreeKind,
    pub level: u32,
    pub parent_id: Option<String>,
    pub child_ids: Vec<String>,
    pub content: String,
    pub token_count: u32,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
    pub time_range_start_ms: i64,
    pub time_range_end_ms: i64,
    pub score: f32,
    pub sealed_at_ms: i64,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Buffer {
    pub tree_id: String,
    pub level: u32,
    pub item_ids: Vec<String>,
    pub token_sum: i64,
    pub oldest_at_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// Job types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum NodeRef {
    Leaf { chunk_id: String },
    Summary { summary_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum AppendTarget {
    Source { source_id: String },
    Topic { tree_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractChunkPayload {
    pub chunk_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendBufferPayload {
    pub node: NodeRef,
    pub target: AppendTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealPayload {
    pub tree_id: String,
    pub level: u32,
    pub force_now_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopicRoutePayload {
    pub node: NodeRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestDailyPayload {
    pub date_iso: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlushStalePayload {
    pub max_age_secs: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub owner_id: String,
    pub kind: JobKind,
    pub payload_json: String,
    pub dedupe_key: Option<String>,
    pub status: JobStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub available_at_ms: i64,
    pub locked_until_ms: Option<i64>,
    pub last_error: Option<String>,
    pub created_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub owner_id: String,
    pub kind: JobKind,
    pub payload_json: String,
    pub dedupe_key: Option<String>,
    pub available_at_ms: Option<i64>,
    pub max_attempts: Option<u32>,
}

// ---------------------------------------------------------------------------
// Score types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreSignals {
    pub token_count: f32,
    pub unique_words: f32,
    pub metadata_weight: f32,
    pub source_weight: f32,
    pub interaction: f32,
    pub entity_density: f32,
    #[serde(default)]
    pub llm_importance: f32,
}

#[derive(Debug, Clone)]
pub struct SignalWeights {
    pub token_count: f32,
    pub unique_words: f32,
    pub metadata_weight: f32,
    pub source_weight: f32,
    pub interaction: f32,
    pub entity_density: f32,
    pub llm_importance: f32,
}

impl Default for SignalWeights {
    fn default() -> Self {
        Self {
            token_count: 1.0,
            unique_words: 1.0,
            metadata_weight: 1.5,
            source_weight: 1.5,
            interaction: 3.0,
            entity_density: 1.0,
            llm_importance: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Entity hotness
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HotnessCounters {
    pub entity_id: String,
    pub mention_count_30d: u32,
    pub distinct_sources: u32,
    pub last_seen_ms: Option<i64>,
    pub query_hits_30d: u32,
    pub graph_centrality: Option<f32>,
    pub ingests_since_check: u32,
    pub last_hotness: Option<f32>,
    pub last_updated_ms: i64,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// L0 buffer token sum that triggers an L0→L1 seal.
pub const INPUT_TOKEN_BUDGET: u32 = 50_000;

/// Max tokens for summariser output per seal.
pub const OUTPUT_TOKEN_BUDGET: u32 = 5_000;

/// Sibling count that triggers seal at level >= 1.
pub const SUMMARY_FANOUT: u32 = 10;

/// Max age before force-sealing a non-empty buffer (7 days).
pub const DEFAULT_FLUSH_AGE_SECS: i64 = 604_800;

/// Upper bound on per-chunk tokens.
pub const DEFAULT_CHUNK_MAX_TOKENS: u32 = 3_000;

/// Score threshold for definite admission (no LLM needed).
pub const DEFAULT_DEFINITE_KEEP: f32 = 0.85;

/// Score threshold for definite drop (no LLM needed).
pub const DEFAULT_DEFINITE_DROP: f32 = 0.15;

/// Score below which chunks are pruned.
pub const DEFAULT_DROP_THRESHOLD: f32 = 0.3;

/// Hotness above which a topic tree is materialised.
pub const TOPIC_CREATION_THRESHOLD: f32 = 10.0;

/// Hotness below which a topic tree is archived.
pub const TOPIC_ARCHIVE_THRESHOLD: f32 = 2.0;

/// Ingests between full hotness recomputes.
pub const TOPIC_RECHECK_EVERY: u32 = 100;

/// Maximum cascade depth for seal operations.
pub const MAX_CASCADE_DEPTH: u32 = 32;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_kind_roundtrip() {
        let kind = SourceKind::Chat;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"chat\"");
        let parsed: SourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn test_tree_kind_display() {
        assert_eq!(TreeKind::Source.to_string(), "source");
        assert_eq!(TreeKind::Topic.to_string(), "topic");
        assert_eq!(TreeKind::Global.to_string(), "global");
    }

    #[test]
    fn test_job_kind_roundtrip() {
        let kind = JobKind::Seal;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"seal\"");
        let parsed: JobKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn test_chunk_deterministic_id() {
        use sha2::{Digest, Sha256};
        let owner_id = "tenant_001";
        let source_kind = SourceKind::Chat;
        let source_id = "wechat:gh_abc:openid_xyz";
        let seq: u32 = 0;
        let content = "Hello world";

        let mut hasher = Sha256::new();
        hasher.update(owner_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(source_kind.as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(source_id.as_bytes());
        hasher.update(b"\0");
        hasher.update(seq.to_le_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_bytes());
        let hash = hasher.finalize();
        let id = format!("{:x}", hash);

        let mut hasher2 = Sha256::new();
        hasher2.update(owner_id.as_bytes());
        hasher2.update(b"\0");
        hasher2.update(source_kind.as_str().as_bytes());
        hasher2.update(b"\0");
        hasher2.update(source_id.as_bytes());
        hasher2.update(b"\0");
        hasher2.update(seq.to_le_bytes());
        hasher2.update(b"\0");
        hasher2.update(content.as_bytes());
        let hash2 = hasher2.finalize();
        let id2 = format!("{:x}", hash2);
        assert_eq!(id, id2);

        let mut hasher3 = Sha256::new();
        hasher3.update(b"tenant_002");
        hasher3.update(b"\0");
        hasher3.update(source_kind.as_str().as_bytes());
        hasher3.update(b"\0");
        hasher3.update(source_id.as_bytes());
        hasher3.update(b"\0");
        hasher3.update(seq.to_le_bytes());
        hasher3.update(b"\0");
        hasher3.update(content.as_bytes());
        let hash3 = hasher3.finalize();
        let id3 = format!("{:x}", hash3);
        assert_ne!(id, id3);
    }
}
