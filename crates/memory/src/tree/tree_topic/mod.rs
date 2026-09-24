//! Topic trees — lazy per-entity summary trees.
//!
//! A topic tree is a per-entity summary tree whose leaves are all chunks
//! mentioning that entity, regardless of source. Topic trees are spawned
//! lazily when an entity's hotness crosses a threshold.

pub use crate::tree::types::{
    TOPIC_ARCHIVE_THRESHOLD, TOPIC_CREATION_THRESHOLD, TOPIC_RECHECK_EVERY,
};
