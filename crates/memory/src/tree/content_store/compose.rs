//! YAML frontmatter + body composition for Obsidian-compatible .md files.

use super::paths::slugify_source_id;

/// Input for composing a chunk .md file.
pub struct ChunkMdInput<'a> {
    pub source_kind: &'a str,
    pub source_id: &'a str,
    pub owner_id: &'a str,
    pub seq: u32,
    pub timestamp_ms: i64,
    pub time_range_start_ms: i64,
    pub time_range_end_ms: i64,
    pub tags: &'a [String],
    pub body: &'a str,
}

/// Input for composing a summary .md file.
pub struct SummaryMdInput<'a> {
    pub summary_id: &'a str,
    pub tree_kind: &'a str,
    pub tree_id: &'a str,
    pub tree_scope: &'a str,
    pub level: u32,
    pub child_ids: &'a [String],
    pub entities: &'a [String],
    pub topics: &'a [String],
    pub time_range_start_ms: i64,
    pub time_range_end_ms: i64,
    pub sealed_at_ms: i64,
    pub body: &'a str,
}

/// Compose a chunk .md file with YAML frontmatter.
pub fn compose_chunk_md(input: &ChunkMdInput) -> String {
    let source_slug = slugify_source_id(input.source_id);
    let timestamp_iso = ms_to_iso(input.timestamp_ms);
    let time_start_iso = ms_to_iso(input.time_range_start_ms);
    let time_end_iso = ms_to_iso(input.time_range_end_ms);

    let mut all_tags = vec![format!("source/{source_slug}")];
    all_tags.extend(input.tags.iter().cloned());

    let tags_yaml = format_yaml_list(&all_tags);

    format!(
        "---\n\
         source_kind: {source_kind}\n\
         source_id: {quoted_source_id}\n\
         seq: {seq}\n\
         owner: {owner_id}\n\
         timestamp: {timestamp_iso}\n\
         time_range_start: {time_start_iso}\n\
         time_range_end: {time_end_iso}\n\
         tags:\n{tags_yaml}\n\
         ---\n\
         \n\
         {body}",
        source_kind = input.source_kind,
        seq = input.seq,
        owner_id = input.owner_id,
        body = input.body,
        quoted_source_id = yaml_scalar(input.source_id),
    )
}

/// Compose a summary .md file with YAML frontmatter.
pub fn compose_summary_md(input: &SummaryMdInput) -> String {
    let time_start_iso = ms_to_iso(input.time_range_start_ms);
    let time_end_iso = ms_to_iso(input.time_range_end_ms);
    let sealed_iso = ms_to_iso(input.sealed_at_ms);

    let children_yaml = format_wikilink_list(input.child_ids);
    let entities_yaml = format_yaml_list(input.entities);
    let child_count = input.child_ids.len();
    let scope_slug = slugify_source_id(input.tree_scope);

    let mut tags = vec![format!(
        "{tree_kind}/{scope_slug}",
        tree_kind = input.tree_kind
    )];
    tags.extend(input.topics.iter().map(|t| format!("topic/{t}")));
    let tags_yaml = format_yaml_list(&tags);

    let alias = format!(
        "L{level} . {scope_slug} . {child_count} children . {time_start_iso}",
        level = input.level
    );

    format!(
        "---\n\
         id: {quoted_id}\n\
         kind: summary\n\
         tree_kind: {tree_kind}\n\
         tree_id: {quoted_tree_id}\n\
         tree_scope: {quoted_tree_scope}\n\
         level: {level}\n\
         children:\n{children_yaml}\n\
         child_count: {child_count}\n\
         time_range_start: {time_start_iso}\n\
         time_range_end: {time_end_iso}\n\
         sealed_at: {sealed_iso}\n\
         aliases:\n  - {alias}\n\
         tags:\n{tags_yaml}\n\
         entities:\n{entities_yaml}\n\
         ---\n\
         \n\
         {body}",
        level = input.level,
        tree_kind = input.tree_kind,
        child_count = child_count,
        body = input.body,
        quoted_id = yaml_scalar(input.summary_id),
        quoted_tree_id = yaml_scalar(input.tree_id),
        quoted_tree_scope = yaml_scalar(input.tree_scope),
    )
}

/// Format a YAML scalar, auto-quoting when needed.
fn yaml_scalar(s: &str) -> String {
    if s.is_empty()
        || s.contains(':')
        || s.contains('#')
        || s.contains('[')
        || s.contains(']')
        || s.contains('{')
        || s.contains('}')
        || s.contains('"')
        || s.contains('\'')
        || s.contains('\\')
        || s.starts_with(' ')
        || s.ends_with(' ')
        || s.starts_with('-')
        || s.starts_with('?')
        || s.starts_with('&')
        || s.starts_with('*')
        || s.starts_with('!')
        || s.starts_with('|')
        || s.starts_with('>')
        || s.starts_with('%')
        || s.starts_with('@')
        || s.starts_with('`')
    {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

fn format_yaml_list(items: &[String]) -> String {
    if items.is_empty() {
        "  []\n".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("  - {}", yaml_scalar(item)))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}

fn format_wikilink_list(ids: &[String]) -> String {
    if ids.is_empty() {
        "  []\n".to_string()
    } else {
        ids.iter()
            .map(|id| format!("  - \"[[{}]]\"", sanitize_wikilink(id)))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}

fn sanitize_wikilink(id: &str) -> String {
    id.replace(['|', '#'], "-").replace(['[', ']'], "")
}

/// Convert epoch milliseconds to ISO 8601 string.
fn ms_to_iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| format!("{ms}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compose_chunk_md() {
        let md = compose_chunk_md(&ChunkMdInput {
            source_kind: "chat",
            source_id: "wechat:gh_abc:sender_1",
            owner_id: "owner_1",
            seq: 0,
            timestamp_ms: 1000000,
            time_range_start_ms: 1000000,
            time_range_end_ms: 2000000,
            tags: &["person:Alice".to_string()],
            body: "Alice: Hello!",
        });

        assert!(md.starts_with("---\n"));
        assert!(md.contains("source_kind: chat"));
        assert!(md.contains("source/wechat-gh_abc-sender_1"));
        assert!(md.contains("person:Alice"));
        assert!(md.contains("Alice: Hello!"));
        assert!(md.contains("---\n\n"));
    }

    #[test]
    fn test_compose_summary_md() {
        let md = compose_summary_md(&SummaryMdInput {
            summary_id: "sum_001",
            tree_kind: "source",
            tree_id: "tree_abc",
            tree_scope: "wechat:gh_abc:sender_1",
            level: 1,
            child_ids: &["chunk_001".to_string(), "chunk_002".to_string()],
            entities: &["person:Alice".to_string()],
            topics: &["project-phoenix".to_string()],
            time_range_start_ms: 1000000,
            time_range_end_ms: 5000000,
            sealed_at_ms: 6000000,
            body: "Summary of chat about project phoenix",
        });

        assert!(md.starts_with("---\n"));
        assert!(md.contains("kind: summary"));
        assert!(md.contains("level: 1"));
        assert!(md.contains("[[chunk_001]]"));
        assert!(md.contains("[[chunk_002]]"));
        assert!(md.contains("child_count: 2"));
        assert!(md.contains("project-phoenix"));
    }

    #[test]
    fn test_yaml_scalar_plain() {
        assert_eq!(yaml_scalar("hello"), "hello");
    }

    #[test]
    fn test_yaml_scalar_quoted() {
        assert_eq!(yaml_scalar("wechat:abc"), "\"wechat:abc\"");
        assert_eq!(yaml_scalar("has #hash"), "\"has #hash\"");
        assert_eq!(yaml_scalar(""), "\"\"");
    }
}
