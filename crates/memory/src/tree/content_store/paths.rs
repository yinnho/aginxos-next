//! Path generation for Obsidian-compatible .md content files.

use std::path::{Path, PathBuf};

/// Slugify a source_id for use as a directory name.
///
/// Rules: lowercase, replace non-`[a-z0-9_-]` with `-`, collapse consecutive
/// `-`, trim leading/trailing `-` and `_`, truncate to 120 chars.
pub fn slugify_source_id(source_id: &str) -> String {
    let lower = source_id.to_lowercase();
    let mut result = String::with_capacity(lower.len());
    let mut prev_dash = false;

    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            // Preserve interior underscores between alphanumeric chars
            if ch == '_' && result.is_empty() {
                prev_dash = false;
                continue;
            }
            result.push(ch);
            prev_dash = false;
        } else if !prev_dash && !result.is_empty() {
            result.push('-');
            prev_dash = true;
        }
    }

    // Trim trailing dashes and underscores
    let trimmed = result.trim_end_matches(['-', '_']);
    if trimmed.is_empty() {
        return "unknown".to_string();
    }

    // Truncate to 120 chars
    let mut end = trimmed.len().min(120);
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_string()
}

/// Sanitize a filename by replacing filesystem-illegal characters.
pub fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// Generate the relative path for a chunk .md file.
///
/// Format: `{source_kind}/{source_slug}/{chunk_id}.md`
pub fn chunk_rel_path(source_kind: &str, source_id: &str, chunk_id: &str) -> String {
    let source_slug = slugify_source_id(source_id);
    let safe_id = sanitize_filename(chunk_id);
    format!("{source_kind}/{source_slug}/{safe_id}.md")
}

/// Generate the absolute path for a chunk .md file.
pub fn chunk_abs_path(
    content_root: &Path,
    source_kind: &str,
    source_id: &str,
    chunk_id: &str,
) -> PathBuf {
    let rel = chunk_rel_path(source_kind, source_id, chunk_id);
    rel_to_abs(content_root, &rel)
}

/// Generate the relative path for a summary .md file.
///
/// Format: `summaries/{tree_kind}-{scope_slug}/L{level}/{summary_filename}.md`
pub fn summary_rel_path(tree_kind: &str, scope_slug: &str, level: u32, summary_id: &str) -> String {
    let safe_id = sanitize_filename(summary_id);
    format!("summaries/{tree_kind}-{scope_slug}/L{level}/{safe_id}.md")
}

/// Generate the absolute path for a summary .md file.
pub fn summary_abs_path(
    content_root: &Path,
    tree_kind: &str,
    scope_slug: &str,
    level: u32,
    summary_id: &str,
) -> PathBuf {
    let rel = summary_rel_path(tree_kind, scope_slug, level, summary_id);
    rel_to_abs(content_root, &rel)
}

/// Convert a forward-slash relative path to an OS-native absolute path.
fn rel_to_abs(content_root: &Path, rel: &str) -> PathBuf {
    let mut buf = content_root.to_path_buf();
    for component in rel.split('/') {
        buf.push(component);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify_wechat() {
        assert_eq!(
            slugify_source_id("wechat:gh_abc:openid_xyz"),
            "wechat-gh_abc-openid_xyz"
        );
    }

    #[test]
    fn test_slugify_feishu() {
        assert_eq!(
            slugify_source_id("feishu:ou_12345:sender_1"),
            "feishu-ou_12345-sender_1"
        );
    }

    #[test]
    fn test_slugify_preserves_underscores() {
        assert_eq!(slugify_source_id("foo_bar"), "foo_bar");
    }

    #[test]
    fn test_slugify_strips_leading_trailing() {
        assert_eq!(slugify_source_id("::hello::"), "hello");
    }

    #[test]
    fn test_slugify_empty() {
        assert_eq!(slugify_source_id(""), "unknown");
        assert_eq!(slugify_source_id(":::"), "unknown");
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("chunk:abc/def"), "chunk-abc-def");
    }

    #[test]
    fn test_chunk_rel_path() {
        let path = chunk_rel_path("chat", "wechat:gh_abc:sender_1", "chunk_001");
        assert_eq!(path, "chat/wechat-gh_abc-sender_1/chunk_001.md");
    }

    #[test]
    fn test_summary_rel_path() {
        let path = summary_rel_path("source", "wechat-gh-abc", 1, "sum_001");
        assert_eq!(path, "summaries/source-wechat-gh-abc/L1/sum_001.md");
    }

    #[test]
    fn test_abs_path() {
        let path = chunk_abs_path(
            Path::new("/data/memory_tree/content/owner_1"),
            "chat",
            "wechat:gh_abc:sender_1",
            "chunk_001",
        );
        assert_eq!(
            path,
            PathBuf::from(
                "/data/memory_tree/content/owner_1/chat/wechat-gh_abc-sender_1/chunk_001.md"
            )
        );
    }
}
