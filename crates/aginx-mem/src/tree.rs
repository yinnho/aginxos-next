//! memory_tree — 分层记忆树检索（M35 从 runtime tools/memory.rs 整体搬来）。
//!
//! 单工具多 mode（search_entities / query_topic / query_source /
//! query_global / drill_down / fetch_leaves），输出文案逐字保留——
//! runtime 桥 spawn 本 CLI 后金样本 flow 输出不变。owner 默认
//! "default" 对齐上游 `ctx.owner_id.unwrap_or("default")`。

use crate::{db_path_of, open_substrate, AgmemCtx};
use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;
use std::path::PathBuf;

pub fn memory_tree(input: &Value, ctx: &AgmemCtx, db_flag: Option<&PathBuf>) -> CarrierResult<String> {
    // owner 缺席回落 "default"、user 缺席 = 不按 user 过滤——与被搬的
    // runtime memory.rs `unwrap_or("default")` / `Option<&str>` 逐字一致
    // （kv 面的回落不同，见 kv.rs）。
    let owner_id = ctx.owner_id.as_deref().unwrap_or("default");
    let user_id = ctx.user_id.as_deref();

    let mode = match input.get("mode").and_then(|v| v.as_str()) {
        Some(m) => m,
        None => {
            return Err(CarrierError::InvalidInput(
                "memory_tree: 'mode' parameter is required. Valid modes: search_entities, query_topic, query_source, query_global, drill_down, fetch_leaves"
                    .to_string(),
            ))
        }
    };

    let sub = open_substrate(&db_path_of(ctx, db_flag))?;
    match mode {
        "search_entities" => search_entities(input, &sub, owner_id, user_id),
        "query_topic" => query_topic(input, &sub, owner_id, user_id),
        "query_source" => query_source(input, &sub, owner_id, user_id),
        "query_global" => query_global(input, &sub, owner_id, user_id),
        "drill_down" => drill_down(input, &sub, owner_id, user_id),
        "fetch_leaves" => fetch_leaves(input, &sub, owner_id, user_id),
        other => Err(CarrierError::InvalidInput(format!(
            "memory_tree: unknown mode `{other}`. Valid modes: search_entities, query_topic, query_source, query_global, drill_down, fetch_leaves"
        ))),
    }
}

use carrier_memory::MemorySubstrate;

fn search_entities(
    input: &Value,
    sub: &MemorySubstrate,
    owner_id: &str,
    user_id: Option<&str>,
) -> CarrierResult<String> {
    let query = input["query"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "query is required for search_entities".to_string(),
        ))?;
    let kind = input["kind"].as_str();
    let limit = input["limit"].as_u64().unwrap_or(5) as usize;

    let kinds: Option<&str> = input["kinds"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .or(kind);

    let req = carrier_types::memory_tree::EntitySearch {
        owner_id,
        query,
        kind: kinds,
        limit,
        user_id,
    };

    let matches = sub.tree_search_entities(&req)?;

    if matches.is_empty() {
        return Ok(format!("No entities matching '{}'.", query));
    }

    let mut lines = Vec::new();
    for m in &matches {
        lines.push(format!(
            "- {} (kind: {}, mentions: {}, last seen: {})",
            m.canonical_id,
            m.kind,
            m.mention_count,
            format_timestamp(m.last_seen_ms)
        ));
    }
    Ok(lines.join("\n"))
}

fn query_topic(
    input: &Value,
    sub: &MemorySubstrate,
    owner_id: &str,
    user_id: Option<&str>,
) -> CarrierResult<String> {
    let entity_id = input["entity_id"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "entity_id is required for query_topic".to_string(),
        ))?;
    let time_window_days = input["time_window_days"].as_u64().map(|d| d as u32);
    let query = input["query"].as_str();
    let limit = input["limit"].as_u64().unwrap_or(10) as usize;

    let req = carrier_types::memory_tree::TopicQuery {
        owner_id,
        entity_id,
        query,
        time_window_days,
        limit,
        user_id,
    };

    format_hit_response(sub.tree_query_topic(&req)?)
}

fn query_source(
    input: &Value,
    sub: &MemorySubstrate,
    owner_id: &str,
    user_id: Option<&str>,
) -> CarrierResult<String> {
    let source_id = input["source_id"].as_str();
    let source_kind = input["source_kind"].as_str();
    let time_window_days = input["time_window_days"].as_u64().map(|d| d as u32);
    let query = input["query"].as_str();
    let limit = input["limit"].as_u64().unwrap_or(10) as usize;

    let req = carrier_types::memory_tree::SourceQuery {
        owner_id,
        source_id,
        source_kind,
        time_window_days,
        query,
        limit,
        user_id,
    };

    format_hit_response(sub.tree_query_source(&req)?)
}

fn query_global(
    input: &Value,
    sub: &MemorySubstrate,
    owner_id: &str,
    user_id: Option<&str>,
) -> CarrierResult<String> {
    let time_window_days = input["time_window_days"].as_u64().map(|d| d as u32);
    let query = input["query"].as_str();
    let limit = input["limit"].as_u64().unwrap_or(10) as usize;

    let req = carrier_types::memory_tree::GlobalQuery {
        owner_id,
        time_window_days,
        query,
        limit,
        user_id,
    };

    format_hit_response(sub.tree_query_global(&req)?)
}

fn drill_down(
    input: &Value,
    sub: &MemorySubstrate,
    owner_id: &str,
    user_id: Option<&str>,
) -> CarrierResult<String> {
    let node_id = input["node_id"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "node_id is required for drill_down".to_string(),
        ))?;
    let max_depth = input["max_depth"].as_u64().unwrap_or(1) as u32;
    let limit = input["limit"].as_u64().unwrap_or(20) as usize;

    let req = carrier_types::memory_tree::DrillDownQuery {
        owner_id,
        node_id,
        max_depth,
        limit,
        user_id,
    };

    let resp = sub.tree_drill_down(&req)?;

    if resp.hits.is_empty() {
        return Ok(format!("No children found for node '{}'.", node_id));
    }

    let mut lines = Vec::new();
    for hit in &resp.hits {
        let kind = if hit.node_kind == carrier_types::memory_tree::NodeKind::Summary {
            "summary"
        } else {
            "chunk"
        };
        lines.push(format!(
            "[{}|L{}] {} (id: {}, children: [{}])",
            kind,
            hit.level,
            truncate_content(&hit.content, 200),
            hit.node_id,
            hit.child_ids.join(", ")
        ));
    }
    Ok(lines.join("\n"))
}

fn fetch_leaves(
    input: &Value,
    sub: &MemorySubstrate,
    owner_id: &str,
    user_id: Option<&str>,
) -> CarrierResult<String> {
    let chunk_ids: Vec<String> = input["chunk_ids"]
        .as_array()
        .ok_or(CarrierError::InvalidInput(
            "chunk_ids is required for fetch_leaves and must be an array".to_string(),
        ))?
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    if chunk_ids.is_empty() {
        return Err(CarrierError::InvalidInput(
            "chunk_ids must not be empty".to_string(),
        ));
    }

    let limit = input["limit"].as_u64().unwrap_or(20) as usize;

    let req = carrier_types::memory_tree::FetchLeavesQuery {
        owner_id,
        chunk_ids,
        limit,
        user_id,
    };

    let resp = sub.tree_fetch_leaves(&req)?;

    if resp.hits.is_empty() {
        return Ok("No leaf chunks found for the given IDs.".to_string());
    }

    let mut lines = Vec::new();
    for hit in &resp.hits {
        lines.push(format!(
            "[leaf|{}] (id: {})",
            truncate_content(&hit.content, 300),
            hit.node_id
        ));
    }
    Ok(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Shared formatting helpers（上游逐字保留）
// ---------------------------------------------------------------------------

fn format_hit_response(resp: carrier_types::memory_tree::QueryResponse) -> CarrierResult<String> {
    if resp.hits.is_empty() {
        return Ok("No memories found matching your query. This query has been checked thoroughly — do not retry with the same query. Try a different query or proceed without this information.".to_string());
    }

    let mut lines = Vec::new();
    for hit in &resp.hits {
        let kind = if hit.node_kind == carrier_types::memory_tree::NodeKind::Summary {
            "summary"
        } else {
            "chunk"
        };
        let time = format_time_range(hit.time_range_start_ms, hit.time_range_end_ms);
        let children = if hit.child_ids.is_empty() {
            String::new()
        } else {
            format!(" children:[{}]", hit.child_ids.join(","))
        };
        lines.push(format!(
            "[{}|{}|{}] {} (id: {}, score: {:.2}{})",
            kind,
            hit.tree_scope,
            time,
            truncate_content(&hit.content, 200),
            hit.node_id,
            hit.score,
            children
        ));
    }
    Ok(lines.join("\n"))
}

/// Byte-budgeted truncation that never splits a UTF-8 character: the cut
/// point snaps back to the nearest char boundary. A raw `&s[..max]` panics
/// when byte `max` lands mid-character — Chinese tree content (3 bytes/char)
/// hits it almost every time, which killed the first real `memory_tree` call
/// in production (2026-08-17 86bus: "end byte index 200 is not a char
/// boundary" panicked the whole agent turn). Ported verbatim from runtime
/// tools/memory.rs — the landmine moves with the code.
pub(crate) fn truncate_content(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max.min(s.len());
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

fn format_timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| ms.to_string())
}

fn format_time_range(start_ms: i64, end_ms: i64) -> String {
    format!(
        "{} — {}",
        format_timestamp(start_ms),
        format_timestamp(end_ms)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The production panic (2026-08-17): Chinese tree content, byte cut at
    /// 200 landed inside a 3-byte char and `&s[..200]` panicked the whole
    /// agent turn. The cut must snap back to a char boundary instead.
    #[test]
    fn truncate_content_snaps_back_on_multibyte_boundary() {
        let chinese = "心".repeat(100); // 300 bytes; byte 200 is mid-char (198..201)
        let t = truncate_content(&chinese, 200);
        let body = t.strip_suffix("...").expect("ellipsis appended");
        assert_eq!(body.chars().count(), 66, "cut snaps to byte 198 = 66 chars");
        assert!(!body.chars().any(|c| c == '\u{FFFD}'));

        // Short input passes through unchanged; ASCII cuts exactly at the cap.
        assert_eq!(truncate_content("abc", 5), "abc");
        assert_eq!(truncate_content("abcdefgh", 5), "abcde...");

        // Mixed content: cap lands mid-ASCII-char impossible, mid-CJK snaps.
        let mixed = format!("{}月卡咨询", "x".repeat(197));
        let t = truncate_content(&mixed, 200);
        assert!(t.ends_with("..."));
        assert!(t.is_char_boundary(t.len() - 3));
    }

    #[test]
    fn mode_gate_errors_match_upstream_copy() {
        let db = tempfile::tempdir().unwrap().into_path().join("gate.db");
        let ctx = crate::default_identity();
        let r = memory_tree(&serde_json::json!({}), &ctx, Some(&db));
        let msg = match r {
            Err(CarrierError::InvalidInput(m)) => m,
            other => panic!("expected InvalidInput, got {other:?}"),
        };
        assert!(msg.contains("'mode' parameter is required"));
        // 未知 mode 的报错列出全部合法值（LLM 自纠错的锚点）
        let r = memory_tree(&serde_json::json!({"mode": "nope"}), &ctx, Some(&db));
        let msg = match r {
            Err(CarrierError::InvalidInput(m)) => m,
            other => panic!("expected InvalidInput, got {other:?}"),
        };
        assert!(msg.contains("search_entities") && msg.contains("fetch_leaves"));
    }
}
