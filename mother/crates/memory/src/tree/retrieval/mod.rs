//! Retrieval primitives for the tree memory system.
//!
//! Six read-only primitives that expose the sealed tree hierarchy:
//! - `query_source` — per-source summary retrieval
//! - `query_global` — cross-source digest for a time window
//! - `query_topic` — entity-scoped retrieval
//! - `search_entities` — fuzzy canonical-id lookup
//! - `drill_down` — walk summary children (BFS)
//! - `fetch_leaves` — batch chunk hydration

pub mod drill_down;
pub mod fetch;
pub mod global;
pub mod search;
pub mod source;
pub mod topic;
