//! Inert summariser — concatenates inputs with provenance prefixes, truncates to budget.
//!
//! No LLM call, no entity extraction. Used when no model is configured or as a
//! fallback when the LLM summariser fails.
//!
//! Entities and topics are intentionally empty — per design, summary-level
//! metadata should be LLM-derived, not mechanically unioned. The inert
//! summariser is a placeholder that preserves content without interpretation.

use super::{Summariser, SummaryContext, SummaryInput, SummaryOutput};
use crate::tree::chunker::approx_token_count;

/// Provenance prefix for each input in the concatenated output.
const PROVENANCE_PREFIX: &str = "-- ";

/// Inert summariser: concatenates and truncates.
pub struct InertSummariser;

impl Summariser for InertSummariser {
    fn summarise(&self, inputs: &[SummaryInput], ctx: &SummaryContext) -> SummaryOutput {
        let mut parts: Vec<String> = Vec::new();

        for input in inputs {
            if input.content.trim().is_empty() {
                continue;
            }
            parts.push(format!("{}{}\n", PROVENANCE_PREFIX, input.id));
            parts.push(format!("{}\n", input.content.trim()));
            parts.push(String::new()); // blank line separator
        }

        let full = parts.join("\n");
        let (content, token_count) = truncate_to_budget(&full, ctx.token_budget);

        SummaryOutput {
            content,
            token_count,
            entities: Vec::new(),
            topics: Vec::new(),
        }
    }
}

/// Truncate text to fit within `token_budget` using the ~4 chars/token heuristic.
fn truncate_to_budget(text: &str, token_budget: u32) -> (String, u32) {
    let max_chars = token_budget as usize * 4;
    if text.chars().count() <= max_chars {
        let tc = approx_token_count(text);
        return (text.to_string(), tc);
    }

    let truncated: String = text.chars().take(max_chars).collect();
    let tc = approx_token_count(&truncated);
    (truncated, tc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_prefix_in_output() {
        let inputs = vec![SummaryInput {
            id: "chunk_001".to_string(),
            content: "Hello world".to_string(),
            token_count: 3,
            entities: vec![],
            topics: vec![],
            time_range_start_ms: 0,
            time_range_end_ms: 0,
            score: 0.5,
        }];
        let ctx = SummaryContext::default();
        let output = InertSummariser.summarise(&inputs, &ctx);
        assert!(output.content.contains("-- chunk_001"));
        assert!(output.content.contains("Hello world"));
    }

    #[test]
    fn no_entities_or_topics() {
        let inputs = vec![SummaryInput {
            id: "chunk_001".to_string(),
            content: "Content".to_string(),
            token_count: 1,
            entities: vec!["person:Alice".to_string()],
            topics: vec!["project".to_string()],
            time_range_start_ms: 0,
            time_range_end_ms: 0,
            score: 0.5,
        }];
        let ctx = SummaryContext::default();
        let output = InertSummariser.summarise(&inputs, &ctx);
        assert!(output.entities.is_empty());
        assert!(output.topics.is_empty());
    }

    #[test]
    fn truncation_to_budget() {
        let inputs = vec![SummaryInput {
            id: "chunk_001".to_string(),
            content: "x".repeat(10_000),
            token_count: 2500,
            entities: vec![],
            topics: vec![],
            time_range_start_ms: 0,
            time_range_end_ms: 0,
            score: 0.5,
        }];
        let ctx = SummaryContext {
            token_budget: 100,
            ..Default::default()
        };
        let output = InertSummariser.summarise(&inputs, &ctx);
        assert!(output.token_count <= 100 + 10); // some slack for provenance
    }

    #[test]
    fn empty_contributions_skipped() {
        let inputs = vec![
            SummaryInput {
                id: "chunk_001".to_string(),
                content: String::new(),
                token_count: 0,
                entities: vec![],
                topics: vec![],
                time_range_start_ms: 0,
                time_range_end_ms: 0,
                score: 0.0,
            },
            SummaryInput {
                id: "chunk_002".to_string(),
                content: "Real content".to_string(),
                token_count: 3,
                entities: vec![],
                topics: vec![],
                time_range_start_ms: 0,
                time_range_end_ms: 0,
                score: 0.5,
            },
        ];
        let ctx = SummaryContext::default();
        let output = InertSummariser.summarise(&inputs, &ctx);
        assert!(!output.content.contains("-- chunk_001"));
        assert!(output.content.contains("-- chunk_002"));
    }
}
