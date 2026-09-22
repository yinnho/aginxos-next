//! Knowledge extraction → kv drawer.
//!
//! Shared between the per-turn path (`end_turn`) and session compaction
//! (`compact_agent_session`). Both turn a bag of free-form "key facts" into
//! structured drawer entries (profile./preference./entity./fact./event.*),
//! merging them idempotently into whatever is already stored.
//!
//! Extracted from `end_turn.rs` so compaction can reuse the exact same merge
//! semantics without duplicating the classify/merge logic.

use std::sync::Arc;

use tracing::{debug, info};

use crate::memory_handle::MemoryHandle;

/// Merge a slice of free-form key facts into the kv drawer.
///
/// Each fact is classified into a drawer key (`profile.*` / `preference.*` /
/// `entity.*` / `fact.*` / `event.*`) and merged into the existing value.
/// Merging is **idempotent** (dedup by exact string) for both state and
/// timeline keys, so re-flushing the same fact — which happens when compaction
/// re-extracts a fact that per-turn extraction already wrote — never produces
/// duplicates.
pub fn merge_key_facts(
    memory_handle: &Arc<dyn MemoryHandle>,
    agent_name: &str,
    owner_id: &str,
    user_id: &str,
    facts: &[String],
) {
    for fact in facts {
        if let Some((key, new_values)) = classify_fact(fact) {
            merge_drawer_value(
                memory_handle,
                agent_name,
                owner_id,
                user_id,
                &key,
                new_values,
            );
        }
    }
}

/// Append `new_values` to `current`, skipping values already present.
///
/// Used for both state and timeline keys — two distinct real events always
/// differ in text, so exact-string dedup only ever suppresses a re-flush of
/// the same fact, never a genuinely new one.
fn extend_dedup(current: &mut Vec<String>, new_values: Vec<String>) {
    for v in new_values {
        if !current.contains(&v) {
            current.push(v);
        }
    }
}

/// Classify a key fact string into a drawer key + the values to store under it.
///
/// Rules:
/// - Phone/email → profile.*
/// - Preference (likes, wants) → preference.*
/// - Named entities (accounts, projects, orgs) → entity.*
/// - Facts (rules, constraints) → fact.*
/// - Decisions, events → event.YYYY-MM-DD.specific
fn classify_fact(fact: &str) -> Option<(String, Vec<String>)> {
    let lower = fact.to_lowercase();

    // Profile: personal identifiers
    if lower.contains("phone") || lower.contains("手机") {
        return Some(("profile.phone_numbers".to_string(), vec![fact.to_string()]));
    }
    if lower.contains("email") || lower.contains("邮箱") {
        return Some(("profile.email".to_string(), vec![fact.to_string()]));
    }

    // Preference
    if lower.contains("prefers")
        || lower.contains("likes")
        || lower.contains("wants")
        || lower.contains("偏好")
        || lower.contains("喜欢")
    {
        return Some(("preference.general".to_string(), vec![fact.to_string()]));
    }

    // Entity: accounts, projects, organizations
    if lower.contains("account")
        || lower.contains("公众号")
        || lower.contains("workspace")
        || lower.contains("项目")
    {
        return Some(("entity.accounts".to_string(), vec![fact.to_string()]));
    }

    // Event: decisions, scheduled items
    if lower.contains("decided")
        || lower.contains("决定")
        || lower.contains("scheduled")
        || lower.contains("计划")
    {
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let key = format!("event.{}.decision", date);
        return Some((key, vec![fact.to_string()]));
    }

    // Default: entity as catch-all for named items
    Some(("entity.misc".to_string(), vec![fact.to_string()]))
}

/// Merge new values into a drawer key.
///
/// Reads the existing value, dedup-merges (state or timeline — both idempotent
/// now), and writes the merged array back.
fn merge_drawer_value(
    memory_handle: &Arc<dyn MemoryHandle>,
    agent_name: &str,
    owner_id: &str,
    user_id: &str,
    key: &str,
    new_values: Vec<String>,
) {
    // Read existing value
    let existing = memory_handle
        .kv_get(agent_name, owner_id, user_id, key)
        .ok()
        .flatten();

    let mut merged = match existing {
        Some(serde_json::Value::Array(arr)) => {
            let mut current: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            extend_dedup(&mut current, new_values);
            current
        }
        Some(serde_json::Value::String(s)) => {
            let mut current = vec![s];
            extend_dedup(&mut current, new_values);
            current
        }
        Some(_other) => {
            // Non-array/string existing value — overwrite with new
            new_values
        }
        None => {
            // No existing value — just write new
            new_values
        }
    };

    let value = serde_json::Value::Array(merged.drain(..).map(serde_json::Value::String).collect());

    if let Err(e) = memory_handle.kv_set(agent_name, owner_id, user_id, key, value) {
        debug!("Failed to write drawer key '{}': {}", key, e);
    } else {
        info!(agent = agent_name, key = key, "Drawer entry updated");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use carrier_types::error::CarrierResult;
    use carrier_types::memory_tree::{
        DrillDownQuery, EntityMatch, EntitySearch, FetchLeavesQuery, GlobalQuery, IngestRequest,
        IngestResult, QueryResponse, SourceQuery, TopicQuery, TreeSummary,
    };

    /// In-memory MemoryHandle that records kv writes in a HashMap keyed by
    /// `(owner_id, user_id, key)`. Enough to exercise merge_key_facts; the
    /// tree/analytics methods are stubbed to empty.
    struct StubHandle {
        kv: Mutex<HashMap<(String, String, String), serde_json::Value>>,
    }

    impl StubHandle {
        fn new() -> Self {
            Self {
                kv: Mutex::new(HashMap::new()),
            }
        }
        fn snapshot(&self, owner: &str, user: &str, key: &str) -> Option<serde_json::Value> {
            self.kv
                .lock()
                .unwrap()
                .get(&(owner.to_string(), user.to_string(), key.to_string()))
                .cloned()
        }
    }

    #[async_trait]
    impl MemoryHandle for StubHandle {
        fn kv_set(
            &self,
            _agent: &str,
            owner: &str,
            user: &str,
            key: &str,
            value: serde_json::Value,
        ) -> CarrierResult<()> {
            self.kv.lock().unwrap().insert(
                (owner.to_string(), user.to_string(), key.to_string()),
                value,
            );
            Ok(())
        }
        fn kv_get(
            &self,
            _agent: &str,
            owner: &str,
            user: &str,
            key: &str,
        ) -> CarrierResult<Option<serde_json::Value>> {
            Ok(self
                .kv
                .lock()
                .unwrap()
                .get(&(owner.to_string(), user.to_string(), key.to_string()))
                .cloned())
        }
        fn kv_list(
            &self,
            _agent: &str,
            _owner: &str,
            _user: &str,
        ) -> CarrierResult<Vec<(String, serde_json::Value)>> {
            Ok(Vec::new())
        }
        fn kv_delete(
            &self,
            _agent: &str,
            _owner: &str,
            _user: &str,
            _key: &str,
        ) -> CarrierResult<()> {
            Ok(())
        }
        async fn tree_ingest(&self, _req: IngestRequest) -> CarrierResult<IngestResult> {
            Ok(IngestResult {
                chunks_created: 0,
                chunks_dropped: 0,
                source_id: String::new(),
            })
        }
        async fn tree_query_source(&self, _req: SourceQuery<'_>) -> CarrierResult<QueryResponse> {
            Ok(QueryResponse {
                hits: vec![],
                total: 0,
                truncated: false,
            })
        }
        async fn tree_query_global(&self, _req: GlobalQuery<'_>) -> CarrierResult<QueryResponse> {
            Ok(QueryResponse {
                hits: vec![],
                total: 0,
                truncated: false,
            })
        }
        async fn tree_query_topic(&self, _req: TopicQuery<'_>) -> CarrierResult<QueryResponse> {
            Ok(QueryResponse {
                hits: vec![],
                total: 0,
                truncated: false,
            })
        }
        async fn tree_search_entities(
            &self,
            _req: EntitySearch<'_>,
        ) -> CarrierResult<Vec<EntityMatch>> {
            Ok(Vec::new())
        }
        async fn tree_drill_down(&self, _req: DrillDownQuery<'_>) -> CarrierResult<QueryResponse> {
            Ok(QueryResponse {
                hits: vec![],
                total: 0,
                truncated: false,
            })
        }
        async fn tree_fetch_leaves(
            &self,
            _req: FetchLeavesQuery<'_>,
        ) -> CarrierResult<QueryResponse> {
            Ok(QueryResponse {
                hits: vec![],
                total: 0,
                truncated: false,
            })
        }
        async fn tree_list_sources(
            &self,
            _owner: &str,
            _kind: Option<&str>,
            _limit: usize,
        ) -> CarrierResult<Vec<TreeSummary>> {
            Ok(Vec::new())
        }
        fn analytics_user_stats(
            &self,
            _agent: &str,
            _days: u32,
        ) -> CarrierResult<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn analytics_user_lookup(
            &self,
            _agent: &str,
            _sender: &str,
        ) -> CarrierResult<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn analytics_usage(&self, _agent: &str, _days: u32) -> CarrierResult<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn analytics_recent_conversations(
            &self,
            _agent: &str,
            _limit: u32,
        ) -> CarrierResult<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
    }

    /// Re-flushing the same event fact twice must NOT append it twice — this is
    /// the regression for the timeline duplication bug that compaction's fact
    /// flush would otherwise re-introduce.
    #[test]
    fn test_merge_key_facts_event_dedup() {
        let stub = Arc::new(StubHandle::new());
        let handle: Arc<dyn MemoryHandle> = stub.clone();
        let fact = "decided to ship on Friday";
        let facts = vec![fact.to_string()];

        // Flush twice — simulates per-turn extraction + compaction re-extraction.
        merge_key_facts(&handle, "agent", "owner", "user", &facts);
        merge_key_facts(&handle, "agent", "owner", "user", &facts);

        // Find the event.* key (date is today, so match by prefix).
        let snap = stub.kv.lock().unwrap();
        let event_entry = snap.iter().find(|((_, _, k), _)| k.starts_with("event."));
        assert!(event_entry.is_some(), "event.* key should exist");
        if let Some((_, serde_json::Value::Array(arr))) = event_entry {
            assert_eq!(
                arr.len(),
                1,
                "event fact must not duplicate on re-flush: {:?}",
                arr
            );
        } else {
            panic!("event entry not an array");
        }
        drop(snap);

        // State key also dedups.
        let pref = "likes dark mode".to_string();
        merge_key_facts(
            &handle,
            "agent",
            "owner",
            "user",
            std::slice::from_ref(&pref),
        );
        merge_key_facts(
            &handle,
            "agent",
            "owner",
            "user",
            std::slice::from_ref(&pref),
        );
        let pref_snap = stub.snapshot("owner", "user", "preference.general");
        if let Some(serde_json::Value::Array(arr)) = pref_snap {
            assert_eq!(arr.len(), 1, "preference fact must not duplicate");
        } else {
            panic!("preference entry missing or wrong type");
        }
    }

    /// Distinct facts under the same key both survive.
    #[test]
    fn test_merge_key_facts_distinct_survive() {
        let stub = Arc::new(StubHandle::new());
        let handle: Arc<dyn MemoryHandle> = stub.clone();
        merge_key_facts(
            &handle,
            "agent",
            "owner",
            "user",
            &["likes dark mode".to_string(), "likes tea".to_string()],
        );
        let snap = stub.snapshot("owner", "user", "preference.general");
        if let Some(serde_json::Value::Array(arr)) = snap {
            assert_eq!(
                arr.len(),
                2,
                "two distinct preferences should both survive: {:?}",
                arr
            );
        } else {
            panic!("preference entry missing");
        }
    }
}
