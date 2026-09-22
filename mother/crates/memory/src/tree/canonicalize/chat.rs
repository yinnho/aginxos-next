//! Chat transcripts → canonical Markdown.
//!
//! Chat sources are scoped by channel/group. A batch of chat messages
//! from the same channel becomes one [`CanonicalisedSource`]; the chunker
//! slices it by token budget downstream.
//!
//! Output format (chunker splits at `## ` boundaries):
//! ```md
//! ## 2026-04-21T10:12:00Z — Alice
//! Message body here.
//!
//! ## 2026-04-21T10:12:40Z — Bob
//! Reply body here.
//! ```

use super::{normalize_source_ref, CanonicalisedSource};
use crate::tree::types::SourceKind;

/// One chat message in a channel/group.
#[derive(Clone, Debug)]
pub struct ChatMessage {
    /// Author display name or id.
    pub author: String,
    /// When the message was sent (epoch milliseconds).
    pub timestamp_ms: i64,
    /// Plain text / markdown body.
    pub text: String,
    /// Optional per-message provenance pointer.
    pub source_ref: Option<String>,
}

/// A batch of messages from one logical channel.
#[derive(Clone, Debug)]
pub struct ChatBatch {
    /// Platform name (e.g. `slack`, `wechat`, `telegram`).
    pub platform: String,
    /// Human-readable channel/group name.
    pub channel_label: String,
    /// Ordered messages (chronological; adapter sorts defensively).
    pub messages: Vec<ChatMessage>,
}

/// Canonicalise a chat batch. Returns `None` if the batch is empty.
pub fn canonicalise(
    _source_id: &str,
    _tags: &[String],
    batch: ChatBatch,
) -> Option<CanonicalisedSource> {
    if batch.messages.is_empty() {
        return None;
    }

    let mut messages = batch.messages;
    messages.sort_by_key(|m| m.timestamp_ms);

    let first_ts_ms = messages.first().map(|m| m.timestamp_ms).unwrap();
    let last_ts_ms = messages.last().map(|m| m.timestamp_ms).unwrap();

    let mut md = String::new();
    for msg in &messages {
        let ts_iso = ms_to_iso(msg.timestamp_ms);
        md.push_str(&format!(
            "## {} — {}\n{}\n\n",
            ts_iso,
            msg.author,
            msg.text.trim()
        ));
    }

    let source_ref = normalize_source_ref(messages.first().and_then(|m| m.source_ref.clone()));

    Some(CanonicalisedSource {
        markdown: md,
        source_kind: SourceKind::Chat,
        first_ts_ms,
        last_ts_ms,
        source_ref,
    })
}

fn ms_to_iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| format!("{ms}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(ts_ms: i64, author: &str, text: &str) -> ChatMessage {
        ChatMessage {
            author: author.to_string(),
            timestamp_ms: ts_ms,
            text: text.to_string(),
            source_ref: Some(format!("slack://x/{ts_ms}")),
        }
    }

    #[test]
    fn empty_batch_returns_none() {
        let b = ChatBatch {
            platform: "slack".into(),
            channel_label: "#eng".into(),
            messages: vec![],
        };
        assert!(canonicalise("slack:#eng", &[], b).is_none());
    }

    #[test]
    fn messages_are_sorted_and_range_captured() {
        let b = ChatBatch {
            platform: "slack".into(),
            channel_label: "#eng".into(),
            messages: vec![
                msg(2000, "bob", "second"),
                msg(1000, "alice", "first"),
                msg(3000, "carol", "third"),
            ],
        };
        let out = canonicalise("slack:#eng", &["eng".into()], b).unwrap();
        assert_eq!(out.first_ts_ms, 1000);
        assert_eq!(out.last_ts_ms, 3000);
        let pos_first = out.markdown.find("first").unwrap();
        let pos_second = out.markdown.find("second").unwrap();
        let pos_third = out.markdown.find("third").unwrap();
        assert!(pos_first < pos_second);
        assert!(pos_second < pos_third);
    }

    #[test]
    fn includes_per_message_sections_without_header() {
        let b = ChatBatch {
            platform: "slack".into(),
            channel_label: "#eng".into(),
            messages: vec![msg(1000, "alice", "hello")],
        };
        let out = canonicalise("slack:#eng", &[], b).unwrap();
        assert!(!out.markdown.starts_with("# "));
        assert!(out.markdown.starts_with("## "));
        assert!(out.markdown.contains("— alice"));
        assert!(out.markdown.contains("hello"));
    }

    #[test]
    fn source_ref_taken_from_first_message() {
        let b = ChatBatch {
            platform: "slack".into(),
            channel_label: "#eng".into(),
            messages: vec![msg(1000, "alice", "hi"), msg(2000, "bob", "hey")],
        };
        let out = canonicalise("slack:#eng", &[], b).unwrap();
        assert_eq!(out.source_ref.as_deref(), Some("slack://x/1000"));
    }

    #[test]
    fn blank_source_ref_is_dropped() {
        let mut first = msg(1000, "alice", "hi");
        first.source_ref = Some("   ".into());
        let b = ChatBatch {
            platform: "slack".into(),
            channel_label: "#eng".into(),
            messages: vec![first],
        };
        let out = canonicalise("slack:#eng", &[], b).unwrap();
        assert!(out.source_ref.is_none());
    }
}
