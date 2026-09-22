//! Tree memory trait and request/response types.
//!
//! The TreeMemory trait is the primary memory interface for opencarrier,
//! replacing the old flat KV/semantic/knowledge stores with a hierarchical
//! tree structure adapted from OpenHuman.

use crate::error::CarrierResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Retrieval types (shared between types and memory crates)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Leaf,
    Summary,
}

impl NodeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Leaf => "leaf",
            Self::Summary => "summary",
        }
    }
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeKind {
    Source,
    Topic,
    Global,
}

impl TreeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Topic => "topic",
            Self::Global => "global",
        }
    }
}

impl std::fmt::Display for TreeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalHit {
    pub node_id: String,
    pub node_kind: NodeKind,
    pub tree_id: String,
    pub tree_kind: TreeKind,
    pub tree_scope: String,
    pub level: u32,
    pub content: String,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
    pub time_range_start_ms: i64,
    pub time_range_end_ms: i64,
    pub score: f32,
    pub child_ids: Vec<String>,
    pub source_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    pub hits: Vec<RetrievalHit>,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Email,
    Url,
    Handle,
    Hashtag,
    Person,
    Organization,
    Location,
    Event,
    Product,
    Datetime,
    Technology,
    Artifact,
    Quantity,
    Misc,
    Topic,
}

impl EntityKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Url => "url",
            Self::Handle => "handle",
            Self::Hashtag => "hashtag",
            Self::Person => "person",
            Self::Organization => "organization",
            Self::Location => "location",
            Self::Event => "event",
            Self::Product => "product",
            Self::Datetime => "datetime",
            Self::Technology => "technology",
            Self::Artifact => "artifact",
            Self::Quantity => "quantity",
            Self::Misc => "misc",
            Self::Topic => "topic",
        }
    }
}

impl std::fmt::Display for EntityKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityMatch {
    pub canonical_id: String,
    pub kind: EntityKind,
    pub surface: String,
    pub mention_count: u64,
    pub last_seen_ms: i64,
}

// ---------------------------------------------------------------------------
// Ingest request types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestMessage {
    pub sender: String,
    pub content: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    pub owner_id: String,
    pub agent_id: String,
    pub source_kind: String,
    pub source_id: String,
    pub messages: Vec<IngestMessage>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// User ID for per-user isolation. None = shared/owner-level data.
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestResult {
    pub chunks_created: usize,
    pub chunks_dropped: usize,
    pub source_id: String,
}

// ---------------------------------------------------------------------------
// Query request types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SourceQuery<'a> {
    pub owner_id: &'a str,
    pub source_id: Option<&'a str>,
    pub source_kind: Option<&'a str>,
    pub time_window_days: Option<u32>,
    pub query: Option<&'a str>,
    pub limit: usize,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct GlobalQuery<'a> {
    pub owner_id: &'a str,
    pub time_window_days: Option<u32>,
    pub query: Option<&'a str>,
    pub limit: usize,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct TopicQuery<'a> {
    pub owner_id: &'a str,
    pub entity_id: &'a str,
    pub query: Option<&'a str>,
    pub time_window_days: Option<u32>,
    pub limit: usize,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct EntitySearch<'a> {
    pub owner_id: &'a str,
    pub query: &'a str,
    pub kind: Option<&'a str>,
    pub limit: usize,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct DrillDownQuery<'a> {
    pub owner_id: &'a str,
    pub node_id: &'a str,
    pub max_depth: u32,
    pub limit: usize,
    pub user_id: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct FetchLeavesQuery<'a> {
    pub owner_id: &'a str,
    pub chunk_ids: Vec<String>,
    pub limit: usize,
    pub user_id: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// Tree summary (for memory_list tool)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeSummary {
    pub tree_id: String,
    pub kind: String,
    pub scope: String,
    pub status: String,
    pub max_level: u32,
    pub chunk_count: usize,
    pub summary_count: usize,
    pub last_sealed_at_ms: Option<i64>,
}

// ---------------------------------------------------------------------------
// TreeMemory trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait TreeMemory: Send + Sync {
    /// Ingest messages into the tree memory system.
    async fn tree_ingest(&self, req: IngestRequest) -> CarrierResult<IngestResult>;

    /// Search within source trees.
    async fn tree_query_source(&self, req: SourceQuery<'_>) -> CarrierResult<QueryResponse>;

    /// Search across all sources via the global tree.
    async fn tree_query_global(&self, req: GlobalQuery<'_>) -> CarrierResult<QueryResponse>;

    /// Search an entity-centric topic tree.
    async fn tree_query_topic(&self, req: TopicQuery<'_>) -> CarrierResult<QueryResponse>;

    /// Fuzzy entity search.
    async fn tree_search_entities(&self, req: EntitySearch<'_>) -> CarrierResult<Vec<EntityMatch>>;

    /// Navigate from a summary node to its children.
    async fn tree_drill_down(&self, req: DrillDownQuery<'_>) -> CarrierResult<QueryResponse>;

    /// Fetch all leaf chunks under a summary node.
    async fn tree_fetch_leaves(&self, req: FetchLeavesQuery<'_>) -> CarrierResult<QueryResponse>;

    /// List all source trees for an owner.
    async fn tree_list_sources(
        &self,
        owner_id: &str,
        source_kind: Option<&str>,
        limit: usize,
    ) -> CarrierResult<Vec<TreeSummary>>;
}
