//! Standalone documents → canonical Markdown.
//!
//! Document sources are single-record: one Notion page, one Drive doc,
//! one meeting-note file. The canonicaliser passes through the body verbatim.

use super::{normalize_source_ref, CanonicalisedSource};
use crate::tree::types::SourceKind;

/// Adapter input for a single document.
#[derive(Clone, Debug)]
pub struct DocumentInput {
    /// Provider name (e.g. `notion`, `drive`, `meeting_notes`).
    pub provider: String,
    /// Document title.
    pub title: String,
    /// Document body (markdown preferred; plain text also accepted).
    pub body: String,
    /// When the document was last modified (epoch ms).
    pub modified_at_ms: i64,
    /// Optional pointer back to source (URL, file path, Notion page id).
    pub source_ref: Option<String>,
}

/// Canonicalise a single document. Returns `None` if both title and body are empty.
pub fn canonicalise(
    _source_id: &str,
    _tags: &[String],
    doc: DocumentInput,
) -> Option<CanonicalisedSource> {
    if doc.body.trim().is_empty() && doc.title.trim().is_empty() {
        return None;
    }

    let md = format!("{}\n", doc.body.trim());

    Some(CanonicalisedSource {
        markdown: md,
        source_kind: SourceKind::Document,
        first_ts_ms: doc.modified_at_ms,
        last_ts_ms: doc.modified_at_ms,
        source_ref: normalize_source_ref(doc.source_ref),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(title: &str, body: &str) -> DocumentInput {
        DocumentInput {
            provider: "notion".into(),
            title: title.into(),
            body: body.into(),
            modified_at_ms: 1_700_000_000_000,
            source_ref: Some("notion://page/abc".into()),
        }
    }

    #[test]
    fn empty_doc_returns_none() {
        let d = DocumentInput {
            provider: "notion".into(),
            title: "".into(),
            body: "   \n  ".into(),
            modified_at_ms: 0,
            source_ref: None,
        };
        assert!(canonicalise("d1", &[], d).is_none());
    }

    #[test]
    fn renders_body_without_header() {
        let out = canonicalise("d1", &[], doc("Launch plan", "step one\n\nstep two")).unwrap();
        assert!(!out.markdown.starts_with("# "));
        assert!(out.markdown.contains("step one"));
        assert!(out.markdown.contains("step two"));
    }

    #[test]
    fn metadata_single_point_time_range() {
        let out = canonicalise("d1", &[], doc("x", "y")).unwrap();
        assert_eq!(out.first_ts_ms, out.last_ts_ms);
        assert_eq!(out.source_kind, SourceKind::Document);
    }

    #[test]
    fn blank_source_ref_is_dropped() {
        let mut input = doc("x", "y");
        input.source_ref = Some(" \n ".into());
        let out = canonicalise("d1", &[], input).unwrap();
        assert!(out.source_ref.is_none());
    }
}
