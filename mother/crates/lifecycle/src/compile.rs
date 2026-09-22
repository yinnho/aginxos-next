//! Knowledge compilation — generate metadata, merge overlaps, compress large files.
//!
//! This module provides the orchestration logic for knowledge compilation.
//! The actual LLM prompt builders and response parsers are in `bloat.rs`.
//! The kernel calls these functions and feeds LLM results back.
//!
//! Compile pipeline:
//! 1. `find_files_needing_metadata()` → kernel calls LLM → `apply_metadata()`
//! 2. `check_bloat()` → merge candidates → kernel calls LLM → `apply_merge()`
//! 3. `find_compress_candidates()` → kernel calls LLM → `apply_compress()`
//! 4. `rebuild_index()` → update MEMORY.md

use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tracing::{info, warn};

use crate::bloat::{
    apply_compress, apply_merge, apply_metadata, build_compress_prompt, build_merge_prompts,
    build_metadata_prompt, build_overlap_check_prompts, check_bloat, delete_expired_files,
    find_compress_candidates, mark_stale_files, parse_metadata_response,
    parse_overlap_check_response,
};
use crate::evolution_config::EvolutionConfig;

/// A file that needs metadata (description + tags) generated.
#[derive(Debug, Clone)]
pub struct FileNeedingMetadata {
    pub filename: String,
    pub body: String,
}

/// Result of a compile run.
#[derive(Debug, Default)]
pub struct CompileResult {
    pub metadata_generated: usize,
    pub files_merged: usize,
    pub stale_marked: usize,
    pub expired_deleted: usize,
    pub files_compressed: usize,
    pub skipped_unchanged: usize,
    pub skipped_by_manifest: usize,
    pub errors: Vec<String>,
}

/// Find knowledge files that lack description or tags in their frontmatter.
pub fn find_files_needing_metadata(workspace: &Path) -> Result<Vec<FileNeedingMetadata>> {
    let knowledge_dir = workspace.join("knowledge");
    if !knowledge_dir.exists() {
        return Ok(vec![]);
    }

    let mut files = Vec::new();

    for entry in fs::read_dir(&knowledge_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.extension().map(|e| e == "md").unwrap_or(false) {
            continue;
        }

        let content = fs::read_to_string(&path).unwrap_or_default();
        let filename = entry.file_name().to_string_lossy().to_string();

        // Check if description or tags are missing
        let needs = needs_metadata(&content);
        if !needs {
            continue;
        }

        // Extract body (without frontmatter)
        let body = extract_body(&content);
        if body.trim().is_empty() {
            continue;
        }

        files.push(FileNeedingMetadata { filename, body });
    }

    Ok(files)
}

/// Check if frontmatter is missing description or tags.
fn needs_metadata(content: &str) -> bool {
    let fm = match extract_frontmatter_text(content) {
        Some(f) => f,
        None => return true, // No frontmatter at all
    };

    let has_desc = fm.lines().any(|line| {
        line.trim().starts_with("description:") && line.trim().len() > "description:".len() + 2
    });
    let has_tags = fm
        .lines()
        .any(|line| line.trim().starts_with("tags:") && (line.contains("[") || line.contains("-")));

    !has_desc || !has_tags
}

/// Extract body text without frontmatter.
fn extract_body(content: &str) -> String {
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            return rest[end + 3..].trim().to_string();
        }
    }
    content.to_string()
}

/// Extract frontmatter text.
fn extract_frontmatter_text(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("---")?;
    Some(&rest[..end])
}

/// Compute SHA-256 content hash of a file.
/// Returns None if the file doesn't exist or can't be read.
fn content_hash(path: &Path) -> Option<String> {
    let content = fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&content);
    Some(format!("{:x}", hasher.finalize()))
}

// ---------------------------------------------------------------------------
// Incremental compile manifest
// ---------------------------------------------------------------------------

/// Per-file entry in the compile manifest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    /// SHA-256 hash of the file content at last compile.
    pub hash: String,
    /// ISO timestamp of last compile.
    pub compiled_at: String,
}

/// Load the compile manifest from `.lifecycle/manifest.json`.
pub fn load_manifest(workspace: &Path) -> std::collections::HashMap<String, ManifestEntry> {
    let path = workspace.join(".lifecycle/manifest.json");
    if !path.exists() {
        return std::collections::HashMap::new();
    }
    match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => std::collections::HashMap::new(),
    }
}

/// Save the compile manifest.
pub fn save_manifest(
    workspace: &Path,
    manifest: &std::collections::HashMap<String, ManifestEntry>,
) -> Result<()> {
    let dir = workspace.join(".lifecycle");
    fs::create_dir_all(&dir)?;
    let path = dir.join("manifest.json");
    let json = serde_json::to_string_pretty(manifest)?;
    fs::write(&path, json)?;
    Ok(())
}

/// Return knowledge files whose content hash differs from the manifest,
/// or that are new (not in manifest). Also returns the set of all current filenames
/// so the caller can detect deleted files.
pub fn find_changed_files(
    workspace: &Path,
    manifest: &std::collections::HashMap<String, ManifestEntry>,
) -> (Vec<String>, Vec<String>) {
    let knowledge_dir = workspace.join("knowledge");
    let mut changed = Vec::new();
    let mut current_files = Vec::new();

    if let Ok(entries) = fs::read_dir(&knowledge_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().map(|e| e == "md").unwrap_or(false) {
                continue;
            }
            let filename = entry.file_name().to_string_lossy().to_string();
            current_files.push(filename.clone());

            let hash = content_hash(&path);
            match (manifest.get(&filename), hash) {
                (Some(entry), Some(h)) if entry.hash == h => {
                    // Unchanged — skip
                }
                _ => {
                    changed.push(filename);
                }
            }
        }
    }

    (changed, current_files)
}

/// Run the full compile pipeline with LLM callbacks.
///
/// The `llm_call` closure receives (system_prompt, user_prompt, max_tokens)
/// and returns the LLM response text. This keeps the lifecycle crate LLM-free.
pub fn run_compile<F>(workspace: &Path, config: &EvolutionConfig, llm_call: &F) -> CompileResult
where
    F: Fn(&str, &str, u32) -> Result<String> + ?Sized,
{
    let mut result = CompileResult::default();
    let now = chrono::Utc::now().to_rfc3339();

    // Load incremental compile manifest
    let mut manifest = load_manifest(workspace);

    // Determine which files changed since last compile
    let (changed_files, current_files) = find_changed_files(workspace, &manifest);

    // Remove deleted files from manifest
    let before = manifest.len();
    manifest.retain(|name, _| current_files.contains(name));
    let removed_from_manifest = before - manifest.len();
    if removed_from_manifest > 0 {
        info!(
            removed = removed_from_manifest,
            "Cleaned stale manifest entries"
        );
    }

    // Phase 1: Generate metadata for files that need it (only changed files)
    match find_files_needing_metadata(workspace) {
        Ok(files) => {
            for file in files {
                // Skip files unchanged since last compile
                if !changed_files.contains(&file.filename) && manifest.contains_key(&file.filename)
                {
                    result.skipped_by_manifest += 1;
                    continue;
                }

                let file_path = workspace.join("knowledge").join(&file.filename);
                let hash_before = content_hash(&file_path);

                let (system, user) = build_metadata_prompt(&file.body);
                match llm_call(&system, &user, 256) {
                    Ok(response) => {
                        match parse_metadata_response(&response) {
                            Ok((desc, tags)) => {
                                if let Err(e) =
                                    apply_metadata(workspace, &file.filename, &desc, &tags)
                                {
                                    result
                                        .errors
                                        .push(format!("metadata {}: {}", file.filename, e));
                                } else {
                                    // Check if content actually changed
                                    let hash_after = content_hash(&file_path);
                                    if hash_before.as_deref() == hash_after.as_deref()
                                        && hash_before.is_some()
                                    {
                                        result.skipped_unchanged += 1;
                                    } else {
                                        result.metadata_generated += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                result
                                    .errors
                                    .push(format!("parse metadata {}: {}", file.filename, e));
                            }
                        }
                    }
                    Err(e) => {
                        result
                            .errors
                            .push(format!("llm metadata {}: {}", file.filename, e));
                    }
                }

                // Update manifest entry for this file
                if let Some(hash) = content_hash(&file_path) {
                    manifest.insert(
                        file.filename.clone(),
                        ManifestEntry {
                            hash,
                            compiled_at: now.clone(),
                        },
                    );
                }
            }
        }
        Err(e) => {
            result.errors.push(format!("scan metadata: {}", e));
        }
    }

    // Phase 2: Bloat detection + merge + cleanup
    match check_bloat(workspace, config) {
        Ok(report) => {
            // Merge overlapping files
            for (file_a, file_b, _similarity) in &report.should_merge_candidates {
                let knowledge_dir = workspace.join("knowledge");
                let path_a = knowledge_dir.join(file_a);
                let path_b = knowledge_dir.join(file_b);

                let content_a = fs::read_to_string(&path_a).unwrap_or_default();
                let content_b = fs::read_to_string(&path_b).unwrap_or_default();

                // Check overlap with LLM
                let (sys, user) = build_overlap_check_prompts(&content_a, &content_b);
                match llm_call(&sys, &user, 128) {
                    Ok(resp) => {
                        if parse_overlap_check_response(&resp) {
                            let (merge_sys, merge_user) =
                                build_merge_prompts(&content_a, &content_b);
                            match llm_call(&merge_sys, &merge_user, 2048) {
                                Ok(merged) => {
                                    if let Err(e) = apply_merge(workspace, file_a, file_b, &merged)
                                    {
                                        result
                                            .errors
                                            .push(format!("merge {} + {}: {}", file_a, file_b, e));
                                    } else {
                                        result.files_merged += 1;
                                    }
                                }
                                Err(e) => {
                                    result.errors.push(format!("merge llm {}: {}", file_a, e));
                                }
                            }
                        }
                    }
                    Err(e) => {
                        result.errors.push(format!("overlap check: {}", e));
                    }
                }
            }

            // Mark stale files
            result.stale_marked = mark_stale_files(workspace, &report.stale_files);

            // Delete expired files
            result.expired_deleted = delete_expired_files(workspace, &report.deletable_files);

            // Compress large files if still over capacity
            let max_total_bytes = (config.knowledge_capacity_mb as u64) * 1024 * 1024;
            if let Ok(candidates) = find_compress_candidates(workspace, max_total_bytes) {
                for (filename, _) in candidates {
                    let knowledge_dir = workspace.join("knowledge");
                    let path = knowledge_dir.join(&filename);
                    let hash_before = content_hash(&path);
                    if let Ok(content) = fs::read_to_string(&path) {
                        let body = extract_body(&content);
                        let (sys, user) = build_compress_prompt(&body);
                        match llm_call(&sys, &user, 2048) {
                            Ok(compressed) => {
                                if let Err(e) = apply_compress(workspace, &filename, &compressed) {
                                    result.errors.push(format!("compress {}: {}", filename, e));
                                } else {
                                    let hash_after = content_hash(&path);
                                    if hash_before.as_deref() == hash_after.as_deref()
                                        && hash_before.is_some()
                                    {
                                        result.skipped_unchanged += 1;
                                    } else {
                                        result.files_compressed += 1;
                                    }
                                }
                            }
                            Err(e) => {
                                result
                                    .errors
                                    .push(format!("compress llm {}: {}", filename, e));
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            result.errors.push(format!("bloat check: {}", e));
        }
    }

    // Phase 3: Rebuild MEMORY.md
    if let Err(e) = crate::evolution::update_memory_index(workspace) {
        warn!(error = %e, "Compile: failed to rebuild MEMORY.md");
    }

    // Save updated manifest
    if let Err(e) = save_manifest(workspace, &manifest) {
        warn!(error = %e, "Compile: failed to save manifest");
    }

    info!(
        metadata = result.metadata_generated,
        merged = result.files_merged,
        stale = result.stale_marked,
        deleted = result.expired_deleted,
        compressed = result.files_compressed,
        skipped = result.skipped_unchanged,
        skipped_manifest = result.skipped_by_manifest,
        errors = result.errors.len(),
        "Compile complete"
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn setup_workspace(tmp: &TempDir) -> PathBuf {
        let ws = tmp.path().to_path_buf();
        fs::create_dir_all(ws.join("knowledge")).unwrap();
        fs::create_dir_all(ws.join("history")).unwrap();
        ws
    }

    #[test]
    fn test_needs_metadata_no_frontmatter() {
        assert!(needs_metadata("Just body text"));
    }

    #[test]
    fn test_needs_metadata_empty_frontmatter() {
        let content = "---\n---\n\nBody";
        assert!(needs_metadata(content));
    }

    #[test]
    fn test_needs_metadata_partial() {
        // Has description but no tags
        let content = "---\ndescription: test desc\n---\n\nBody";
        assert!(needs_metadata(content));
    }

    #[test]
    fn test_needs_metadata_complete() {
        let content = "---\ndescription: test desc\ntags: [\"a\"]\n---\n\nBody";
        assert!(!needs_metadata(content));
    }

    #[test]
    fn test_find_files_needing_metadata() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        // File without metadata
        fs::write(
            ws.join("knowledge/no-meta.md"),
            "---\nname: test\n---\n\nSome content",
        )
        .unwrap();

        // File with metadata
        fs::write(
            ws.join("knowledge/with-meta.md"),
            "---\ndescription: test\ntags: [\"a\"]\n---\n\nContent",
        )
        .unwrap();

        // Empty body file
        fs::write(ws.join("knowledge/empty.md"), "---\n---\n\n").unwrap();

        let files = find_files_needing_metadata(&ws).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "no-meta.md");
    }

    #[test]
    fn test_extract_body() {
        assert_eq!(extract_body("---\nname: x\n---\n\nContent"), "Content");
        assert_eq!(extract_body("No frontmatter"), "No frontmatter");
    }

    #[test]
    fn test_content_hash_detects_changes() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        let file_path = ws.join("knowledge/test.md");
        fs::write(&file_path, "original content").unwrap();

        let hash1 = content_hash(&file_path);
        assert!(hash1.is_some());

        fs::write(&file_path, "modified content").unwrap();
        let hash2 = content_hash(&file_path);
        assert_ne!(hash1, hash2, "hash should change when content changes");

        // Same content should produce same hash
        fs::write(&file_path, "modified content").unwrap();
        let hash3 = content_hash(&file_path);
        assert_eq!(hash2, hash3, "hash should be stable for same content");
    }

    #[test]
    fn test_content_hash_missing_file() {
        let hash = content_hash(Path::new("/nonexistent/file.md"));
        assert!(hash.is_none());
    }

    #[test]
    fn test_manifest_save_load_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        let mut manifest = std::collections::HashMap::new();
        manifest.insert(
            "test.md".to_string(),
            ManifestEntry {
                hash: "abc123".to_string(),
                compiled_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );
        manifest.insert(
            "other.md".to_string(),
            ManifestEntry {
                hash: "def456".to_string(),
                compiled_at: "2025-01-02T00:00:00Z".to_string(),
            },
        );

        save_manifest(&ws, &manifest).unwrap();
        let loaded = load_manifest(&ws);

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["test.md"].hash, "abc123");
        assert_eq!(loaded["other.md"].compiled_at, "2025-01-02T00:00:00Z");
    }

    #[test]
    fn test_manifest_load_missing() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);
        let manifest = load_manifest(&ws);
        assert!(manifest.is_empty());
    }

    #[test]
    fn test_find_changed_files_detects_new_and_modified() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        // Create two files
        fs::write(ws.join("knowledge/a.md"), "content A").unwrap();
        fs::write(ws.join("knowledge/b.md"), "content B").unwrap();

        let path_a = ws.join("knowledge/a.md");
        let hash_a = content_hash(&path_a).unwrap();

        // Manifest only has a.md
        let mut manifest = std::collections::HashMap::new();
        manifest.insert(
            "a.md".to_string(),
            ManifestEntry {
                hash: hash_a,
                compiled_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );

        let (changed, current) = find_changed_files(&ws, &manifest);
        assert!(
            changed.contains(&"b.md".to_string()),
            "b.md is new, should be changed"
        );
        assert!(
            !changed.contains(&"a.md".to_string()),
            "a.md hash matches, should NOT be changed"
        );
        assert_eq!(current.len(), 2);

        // Modify a.md
        fs::write(ws.join("knowledge/a.md"), "modified A").unwrap();
        let (changed2, _) = find_changed_files(&ws, &manifest);
        assert!(
            changed2.contains(&"a.md".to_string()),
            "a.md changed, should be detected"
        );
        assert!(changed2.contains(&"b.md".to_string()), "b.md still new");
    }

    #[test]
    fn test_find_changed_files_detects_deletions() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(ws.join("knowledge/a.md"), "content A").unwrap();

        let mut manifest = std::collections::HashMap::new();
        manifest.insert(
            "a.md".to_string(),
            ManifestEntry {
                hash: "old_hash".to_string(),
                compiled_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );
        manifest.insert(
            "deleted.md".to_string(),
            ManifestEntry {
                hash: "dead".to_string(),
                compiled_at: "2025-01-01T00:00:00Z".to_string(),
            },
        );

        let (changed, current) = find_changed_files(&ws, &manifest);
        assert_eq!(current.len(), 1, "Only a.md exists on disk");
        assert!(
            !current.contains(&"deleted.md".to_string()),
            "deleted.md not on disk"
        );
        assert!(
            changed.contains(&"a.md".to_string()),
            "a.md hash differs from manifest"
        );
    }

    #[test]
    fn test_run_compile_with_mock_llm() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        // Create a file needing metadata
        fs::write(
            ws.join("knowledge/test.md"),
            "---\nname: test\n---\n\nThis is a test knowledge file about refunds.",
        )
        .unwrap();

        let config = EvolutionConfig {
            max_knowledge_files: 200,
            ..Default::default()
        };

        // Mock LLM that returns metadata
        let mock_llm = |_sys: &str, _user: &str, _max_tokens: u32| -> Result<String> {
            Ok(r#"{"description": "退款政策", "tags": ["退款", "售后"]}"#.to_string())
        };

        let result = run_compile(&ws, &config, &mock_llm);
        assert_eq!(result.metadata_generated, 1);
        assert!(result.errors.is_empty());

        // Verify metadata was written
        let content = fs::read_to_string(ws.join("knowledge/test.md")).unwrap();
        assert!(content.contains("description: 退款政策"));
        assert!(content.contains("tags: [\"退款\", \"售后\"]"));
    }
}
