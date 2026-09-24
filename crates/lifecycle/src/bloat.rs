//! Knowledge bloat control — detect stale/expired/deletable files and manage capacity.
//!
//! Two-step expiry:
//! 1. Files older than `stale_days` (default 30) with no `status: expired` → mark expired
//! 2. Files already expired for `delete_days` (default 60) → delete
//!
//! Capacity pruning: if file count or total size exceeds limits, coldest files get pruned.
//!
//! LLM-dependent operations (merge, compress, generate metadata) are provided as
//! prompt builders + response parsers — the kernel calls LLM itself.

use anyhow::Result;
use std::fs;
use std::path::Path;
use tracing::info;

use crate::evolution_config::EvolutionConfig;

/// Scanned metadata for a single knowledge file.
#[derive(Debug, Clone)]
pub struct KnowledgeMeta {
    pub filename: String,
    /// Modification time as Unix timestamp (seconds).
    pub modified_at: i64,
    /// Tags from frontmatter.
    pub tags: Vec<String>,
    /// Status from frontmatter (e.g. "expired", "active").
    pub status: Option<String>,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// Result of bloat detection scan.
#[derive(Debug, Clone)]
pub struct BloatReport {
    /// Tag-overlapping pairs that should be merged. (file_a, file_b, jaccard).
    /// Only populated when `needs_pruning` is true and tags exist.
    pub should_merge_candidates: Vec<(String, String, f64)>,
    /// Files older than stale_days but not yet expired → mark expired.
    pub stale_files: Vec<String>,
    /// Files already expired for longer than delete_days → delete.
    pub deletable_files: Vec<String>,
    /// Total size of all knowledge files in bytes.
    pub total_size_bytes: u64,
    /// Total number of knowledge files.
    pub total_files: usize,
    /// Whether file count or total size exceeds configured limits.
    pub needs_pruning: bool,
}

// ---------------------------------------------------------------------------
// Pure file-system logic (no LLM)
// ---------------------------------------------------------------------------

/// Scan knowledge directory and produce a bloat report.
///
/// This is pure file-system logic — reads files, checks modification times,
/// computes tag overlap. The kernel decides what to do with the report.
pub fn check_bloat(workspace: &Path, config: &EvolutionConfig) -> Result<BloatReport> {
    let knowledge_dir = workspace.join("knowledge");
    if !knowledge_dir.exists() {
        return Ok(BloatReport {
            should_merge_candidates: vec![],
            stale_files: vec![],
            deletable_files: vec![],
            total_size_bytes: 0,
            total_files: 0,
            needs_pruning: false,
        });
    }

    let mut metas = Vec::new();
    let mut total_size_bytes: u64 = 0;

    for entry in fs::read_dir(&knowledge_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map(|e| e == "md").unwrap_or(false) {
            continue;
        }
        let filename = entry.file_name().to_string_lossy().to_string();
        let metadata = fs::metadata(&path)?;
        let size_bytes = metadata.len();
        total_size_bytes += size_bytes;

        let modified_at = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let content = fs::read_to_string(&path).unwrap_or_default();
        let (tags, status) = parse_frontmatter_tags_status(&content);

        metas.push(KnowledgeMeta {
            filename,
            modified_at,
            tags,
            status,
            size_bytes,
        });
    }

    let total_files = metas.len();
    let max_total_bytes = (config.knowledge_capacity_mb as u64) * 1024 * 1024;
    let needs_pruning =
        total_files > config.max_knowledge_files || total_size_bytes > max_total_bytes;

    // Tag overlap candidates (Jaccard ≥ 0.7)
    let mut should_merge_candidates = Vec::new();
    if needs_pruning {
        let merge_threshold = 0.7;
        let max_checks = 20;
        let mut checked = 0;
        for i in 0..metas.len() {
            if checked >= max_checks {
                break;
            }
            for j in (i + 1)..metas.len() {
                if checked >= max_checks {
                    break;
                }
                let overlap = count_tag_overlap(&metas[i].tags, &metas[j].tags);
                if overlap >= merge_threshold {
                    should_merge_candidates.push((
                        metas[i].filename.clone(),
                        metas[j].filename.clone(),
                        overlap,
                    ));
                }
                checked += 1;
            }
        }
    }

    // Two-step expiry
    let now_ts = chrono::Utc::now().timestamp();
    let stale_secs = (config.bloat_stale_days as i64) * 24 * 3600;
    let delete_secs = (config.bloat_delete_days as i64) * 24 * 3600;

    let mut stale_files = Vec::new();
    let mut deletable_files = Vec::new();

    for m in &metas {
        let age = now_ts - m.modified_at;
        if m.status.as_deref() == Some("expired") {
            if age > delete_secs {
                deletable_files.push(m.filename.clone());
            }
        } else if age > stale_secs {
            stale_files.push(m.filename.clone());
        }
    }

    // Capacity-based forced pruning: coldest files first
    if needs_pruning {
        let overflow = total_files.saturating_sub(config.max_knowledge_files);
        if overflow > 0 && deletable_files.len() < overflow {
            let mut sorted = metas.clone();
            sorted.sort_by_key(|m| m.modified_at);
            for m in sorted {
                if deletable_files.len() >= overflow {
                    break;
                }
                if !deletable_files.contains(&m.filename) && !stale_files.contains(&m.filename) {
                    deletable_files.push(m.filename);
                }
            }
        }
    }

    Ok(BloatReport {
        should_merge_candidates,
        stale_files,
        deletable_files,
        total_size_bytes,
        total_files,
        needs_pruning,
    })
}

/// Mark stale files as expired in their frontmatter.
///
/// Returns the number of files marked.
pub fn mark_stale_files(workspace: &Path, stale_files: &[String]) -> usize {
    let knowledge_dir = workspace.join("knowledge");
    let mut marked = 0;

    for filename in stale_files {
        let path = knowledge_dir.join(filename);
        if let Ok(content) = fs::read_to_string(&path) {
            let updated = set_frontmatter_field(&content, "status", "expired");
            if fs::write(&path, updated).is_ok() {
                marked += 1;
                info!(file = filename, "Bloat: marked expired");
            }
        }
    }

    marked
}

/// Delete expired files, recording version entries before removal.
///
/// Returns the number of files deleted.
pub fn delete_expired_files(workspace: &Path, deletable_files: &[String]) -> usize {
    let knowledge_dir = workspace.join("knowledge");
    let mut deleted = 0;

    for filename in deletable_files {
        let path = knowledge_dir.join(filename);
        if path.exists() {
            // Record version before deletion
            if let Ok(before) = fs::read_to_string(&path) {
                let _ = crate::version::record_version(
                    workspace,
                    "delete",
                    filename,
                    Some(&before),
                    None,
                    "bloat",
                );
            }
            if fs::remove_file(&path).is_ok() {
                deleted += 1;
                info!(file = filename, "Bloat: deleted expired file");
            }
        }
    }

    deleted
}

/// Compute Jaccard similarity between two tag sets.
pub fn count_tag_overlap(tags_a: &[String], tags_b: &[String]) -> f64 {
    if tags_a.is_empty() || tags_b.is_empty() {
        return 0.0;
    }
    let set_a: std::collections::HashSet<_> = tags_a.iter().map(|s| s.to_lowercase()).collect();
    let set_b: std::collections::HashSet<_> = tags_b.iter().map(|s| s.to_lowercase()).collect();
    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();
    intersection as f64 / union as f64
}

// ---------------------------------------------------------------------------
// LLM prompt builders (no LLM call, just prompt construction)
// ---------------------------------------------------------------------------

/// Build (system_prompt, user_prompt) for checking if two files overlap.
pub fn build_overlap_check_prompts(content_a: &str, content_b: &str) -> (String, String) {
    let system = r#"判断以下两个知识文件是否在讲同一件事。
返回 true（重叠）或 false（不重叠）。
只返回 true 或 false，不要其他内容。"#
        .to_string();

    let preview_a = safe_truncate(content_a, 400);
    let preview_b = safe_truncate(content_b, 400);
    let user = format!("文件A:\n{}\n\n文件B:\n{}", preview_a, preview_b);

    (system, user)
}

/// Parse the LLM response for overlap check.
pub fn parse_overlap_check_response(text: &str) -> bool {
    text.trim().to_lowercase() == "true"
}

/// Build (system_prompt, user_prompt) for merging two knowledge files.
pub fn build_merge_prompts(content_a: &str, content_b: &str) -> (String, String) {
    let system = r#"将以下两个知识文件合并为一个更完整的文件。
保留两个文件中的所有有用信息，去除重复内容。
只输出合并后的文件内容（纯 markdown，不含 frontmatter）。"#
        .to_string();

    let preview_a = safe_truncate(content_a, 600);
    let preview_b = safe_truncate(content_b, 600);
    let user = format!("文件A:\n{}\n\n文件B:\n{}", preview_a, preview_b);

    (system, user)
}

/// Apply merge result: write merged content to file_a, delete file_b, record versions.
pub fn apply_merge(
    workspace: &Path,
    filename_a: &str,
    filename_b: &str,
    merged_body: &str,
) -> Result<()> {
    let knowledge_dir = workspace.join("knowledge");
    let path_a = knowledge_dir.join(filename_a);
    let path_b = knowledge_dir.join(filename_b);

    // Read original frontmatter from file_a
    let original_a = fs::read_to_string(&path_a).unwrap_or_default();
    let (tags, _) = parse_frontmatter_tags_status(&original_a);
    let tags_yaml = format_yaml_list(&tags);

    let merged_content = format!(
        "---\nsource: compile\ntags: {}\n---\n\n{}",
        tags_yaml, merged_body
    );

    // Record version for file_a (update)
    crate::version::record_version(
        workspace,
        "merge-update",
        filename_a,
        Some(&original_a),
        Some(&merged_content),
        "compile",
    )?;

    fs::write(&path_a, &merged_content)?;

    // Record version for file_b (delete)
    if path_b.exists() {
        let original_b = fs::read_to_string(&path_b).unwrap_or_default();
        crate::version::record_version(
            workspace,
            "merge-delete",
            filename_b,
            Some(&original_b),
            None,
            "compile",
        )?;
        fs::remove_file(&path_b)?;
    }

    info!(a = filename_a, b = filename_b, "Compile: merged files");
    Ok(())
}

/// Build (system_prompt, user_prompt) for generating metadata.
pub fn build_metadata_prompt(body: &str) -> (String, String) {
    let system = r#"分析以下知识文件内容，生成：
1. description: 一句话描述（不超过50字）
2. tags: 3-5 个分类标签
返回严格 JSON 格式：
{"description": "描述", "tags": ["标签1", "标签2", "标签3"]}
只返回 JSON，不要其他内容。"#
        .to_string();

    let preview = safe_truncate(body, 800);
    (system, preview.to_string())
}

/// Parse the LLM metadata generation response.
pub fn parse_metadata_response(text: &str) -> Result<(String, Vec<String>)> {
    let json_str = extract_json(text);
    let v: serde_json::Value =
        serde_json::from_str(&json_str).map_err(|e| anyhow::anyhow!("JSON parse failed: {}", e))?;
    let desc = v["description"].as_str().unwrap_or("").to_string();
    let tags = v["tags"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Ok((desc, tags))
}

/// Apply generated metadata to a knowledge file.
pub fn apply_metadata(
    workspace: &Path,
    filename: &str,
    description: &str,
    tags: &[String],
) -> Result<()> {
    let knowledge_dir = workspace.join("knowledge");
    let path = knowledge_dir.join(filename);
    let content = fs::read_to_string(&path)?;
    let updated = update_frontmatter_fields(&content, description, tags);
    fs::write(&path, &updated)?;
    Ok(())
}

/// Build (system_prompt, user_prompt) for compressing a large knowledge file.
pub fn build_compress_prompt(body: &str) -> (String, String) {
    let system = r#"将以下知识内容压缩为精华版本。
保留所有关键事实、规则和流程，去除冗余描述、重复说明和废话。
目标：将内容缩短到原来的一半左右，同时不丢失任何有用信息。
只输出压缩后的内容（纯 markdown），不要其他文字。"#
        .to_string();

    let preview = safe_truncate(body, 2000);
    (system, preview.to_string())
}

/// Apply compression result to a knowledge file.
pub fn apply_compress(workspace: &Path, filename: &str, compressed_body: &str) -> Result<()> {
    let knowledge_dir = workspace.join("knowledge");
    let path = knowledge_dir.join(filename);
    let original = fs::read_to_string(&path)?;
    let (tags, status) = parse_frontmatter_tags_status(&original);
    let tags_yaml = format_yaml_list(&tags);
    let status_str = status.as_deref().unwrap_or("compressed");

    let compressed_content = format!(
        "---\nstatus: {}\ntags: {}\n---\n\n{}",
        status_str, tags_yaml, compressed_body
    );

    crate::version::record_version(
        workspace,
        "compress",
        filename,
        Some(&original),
        Some(&compressed_content),
        "compile",
    )?;

    fs::write(&path, &compressed_content)?;
    info!(file = filename, "Compile: compressed file");
    Ok(())
}

/// Find the largest files that could benefit from compression.
pub fn find_compress_candidates(workspace: &Path, target_bytes: u64) -> Result<Vec<(String, u64)>> {
    let knowledge_dir = workspace.join("knowledge");
    if !knowledge_dir.exists() {
        return Ok(vec![]);
    }

    let mut files: Vec<(String, u64)> = Vec::new();
    let mut current_total: u64 = 0;

    for entry in fs::read_dir(&knowledge_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map(|e| e == "md").unwrap_or(false) {
            continue;
        }
        let size = fs::metadata(&path)?.len();
        let name = entry.file_name().to_string_lossy().to_string();
        current_total += size;
        if size >= 1000 {
            // Only worth compressing if > 1KB
            files.push((name, size));
        }
    }

    if current_total <= target_bytes {
        return Ok(vec![]);
    }

    // Sort by size descending — largest first
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    Ok(files)
}

// ---------------------------------------------------------------------------
// Frontmatter helpers
// ---------------------------------------------------------------------------

/// Parse tags and status from YAML frontmatter.
fn parse_frontmatter_tags_status(content: &str) -> (Vec<String>, Option<String>) {
    let frontmatter = match extract_frontmatter(content) {
        Some(fm) => fm,
        None => return (vec![], None),
    };

    let mut tags = Vec::new();
    let mut status = None;
    let mut in_tags_array = false;

    for line in frontmatter.lines() {
        let line = line.trim();

        // Status
        if let Some(val) = line.strip_prefix("status:") {
            let val = val.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                status = Some(val.to_string());
            }
            in_tags_array = false;
            continue;
        }

        // Tags — could be inline [a, b] or multi-line array
        if let Some(val) = line.strip_prefix("tags:") {
            let val = val.trim();
            if val.starts_with('[') {
                // Inline array: tags: ["a", "b"]
                tags = parse_inline_array(val);
            } else if val.is_empty() || val == "-" {
                in_tags_array = true;
            }
            continue;
        }

        // Multi-line array item: - "tag"
        if in_tags_array && line.starts_with('-') {
            let item = line
                .trim_start_matches('-')
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            if !item.is_empty() {
                tags.push(item.to_string());
            }
        } else if in_tags_array && !line.starts_with('-') && !line.is_empty() {
            in_tags_array = false;
        }
    }

    (tags, status)
}

/// Extract the raw frontmatter string between --- markers.
fn extract_frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("---")?;
    Some(&rest[..end])
}

/// Parse an inline YAML array like `["a", "b", "c"]`.
fn parse_inline_array(s: &str) -> Vec<String> {
    let inner = s.trim_start_matches('[').trim_end_matches(']');
    inner
        .split(',')
        .map(|item| {
            item.trim()
                .trim_matches('"')
                .trim_matches('\'')
                .trim()
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// Set or update a single frontmatter field.
fn set_frontmatter_field(content: &str, key: &str, value: &str) -> String {
    let (frontmatter, body) = split_frontmatter(content);

    // Check if key already exists
    let mut found = false;
    let mut new_lines = Vec::new();

    for line in frontmatter.lines() {
        if line.trim().starts_with(&format!("{}:", key)) {
            new_lines.push(format!("{}: {}", key, value));
            found = true;
        } else {
            new_lines.push(line.to_string());
        }
    }

    if !found {
        new_lines.push(format!("{}: {}", key, value));
    }

    format!("---\n{}\n---\n{}", new_lines.join("\n"), body)
}

/// Update frontmatter with description and tags.
fn update_frontmatter_fields(content: &str, description: &str, tags: &[String]) -> String {
    let (frontmatter, body) = split_frontmatter(content);
    let tags_yaml = format_yaml_list(tags);

    let mut lines: Vec<String> = Vec::new();
    let mut has_desc = false;
    let mut has_tags = false;

    for line in frontmatter.lines() {
        if line.trim().starts_with("description:") {
            lines.push(format!("description: {}", description));
            has_desc = true;
        } else if line.trim().starts_with("tags:") {
            lines.push(format!("tags: {}", tags_yaml));
            has_tags = true;
        } else {
            lines.push(line.to_string());
        }
    }

    if !has_desc {
        lines.push(format!("description: {}", description));
    }
    if !has_tags {
        lines.push(format!("tags: {}", tags_yaml));
    }

    format!("---\n{}\n---\n{}", lines.join("\n"), body)
}

/// Split content into (frontmatter_text, body).
fn split_frontmatter(content: &str) -> (String, String) {
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            let fm = &rest[..end];
            let body = &rest[end + 3..];
            return (fm.trim().to_string(), body.trim_start().to_string());
        }
    }
    (String::new(), content.to_string())
}

/// Format a list as a YAML inline array: `["a", "b", "c"]`.
fn format_yaml_list(items: &[String]) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    format!(
        "[{}]",
        items
            .iter()
            .map(|t| format!("\"{}\"", t))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Truncate a string to max_chars, respecting char boundaries.
fn safe_truncate(s: &str, max_chars: usize) -> &str {
    if s.chars().count() <= max_chars {
        return s;
    }
    let end = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    &s[..end]
}

/// Extract JSON from text (handles markdown code blocks).
fn extract_json(text: &str) -> String {
    if let Some(start) = text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = text[json_start..].find("```") {
            return text[json_start..json_start + end].trim().to_string();
        }
    }
    if let Some(start) = text.find("```") {
        let json_start = start + 3;
        if let Some(end) = text[json_start..].find("```") {
            return text[json_start..json_start + end].trim().to_string();
        }
    }
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return text[start..=end].to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[expect(dead_code)]
    fn write_knowledge_file(dir: &Path, name: &str, content: &str) {
        fs::create_dir_all(dir.join("knowledge")).unwrap();
        fs::write(dir.join("knowledge").join(name), content).unwrap();
    }

    #[test]
    fn test_check_bloat_empty() {
        let tmp = TempDir::new().unwrap();
        let config = EvolutionConfig::default();
        let report = check_bloat(tmp.path(), &config).unwrap();
        assert_eq!(report.total_files, 0);
        assert!(!report.needs_pruning);
    }

    #[test]
    fn test_check_bloat_detects_expired() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("knowledge");
        fs::create_dir_all(&dir).unwrap();

        // File already marked expired
        let content = "---\nname: test\nstatus: expired\n---\n\nTest content";
        fs::write(dir.join("expired-file.md"), content).unwrap();

        let config = EvolutionConfig {
            bloat_stale_days: 30,
            bloat_delete_days: 60,
            ..Default::default()
        };
        let report = check_bloat(tmp.path(), &config).unwrap();
        // File just created, so age is 0 — neither stale nor deletable
        assert_eq!(report.total_files, 1);
        // The file has status: expired but age is 0s < delete_threshold
        assert!(report.deletable_files.is_empty());
    }

    #[test]
    fn test_parse_frontmatter_tags_status() {
        let content = "---\nname: test\ntags: [\"退款\", \"售后\"]\nstatus: active\n---\n\nBody";
        let (tags, status) = parse_frontmatter_tags_status(content);
        assert_eq!(tags, vec!["退款", "售后"]);
        assert_eq!(status, Some("active".to_string()));
    }

    #[test]
    fn test_parse_frontmatter_multiline_tags() {
        let content = "---\nname: test\ntags:\n  - \"退款\"\n  - \"售后\"\n---\n\nBody";
        let (tags, status) = parse_frontmatter_tags_status(content);
        assert_eq!(tags, vec!["退款", "售后"]);
        assert!(status.is_none());
    }

    #[test]
    fn test_parse_frontmatter_no_tags() {
        let content = "---\nname: test\n---\n\nBody";
        let (tags, status) = parse_frontmatter_tags_status(content);
        assert!(tags.is_empty());
        assert!(status.is_none());
    }

    #[test]
    fn test_count_tag_overlap() {
        let a = vec!["退款".to_string(), "售后".to_string()];
        let b = vec!["退款".to_string(), "物流".to_string()];
        let overlap = count_tag_overlap(&a, &b);
        // Intersection: 退款 (1), Union: 退款, 售后, 物流 (3) → 1/3
        assert!((overlap - 0.333).abs() < 0.01);

        // No overlap
        let c = vec!["退款".to_string()];
        let d = vec!["物流".to_string()];
        assert_eq!(count_tag_overlap(&c, &d), 0.0);

        // Empty
        assert_eq!(count_tag_overlap(&[], &d), 0.0);

        // Identical
        assert_eq!(count_tag_overlap(&a, &a), 1.0);
    }

    #[test]
    fn test_set_frontmatter_field() {
        let content = "---\nname: test\n---\n\nBody";
        let updated = set_frontmatter_field(content, "status", "expired");
        assert!(updated.contains("status: expired"));
        assert!(updated.contains("name: test"));
        assert!(updated.contains("Body"));
    }

    #[test]
    fn test_set_frontmatter_field_existing() {
        let content = "---\nname: test\nstatus: active\n---\n\nBody";
        let updated = set_frontmatter_field(content, "status", "expired");
        assert!(updated.contains("status: expired"));
        assert!(!updated.contains("status: active"));
    }

    #[test]
    fn test_update_frontmatter_fields() {
        let content = "---\nname: test\n---\n\nBody";
        let tags = vec!["退款".to_string(), "售后".to_string()];
        let updated = update_frontmatter_fields(content, "退款政策说明", &tags);
        assert!(updated.contains("description: 退款政策说明"));
        assert!(updated.contains("tags: [\"退款\", \"售后\"]"));
    }

    #[test]
    fn test_mark_stale_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("knowledge");
        fs::create_dir_all(&dir).unwrap();

        let content = "---\nname: test\n---\n\nBody";
        fs::write(dir.join("file1.md"), content).unwrap();
        fs::write(dir.join("file2.md"), content).unwrap();

        let marked = mark_stale_files(
            tmp.path(),
            &["file1.md".to_string(), "file2.md".to_string()],
        );
        assert_eq!(marked, 2);

        let updated = fs::read_to_string(dir.join("file1.md")).unwrap();
        assert!(updated.contains("status: expired"));
    }

    #[test]
    fn test_delete_expired_files() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("knowledge");
        fs::create_dir_all(&dir).unwrap();
        // Create history dir for version recording
        fs::create_dir_all(tmp.path().join("history")).unwrap();

        let content = "---\nname: test\n---\n\nBody";
        fs::write(dir.join("old.md"), content).unwrap();
        assert!(dir.join("old.md").exists());

        let deleted = delete_expired_files(tmp.path(), &["old.md".to_string()]);
        assert_eq!(deleted, 1);
        assert!(!dir.join("old.md").exists());

        // Version recorded
        let versions = crate::version::get_all_versions(tmp.path()).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].action, "delete");
    }

    #[test]
    fn test_build_overlap_check_prompts() {
        let (sys, user) = build_overlap_check_prompts("Content A", "Content B");
        assert!(sys.contains("同一件事"));
        assert!(user.contains("Content A"));
        assert!(user.contains("Content B"));
    }

    #[test]
    fn test_parse_overlap_check_response() {
        assert!(parse_overlap_check_response("true"));
        assert!(parse_overlap_check_response("True"));
        assert!(parse_overlap_check_response(" true "));
        assert!(!parse_overlap_check_response("false"));
        assert!(!parse_overlap_check_response("maybe"));
    }

    #[test]
    fn test_build_merge_prompts() {
        let (sys, user) = build_merge_prompts("AAA", "BBB");
        assert!(sys.contains("合并"));
        assert!(user.contains("AAA"));
    }

    #[test]
    fn test_parse_metadata_response() {
        let json = r#"{"description": "退款政策", "tags": ["退款", "售后"]}"#;
        let (desc, tags) = parse_metadata_response(json).unwrap();
        assert_eq!(desc, "退款政策");
        assert_eq!(tags, vec!["退款", "售后"]);
    }

    #[test]
    fn test_parse_metadata_response_in_markdown() {
        let text = "```json\n{\"description\": \"测试\", \"tags\": [\"a\"]}\n```";
        let (desc, tags) = parse_metadata_response(text).unwrap();
        assert_eq!(desc, "测试");
        assert_eq!(tags, vec!["a"]);
    }

    #[test]
    fn test_build_compress_prompt() {
        let (sys, user) = build_compress_prompt("Long content here");
        assert!(sys.contains("压缩"));
        assert!(user.contains("Long content"));
    }

    #[test]
    fn test_format_yaml_list() {
        assert_eq!(format_yaml_list(&[]), "[]");
        assert_eq!(format_yaml_list(&["a".to_string()]), "[\"a\"]");
        assert_eq!(
            format_yaml_list(&["a".to_string(), "b".to_string()]),
            "[\"a\", \"b\"]"
        );
    }

    #[test]
    fn test_safe_truncate() {
        assert_eq!(safe_truncate("hello", 10), "hello");
        assert_eq!(safe_truncate("hello world", 5), "hello");
        // UTF-8 boundary safe
        let chinese = "你好世界测试";
        let truncated = safe_truncate(chinese, 2);
        assert_eq!(truncated, "你好");
    }

    #[test]
    fn test_extract_json() {
        assert_eq!(extract_json("```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(extract_json("plain text {\"a\":1} more"), "{\"a\":1}");
    }

    #[test]
    fn test_split_frontmatter() {
        let content = "---\nname: test\n---\n\nBody text";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("name: test"));
        assert_eq!(body, "Body text");
    }
}
