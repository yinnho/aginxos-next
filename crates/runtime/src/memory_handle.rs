//! Memory handle trait — the carrier's independent memory service.
//!
//! Like Brain (LLM calls) and KernelHandle (inter-agent operations),
//! MemoryHandle is a top-level service handle injected into the agent
//! loop and ToolContext. It provides two capabilities:
//!
//! - **kv**: structured key-value storage (credentials, preferences, summaries)
//! - **tree**: hierarchical conversation history retrieval
//!
//! Both are scoped by (agent_id, owner_id, user_id) for multi-user isolation.

use async_trait::async_trait;
use carrier_types::error::CarrierResult;

/// Handle to memory operations, passed into the agent loop and tools.
///
/// Implemented by CarrierKernel by delegating to MemorySubstrate.
#[async_trait]
pub trait MemoryHandle: Send + Sync {
    // -----------------------------------------------------------------
    // KV operations — structured key-value storage
    // -----------------------------------------------------------------

    /// Store a key-value pair in the user's private memory.
    fn kv_set(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> CarrierResult<()>;

    /// Retrieve a value from the user's private memory by key.
    fn kv_get(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<Option<serde_json::Value>>;

    /// List all key-value pairs for a given agent + user.
    fn kv_list(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
    ) -> CarrierResult<Vec<(String, serde_json::Value)>>;

    /// Delete a key-value pair from the user's private memory.
    fn kv_delete(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<()>;

    // -----------------------------------------------------------------
    // Tree memory operations — conversation history retrieval
    // -----------------------------------------------------------------

    /// Ingest messages into the tree memory system.
    async fn tree_ingest(
        &self,
        req: carrier_types::memory_tree::IngestRequest,
    ) -> CarrierResult<carrier_types::memory_tree::IngestResult>;

    /// Query source-scoped tree summaries.
    async fn tree_query_source(
        &self,
        req: carrier_types::memory_tree::SourceQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse>;

    /// Query global tree summaries.
    async fn tree_query_global(
        &self,
        req: carrier_types::memory_tree::GlobalQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse>;

    /// Query topic-scoped tree by entity.
    async fn tree_query_topic(
        &self,
        req: carrier_types::memory_tree::TopicQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse>;

    /// Search entities by substring.
    async fn tree_search_entities(
        &self,
        req: carrier_types::memory_tree::EntitySearch<'_>,
    ) -> CarrierResult<Vec<carrier_types::memory_tree::EntityMatch>>;

    /// Drill down from a summary node to its children.
    async fn tree_drill_down(
        &self,
        req: carrier_types::memory_tree::DrillDownQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse>;

    /// Fetch all leaf chunks under a summary node.
    async fn tree_fetch_leaves(
        &self,
        req: carrier_types::memory_tree::FetchLeavesQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse>;

    /// List all source trees for an owner.
    async fn tree_list_sources(
        &self,
        owner_id: &str,
        source_kind: Option<&str>,
        limit: usize,
    ) -> CarrierResult<Vec<carrier_types::memory_tree::TreeSummary>>;

    // -----------------------------------------------------------------
    // Analytics operations (for data_analyze tool)
    // -----------------------------------------------------------------

    /// User statistics: total users, active users, new users.
    fn analytics_user_stats(
        &self,
        agent_id: &str,
        active_days: u32,
    ) -> CarrierResult<serde_json::Value>;

    /// Per-user lookup: session count, last active, recent conversation summary.
    fn analytics_user_lookup(
        &self,
        agent_id: &str,
        sender_id: &str,
    ) -> CarrierResult<serde_json::Value>;

    /// Usage analytics: token consumption, daily trends, per-model breakdown.
    fn analytics_usage(&self, agent_id: &str, days: u32) -> CarrierResult<serde_json::Value>;

    /// Recent conversations list (metadata only, no message content).
    fn analytics_recent_conversations(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> CarrierResult<serde_json::Value>;
}
