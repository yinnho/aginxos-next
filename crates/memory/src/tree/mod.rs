//! Tree memory module — hierarchical memory with Obsidian-compatible content storage.
//!
//! Three-tier tree structure:
//! - tree_source: per (owner_id, agent_id, source_id) L0→L1→L2+ cascade
//! - tree_topic: per (owner_id, entity_id) lazy materialization
//! - tree_global: per owner_id daily→weekly→monthly→yearly cascade

pub mod bucket_seal;
pub mod canonicalize;
pub mod chunker;
pub mod content_store;
pub mod entity_store;
pub mod extract;
pub mod ingest;
pub mod job_store;
pub mod score_store;
pub mod scoring;
pub mod store;
pub mod summariser;
pub mod tree_global;
pub mod tree_store;
pub mod tree_topic;
pub mod types;

pub mod retrieval;
