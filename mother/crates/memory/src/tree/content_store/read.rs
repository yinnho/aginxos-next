//! Read and parse Obsidian-compatible .md files.

use std::path::Path;

use carrier_types::error::{CarrierError, CarrierResult};

/// Parsed frontmatter + body from a .md file.
#[derive(Debug, Clone)]
pub struct ParsedMd {
    pub frontmatter: String,
    pub body: String,
}

/// Parse a .md file into frontmatter and body sections.
pub fn parse_md(content: &str) -> CarrierResult<ParsedMd> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Ok(ParsedMd {
            frontmatter: String::new(),
            body: content.to_string(),
        });
    }

    // Find the closing ---
    let after_first = &trimmed[3..];
    let rest = after_first.trim_start_matches(['\r', '\n']);

    if let Some(end_pos) = rest.find("\n---") {
        let frontmatter = rest[..end_pos].to_string();
        let after_closing = &rest[end_pos + 4..]; // skip \n---
        let body = after_closing.trim_start_matches(['\r', '\n']).to_string();
        Ok(ParsedMd { frontmatter, body })
    } else {
        Err(CarrierError::Internal(
            "Markdown file has opening --- but no closing ---".to_string(),
        ))
    }
}

/// Read and parse a .md file from disk.
pub fn read_and_parse(path: &Path) -> CarrierResult<ParsedMd> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| CarrierError::Internal(format!("read {}: {e}", path.display())))?;
    parse_md(&content)
}

/// Extract the body content from a .md file (everything after closing ---).
pub fn read_body(path: &Path) -> CarrierResult<String> {
    let parsed = read_and_parse(path)?;
    Ok(parsed.body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_md_with_frontmatter() {
        let content = "---\nkey: value\n---\n\nHello world";
        let parsed = parse_md(content).unwrap();
        assert_eq!(parsed.frontmatter, "key: value");
        assert_eq!(parsed.body, "Hello world");
    }

    #[test]
    fn test_parse_md_no_frontmatter() {
        let content = "Just plain text";
        let parsed = parse_md(content).unwrap();
        assert!(parsed.frontmatter.is_empty());
        assert_eq!(parsed.body, "Just plain text");
    }

    #[test]
    fn test_parse_md_multiline_frontmatter() {
        let content = "---\nkey1: val1\nkey2: val2\n---\n\nBody text";
        let parsed = parse_md(content).unwrap();
        assert!(parsed.frontmatter.contains("key1: val1"));
        assert!(parsed.frontmatter.contains("key2: val2"));
        assert_eq!(parsed.body, "Body text");
    }

    #[test]
    fn test_parse_md_unclosed_frontmatter() {
        let content = "---\nkey: value\nno closing";
        assert!(parse_md(content).is_err());
    }
}
