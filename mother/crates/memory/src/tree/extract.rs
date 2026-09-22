//! Regex-based entity extraction from chunk content.

use std::collections::HashSet;

use super::types::EntityKind;

/// An extracted entity from text.
#[derive(Debug, Clone)]
pub struct ExtractedEntity {
    pub canonical_id: String,
    pub kind: EntityKind,
    pub surface: String,
}

/// Extract entities from text using regex patterns.
pub fn extract_entities(text: &str) -> Vec<ExtractedEntity> {
    let mut entities = Vec::new();
    let mut seen = HashSet::new();

    // Email
    for mat in EMAIL_RE.find_iter(text) {
        let surface = mat.as_str().to_string();
        let canonical_id = format!("email:{}", surface.to_lowercase());
        if seen.insert(canonical_id.clone()) {
            entities.push(ExtractedEntity {
                canonical_id,
                kind: EntityKind::Email,
                surface,
            });
        }
    }

    // URL
    for mat in URL_RE.find_iter(text) {
        let surface = mat.as_str().to_string();
        let canonical_id = format!("url:{}", surface);
        if seen.insert(canonical_id.clone()) {
            entities.push(ExtractedEntity {
                canonical_id,
                kind: EntityKind::Url,
                surface,
            });
        }
    }

    // Hashtag (also generates a topic)
    for cap in HASHTAG_RE.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let tag = m.as_str().to_string();
            let canonical_id = format!("hashtag:{}", tag.to_lowercase());
            if seen.insert(canonical_id.clone()) {
                entities.push(ExtractedEntity {
                    canonical_id,
                    kind: EntityKind::Hashtag,
                    surface: format!("#{tag}"),
                });
            }
            // Also generate a topic entity
            let topic_id = format!("topic:{}", tag.to_lowercase());
            if seen.insert(topic_id.clone()) {
                entities.push(ExtractedEntity {
                    canonical_id: topic_id,
                    kind: EntityKind::Topic,
                    surface: tag.to_lowercase(),
                });
            }
        }
    }

    // Handle (@mention)
    for cap in HANDLE_RE.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let handle = m.as_str().to_string();
            let canonical_id = format!("handle:{}", handle.to_lowercase());
            if seen.insert(canonical_id.clone()) {
                entities.push(ExtractedEntity {
                    canonical_id,
                    kind: EntityKind::Handle,
                    surface: format!("@{handle}"),
                });
            }
        }
    }

    entities
}

lazy_static::lazy_static! {
    static ref EMAIL_RE: regex::Regex = regex::Regex::new(
        r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b"
    ).unwrap();

    static ref URL_RE: regex::Regex = regex::Regex::new(
        r"https?://[^\s<>\]\[()]+[^\s<>\]\[()\.\\\,;:\!\?]"
    ).unwrap();

    static ref HASHTAG_RE: regex::Regex = regex::Regex::new(
        r"(?:^|[\s(])#([A-Za-z][A-Za-z0-9_\-]{1,})"
    ).unwrap();

    static ref HANDLE_RE: regex::Regex = regex::Regex::new(
        r"(?:^|[\s(])@([A-Za-z0-9_][A-Za-z0-9_.\-]{1,})"
    ).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_email() {
        let entities = extract_entities("Contact alice@example.com for info");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].canonical_id, "email:alice@example.com");
        assert_eq!(entities[0].kind, EntityKind::Email);
    }

    #[test]
    fn test_extract_url() {
        let entities = extract_entities("Check https://example.com/page for details");
        assert!(entities.iter().any(|e| e.kind == EntityKind::Url));
        assert!(entities.iter().any(|e| e.canonical_id.starts_with("url:")));
    }

    #[test]
    fn test_extract_hashtag() {
        let entities = extract_entities("Working on #project-phoenix today");
        assert!(entities.iter().any(|e| e.kind == EntityKind::Hashtag));
        assert!(entities.iter().any(|e| e.kind == EntityKind::Topic));
        assert!(entities
            .iter()
            .any(|e| e.canonical_id == "hashtag:project-phoenix"));
        assert!(entities
            .iter()
            .any(|e| e.canonical_id == "topic:project-phoenix"));
    }

    #[test]
    fn test_extract_handle() {
        let entities = extract_entities("Hey @alice_smith check this out");
        assert!(entities.iter().any(|e| e.kind == EntityKind::Handle));
        assert!(entities
            .iter()
            .any(|e| e.canonical_id == "handle:alice_smith"));
    }

    #[test]
    fn test_dedup_entities() {
        let entities = extract_entities("Email alice@x.com and also alice@x.com again");
        let email_count = entities
            .iter()
            .filter(|e| e.kind == EntityKind::Email)
            .count();
        assert_eq!(email_count, 1); // Deduplicated
    }

    #[test]
    fn test_no_entities() {
        let entities = extract_entities("Just a plain text with no entities");
        assert!(entities.is_empty());
    }

    #[test]
    fn test_hashtag_rejects_numbers() {
        let entities = extract_entities("Issue #123 is fixed");
        assert!(!entities.iter().any(|e| e.kind == EntityKind::Hashtag));
    }
}
