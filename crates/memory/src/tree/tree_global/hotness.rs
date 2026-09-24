//! Entity hotness computation for topic tree spawning.
//!
//! Hotness formula (no LLM, no learned weights):
//! ```text
//! hotness = ln(mentions + 1) + 0.5 * distinct_sources
//!         + recency_decay(last_seen) + graph_centrality + 2.0 * query_hits
//! ```
//!
//! Recency decay is piecewise linear:
//! - age ≤ 1 day → 1.0
//! - age 1…7 days → 1.0 → 0.5
//! - age 7…30 days → 0.5 → 0.0
//! - age > 30 days → 0.0

/// Pure hotness function.
pub fn hotness(
    mention_count_30d: u32,
    distinct_sources: u32,
    last_seen_ms: Option<i64>,
    query_hits_30d: u32,
    graph_centrality: Option<f32>,
    now_ms: i64,
) -> f32 {
    let mention_weight = ((mention_count_30d as f32) + 1.0).ln();
    let source_weight = (distinct_sources as f32) * 0.5;
    let recency_weight = recency_decay(last_seen_ms, now_ms);
    let centrality = graph_centrality.unwrap_or(0.0);
    let query_weight = (query_hits_30d as f32) * 2.0;

    mention_weight + source_weight + recency_weight + centrality + query_weight
}

/// Recency decay helper. Returns 0.0 when `last_seen_ms` is `None`.
pub fn recency_decay(last_seen_ms: Option<i64>, now_ms: i64) -> f32 {
    let Some(last_seen) = last_seen_ms else {
        return 0.0;
    };
    let age_ms = (now_ms - last_seen).max(0);
    const DAY_MS: i64 = 24 * 60 * 60 * 1_000;
    let age_days = (age_ms as f32) / (DAY_MS as f32);

    if age_days <= 1.0 {
        1.0
    } else if age_days <= 7.0 {
        let frac = (age_days - 1.0) / 6.0;
        1.0 - 0.5 * frac
    } else if age_days <= 30.0 {
        let frac = (age_days - 7.0) / 23.0;
        0.5 - 0.5 * frac
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::types::TOPIC_CREATION_THRESHOLD;

    const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

    #[test]
    fn zero_signal_entity_is_zero() {
        let h = hotness(0, 0, None, 0, None, 1_700_000_000_000);
        assert!(h.abs() < 1e-6);
    }

    #[test]
    fn spike_of_mentions_pushes_over_creation_threshold() {
        let now_ms = 1_700_000_000_000;
        let h = hotness(100, 5, Some(now_ms - DAY_MS / 2), 3, None, now_ms);
        assert!(
            h > TOPIC_CREATION_THRESHOLD,
            "expected hot entity > {TOPIC_CREATION_THRESHOLD}, got {h}"
        );
    }

    #[test]
    fn recency_decay_today_is_one() {
        let now_ms = 1_700_000_000_000;
        let r = recency_decay(Some(now_ms), now_ms);
        assert!((r - 1.0).abs() < 1e-6);
    }

    #[test]
    fn recency_decay_week_old_is_half() {
        let now_ms = 1_700_000_000_000;
        let r = recency_decay(Some(now_ms - 7 * DAY_MS), now_ms);
        assert!((r - 0.5).abs() < 1e-3);
    }

    #[test]
    fn recency_decay_month_old_is_zero() {
        let now_ms = 1_700_000_000_000;
        let r = recency_decay(Some(now_ms - 30 * DAY_MS), now_ms);
        assert!(r.abs() < 1e-3);
    }

    #[test]
    fn recency_decay_none_is_zero() {
        assert_eq!(recency_decay(None, 1_700_000_000_000), 0.0);
    }
}
