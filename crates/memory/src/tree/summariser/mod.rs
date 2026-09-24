//! Summariser trait and implementations.
//!
//! The summariser produces a summary from a set of inputs (chunks or lower-level
//! summaries). Two implementations:
//!
//! - **InertSummariser**: concatenates inputs with provenance prefixes, truncates
//!   to token budget. No LLM call, no entities/topics. Used when no model is
//!   configured or as a fallback.
//! - **LlmSummariser**: (future) calls an LLM to produce a real summary.

pub mod inert;

use crate::tree::types::{TreeKind, OUTPUT_TOKEN_BUDGET};

/// Input to the summariser — one leaf chunk or lower-level summary.
#[derive(Clone, Debug)]
pub struct SummaryInput {
    pub id: String,
    pub content: String,
    pub token_count: u32,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
    pub time_range_start_ms: i64,
    pub time_range_end_ms: i64,
    pub score: f32,
}

/// Context passed to the summariser.
#[derive(Clone, Debug)]
pub struct SummaryContext<'a> {
    pub tree_id: &'a str,
    pub tree_kind: TreeKind,
    pub target_level: u32,
    pub token_budget: u32,
}

impl Default for SummaryContext<'_> {
    fn default() -> Self {
        Self {
            tree_id: "",
            tree_kind: TreeKind::Source,
            target_level: 1,
            token_budget: OUTPUT_TOKEN_BUDGET,
        }
    }
}

/// Output of the summariser.
#[derive(Clone, Debug)]
pub struct SummaryOutput {
    pub content: String,
    pub token_count: u32,
    pub entities: Vec<String>,
    pub topics: Vec<String>,
}

/// Trait for summarising a set of inputs.
pub trait Summariser: Send + Sync {
    fn summarise(&self, inputs: &[SummaryInput], ctx: &SummaryContext) -> SummaryOutput;
}
