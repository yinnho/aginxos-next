//! Canonicalisers — normalise source-specific payloads into canonical Markdown.
//!
//! Each source kind has its own adapter returning the same shape:
//! a [`CanonicalisedSource`] containing the markdown blob plus provenance
//! metadata that the chunker will carry onto each produced chunk.
//!
//! Adapters do not interpret content semantically — they only normalise
//! shape and capture provenance. Scoring / entity extraction / summarisation
//! happen downstream.

pub mod chat;
pub mod document;
pub mod email;

use crate::tree::types::SourceKind;

/// Output of a canonicaliser — one per logical source record.
#[derive(Clone, Debug)]
pub struct CanonicalisedSource {
    /// Canonical Markdown blob produced by the adapter.
    pub markdown: String,
    /// Source kind (carried through to chunk metadata).
    pub source_kind: SourceKind,
    /// First timestamp in the source (epoch ms).
    pub first_ts_ms: i64,
    /// Last timestamp in the source (epoch ms).
    pub last_ts_ms: i64,
    /// Source reference (e.g. permalink), trimmed and non-empty.
    pub source_ref: Option<String>,
}

/// Trim provider-specific source references and drop blank pointers.
pub fn normalize_source_ref(source_ref: Option<String>) -> Option<String> {
    source_ref.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}
