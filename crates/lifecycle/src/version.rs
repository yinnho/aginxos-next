//! Knowledge version management — change tracking via JSONL log.
//!
//! Ported from openclone-core/src/version.rs with path adaptation:
//! - `clone_dir/history/versions.jsonl` → `workspace/history/versions.jsonl`
//! - Uses `chrono` for timestamps instead of raw Unix epoch

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

/// A single version record.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VersionEntry {
    /// ISO 8601 timestamp
    pub timestamp: String,
    /// Action type: create / update / delete / verify / rollback
    pub action: String,
    /// Filename (relative to knowledge/)
    pub file: String,
    /// Content before change (None = new file)
    pub before: Option<String>,
    /// Content after change (None = deleted)
    pub after: Option<String>,
    /// Source: evolution / user / verify / rollback
    pub source: String,
    /// Whether a human has verified this change
    pub verified: bool,
}

/// Append a version record to the JSONL log.
pub fn record_version(
    workspace: &Path,
    action: &str,
    file: &str,
    before: Option<&str>,
    after: Option<&str>,
    source: &str,
) -> Result<()> {
    let history_dir = workspace.join("history");
    fs::create_dir_all(&history_dir)?;

    let versions_path = history_dir.join("versions.jsonl");
    let entry = VersionEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        action: action.to_string(),
        file: file.to_string(),
        before: before.map(String::from),
        after: after.map(String::from),
        source: source.to_string(),
        verified: source == "user" || source == "verify",
    };

    let mut line = serde_json::to_string(&entry)?;
    line.push('\n');

    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&versions_path)?;
    f.write_all(line.as_bytes())?;

    Ok(())
}

/// Get version history for a specific file.
pub fn get_file_history(workspace: &Path, filename: &str) -> Result<Vec<VersionEntry>> {
    let versions_path = workspace.join("history/versions.jsonl");
    if !versions_path.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(&versions_path)?;
    let entries: Vec<VersionEntry> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<VersionEntry>(line).ok())
        .filter(|e| e.file == filename)
        .collect();

    Ok(entries)
}

/// Get all version records.
pub fn get_all_versions(workspace: &Path) -> Result<Vec<VersionEntry>> {
    let versions_path = workspace.join("history/versions.jsonl");
    if !versions_path.exists() {
        return Ok(vec![]);
    }

    let content = fs::read_to_string(&versions_path)?;
    let entries: Vec<VersionEntry> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<VersionEntry>(line).ok())
        .collect();

    Ok(entries)
}

/// Rollback a knowledge file to its previous version.
///
/// Finds the most recent version entry for the file and restores the `before`
/// content. Three cases:
/// 1. File exists + has previous content → restore previous
/// 2. File exists + no previous → delete (was newly created)
/// 3. File missing + has previous → restore deleted file
///
/// After rollback, MEMORY.md index is rebuilt.
pub fn rollback_file(workspace: &Path, filename: &str) -> Result<()> {
    let knowledge_dir = workspace.join("knowledge");
    let file_path = knowledge_dir.join(filename);

    let current = if file_path.exists() {
        Some(fs::read_to_string(&file_path)?)
    } else {
        None
    };

    let history = get_file_history(workspace, filename)?;
    if history.is_empty() {
        anyhow::bail!("{} has no version history", filename);
    }

    // Find the most recent version's `before` content
    let previous = history.last().and_then(|v| v.before.clone());

    match (&current, previous) {
        (Some(cur), Some(prev)) => {
            fs::write(&file_path, &prev)?;
            record_version(
                workspace,
                "rollback",
                filename,
                Some(cur.as_str()),
                Some(&prev),
                "rollback",
            )?;
        }
        (Some(cur), None) => {
            let cur_content = cur.clone();
            fs::remove_file(&file_path)?;
            record_version(
                workspace,
                "rollback",
                filename,
                Some(cur_content.as_str()),
                None,
                "rollback",
            )?;
        }
        (None, Some(prev)) => {
            fs::create_dir_all(&knowledge_dir)?;
            fs::write(&file_path, &prev)?;
            record_version(
                workspace,
                "rollback",
                filename,
                None,
                Some(&prev),
                "rollback",
            )?;
        }
        (None, None) => {
            anyhow::bail!("cannot rollback {}: no history available", filename);
        }
    }

    // Rebuild MEMORY.md index
    crate::evolution::update_memory_index(workspace)?;

    Ok(())
}

/// Mark a version entry as verified by a human.
///
/// Also upgrades the knowledge file's `confidence` field to EXTRACTED.
pub fn verify_version(workspace: &Path, filename: &str) -> Result<()> {
    let versions_path = workspace.join("history/versions.jsonl");
    if !versions_path.exists() {
        anyhow::bail!("no version history exists");
    }

    let content = fs::read_to_string(&versions_path)?;
    let mut entries: Vec<VersionEntry> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<VersionEntry>(line).ok())
        .collect();

    let mut found = false;
    for entry in entries.iter_mut().rev() {
        if entry.file == filename && !entry.verified {
            entry.verified = true;
            entry.action = format!("{}+verify", entry.action);
            found = true;
            break; // only verify the latest unverified
        }
    }

    if !found {
        anyhow::bail!("no unverified entry for {}", filename);
    }

    // Upgrade confidence in the knowledge file to EXTRACTED
    let knowledge_dir = workspace.join("knowledge");
    let file_path = knowledge_dir.join(filename);
    if file_path.exists() {
        if let Ok(file_content) = fs::read_to_string(&file_path) {
            let updated = upgrade_confidence(&file_content, "EXTRACTED");
            let _ = fs::write(&file_path, &updated);
        }
    }

    // Rewrite the JSONL file
    let mut f = fs::File::create(&versions_path)?;
    for entry in &entries {
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');
        f.write_all(line.as_bytes())?;
    }

    Ok(())
}

/// Upgrade the confidence field in frontmatter to the given value.
/// If no confidence field exists, adds one.
pub fn upgrade_confidence(content: &str, confidence: &str) -> String {
    if let Some(fm_end) = content.find("\n---") {
        let (fm, rest) = content.split_at(fm_end);
        if fm.contains("confidence:") {
            // Replace existing confidence value
            let updated = fm
                .lines()
                .map(|line| {
                    if line.starts_with("confidence:") {
                        format!("confidence: {}", confidence)
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("{}{}", updated, rest)
        } else {
            // Add confidence field before closing ---
            format!("{}\nconfidence: {}{}", fm, confidence, rest)
        }
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_record_and_read_version() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        record_version(
            workspace,
            "create",
            "test-knowledge.md",
            None,
            Some("content"),
            "evolution",
        )
        .unwrap();

        let history = get_file_history(workspace, "test-knowledge.md").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].action, "create");
        assert_eq!(history[0].file, "test-knowledge.md");
        assert!(history[0].before.is_none());
        assert_eq!(history[0].after.as_deref(), Some("content"));
        assert!(!history[0].verified);
    }

    #[test]
    fn test_multiple_versions() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        record_version(workspace, "create", "a.md", None, Some("v1"), "evolution").unwrap();
        record_version(
            workspace,
            "update",
            "a.md",
            Some("v1"),
            Some("v2"),
            "evolution",
        )
        .unwrap();
        record_version(workspace, "create", "b.md", None, Some("other"), "user").unwrap();

        let all = get_all_versions(workspace).unwrap();
        assert_eq!(all.len(), 3);

        let a_history = get_file_history(workspace, "a.md").unwrap();
        assert_eq!(a_history.len(), 2);
    }

    #[test]
    fn test_user_source_is_verified() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        record_version(workspace, "create", "x.md", None, Some("content"), "user").unwrap();

        let history = get_file_history(workspace, "x.md").unwrap();
        assert!(history[0].verified);
    }

    #[test]
    fn test_empty_history() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        let history = get_file_history(workspace, "nonexistent.md").unwrap();
        assert!(history.is_empty());

        let all = get_all_versions(workspace).unwrap();
        assert!(all.is_empty());
    }

    #[test]
    fn test_rollback_restore_previous() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let knowledge_dir = workspace.join("knowledge");
        fs::create_dir_all(&knowledge_dir).unwrap();

        // Create → update
        record_version(
            workspace,
            "create",
            "test.md",
            None,
            Some("v1"),
            "evolution",
        )
        .unwrap();
        fs::write(knowledge_dir.join("test.md"), "v1").unwrap();

        record_version(
            workspace,
            "update",
            "test.md",
            Some("v1"),
            Some("v2"),
            "evolution",
        )
        .unwrap();
        fs::write(knowledge_dir.join("test.md"), "v2").unwrap();

        // Rollback: v2 → v1
        rollback_file(workspace, "test.md").unwrap();

        let content = fs::read_to_string(knowledge_dir.join("test.md")).unwrap();
        assert_eq!(content, "v1");

        // Version log should have rollback entry
        let all = get_all_versions(workspace).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[2].action, "rollback");
        assert_eq!(all[2].before.as_deref(), Some("v2"));
        assert_eq!(all[2].after.as_deref(), Some("v1"));
    }

    #[test]
    fn test_rollback_delete_new_file() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let knowledge_dir = workspace.join("knowledge");
        fs::create_dir_all(&knowledge_dir).unwrap();

        // Create only (no previous version)
        record_version(
            workspace,
            "create",
            "new.md",
            None,
            Some("content"),
            "evolution",
        )
        .unwrap();
        fs::write(knowledge_dir.join("new.md"), "content").unwrap();

        // Rollback: delete the new file
        rollback_file(workspace, "new.md").unwrap();

        assert!(!knowledge_dir.join("new.md").exists());

        let all = get_all_versions(workspace).unwrap();
        assert_eq!(all[1].action, "rollback");
        assert!(all[1].after.is_none());
    }

    #[test]
    fn test_rollback_restore_deleted_file() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let knowledge_dir = workspace.join("knowledge");
        fs::create_dir_all(&knowledge_dir).unwrap();

        // Create then delete
        record_version(
            workspace,
            "create",
            "x.md",
            None,
            Some("original"),
            "evolution",
        )
        .unwrap();
        record_version(
            workspace,
            "delete",
            "x.md",
            Some("original"),
            None,
            "evolution",
        )
        .unwrap();

        // File is deleted
        assert!(!knowledge_dir.join("x.md").exists());

        // Rollback: restore the deleted file
        rollback_file(workspace, "x.md").unwrap();

        let content = fs::read_to_string(knowledge_dir.join("x.md")).unwrap();
        assert_eq!(content, "original");
    }

    #[test]
    fn test_rollback_no_history() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        let result = rollback_file(workspace, "noexist.md");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_version() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        record_version(workspace, "create", "a.md", None, Some("c"), "evolution").unwrap();

        verify_version(workspace, "a.md").unwrap();

        let history = get_file_history(workspace, "a.md").unwrap();
        assert!(history.iter().all(|e| e.verified));
    }
}
