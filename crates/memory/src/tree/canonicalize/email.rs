//! Email threads → canonical Markdown.
//!
//! Email sources are scoped by participant set. One participant bucket
//! becomes one [`CanonicalisedSource`]. Headers (From, To, Cc, Subject, Date)
//! surface in a frontmatter-style block per message; the body follows.
//!
//! The chunker splits at `---` separators so each message becomes one chunk.

use super::{normalize_source_ref, CanonicalisedSource};
use crate::tree::types::SourceKind;

/// One email in a thread.
#[derive(Clone, Debug)]
pub struct EmailMessage {
    pub from: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub subject: String,
    /// When the email was sent (epoch ms).
    pub sent_at_ms: i64,
    /// Plain-text or markdown body.
    pub body: String,
    /// Message-id header or provider URL.
    pub source_ref: Option<String>,
}

/// A whole email thread.
#[derive(Clone, Debug)]
pub struct EmailThread {
    /// Provider name (e.g. `gmail`, `outlook`).
    pub provider: String,
    /// Thread subject.
    pub thread_subject: String,
    /// Ordered messages (chronological; adapter sorts defensively).
    pub messages: Vec<EmailMessage>,
}

/// Canonicalise an email thread. Returns `None` when the thread has no messages.
pub fn canonicalise(
    _source_id: &str,
    _tags: &[String],
    thread: EmailThread,
) -> Option<CanonicalisedSource> {
    if thread.messages.is_empty() {
        return None;
    }

    let mut messages = thread.messages;
    messages.sort_by_key(|m| m.sent_at_ms);

    let first_ts_ms = messages.first().map(|m| m.sent_at_ms).unwrap();
    let last_ts_ms = messages.last().map(|m| m.sent_at_ms).unwrap();

    let mut md = String::new();
    for msg in &messages {
        md.push_str("---\n");
        md.push_str(&format!("From: {}\n", msg.from));
        if !msg.to.is_empty() {
            md.push_str(&format!("To: {}\n", msg.to.join(", ")));
        }
        if !msg.cc.is_empty() {
            md.push_str(&format!("Cc: {}\n", msg.cc.join(", ")));
        }
        md.push_str(&format!("Subject: {}\n", msg.subject));
        md.push_str(&format!("Date: {}\n", ms_to_iso(msg.sent_at_ms)));
        md.push('\n');
        let body = msg.body.trim();
        if body.is_empty() {
            md.push('\n');
        } else {
            md.push_str(body);
        }
        md.push_str("\n\n");
    }

    let source_ref = normalize_source_ref(messages.first().and_then(|m| m.source_ref.clone()));

    Some(CanonicalisedSource {
        markdown: md,
        source_kind: SourceKind::Email,
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

    fn email(ts_ms: i64, from: &str, subject: &str, body: &str) -> EmailMessage {
        EmailMessage {
            from: from.to_string(),
            to: vec!["alice@example.com".into()],
            cc: vec![],
            subject: subject.to_string(),
            sent_at_ms: ts_ms,
            body: body.to_string(),
            source_ref: Some(format!("<msg-{ts_ms}@example.com>")),
        }
    }

    #[test]
    fn empty_thread_returns_none() {
        let t = EmailThread {
            provider: "gmail".into(),
            thread_subject: "x".into(),
            messages: vec![],
        };
        assert!(canonicalise("gmail:t1", &[], t).is_none());
    }

    #[test]
    fn renders_headers_and_body_per_message() {
        let t = EmailThread {
            provider: "gmail".into(),
            thread_subject: "Launch".into(),
            messages: vec![
                email(1000, "bob@example.com", "Launch", "let's ship"),
                email(2000, "alice@example.com", "Re: Launch", "agreed"),
            ],
        };
        let out = canonicalise("gmail:t1", &[], t).unwrap();
        assert!(out.markdown.contains("From: bob@example.com"));
        assert!(out.markdown.contains("Subject: Launch"));
        assert!(out.markdown.contains("let's ship"));
        assert!(out.markdown.contains("Re: Launch"));
        assert!(out.markdown.contains("agreed"));
    }

    #[test]
    fn time_range_spans_thread() {
        let t = EmailThread {
            provider: "gmail".into(),
            thread_subject: "x".into(),
            messages: vec![
                email(3000, "c", "y", "third"),
                email(1000, "a", "y", "first"),
                email(2000, "b", "y", "second"),
            ],
        };
        let out = canonicalise("gmail:t1", &[], t).unwrap();
        assert_eq!(out.first_ts_ms, 1000);
        assert_eq!(out.last_ts_ms, 3000);
    }

    #[test]
    fn source_ref_from_first_message() {
        let t = EmailThread {
            provider: "gmail".into(),
            thread_subject: "x".into(),
            messages: vec![email(1000, "a", "y", "b"), email(2000, "b", "y", "c")],
        };
        let out = canonicalise("gmail:t1", &[], t).unwrap();
        assert_eq!(out.source_ref.as_deref(), Some("<msg-1000@example.com>"));
    }

    #[test]
    fn blank_source_ref_is_dropped() {
        let mut first = email(1000, "a", "y", "b");
        first.source_ref = Some("".into());
        let t = EmailThread {
            provider: "gmail".into(),
            thread_subject: "x".into(),
            messages: vec![first],
        };
        let out = canonicalise("gmail:t1", &[], t).unwrap();
        assert!(out.source_ref.is_none());
    }
}
