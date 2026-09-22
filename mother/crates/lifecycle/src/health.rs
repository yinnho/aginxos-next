//! Knowledge health check — lint quality issues in knowledge files.
//!
//! Checks for common problems:
//! - Empty files
//! - Oversized files (> 50KB without compression)
//! - Missing or malformed frontmatter
//! - Missing recommended frontmatter fields (source, type, confidence)
//! - Invalid schema field values
//! - Missing dual-layer separator (second `---`)
//! - Duplicate content (by filename)
//! - Files with expired status still present
//! - Broken cross-references in MEMORY.md

use std::fs;
use std::path::Path;

/// A single health issue found in the knowledge base.
#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthIssue {
    pub filename: String,
    pub severity: IssueSeverity,
    pub kind: IssueKind,
    pub message: String,
}

/// Issue severity level.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum IssueSeverity {
    /// Can be auto-fixed.
    Warning,
    /// Needs manual attention.
    Error,
}

/// Type of health issue.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum IssueKind {
    EmptyFile,
    OversizedFile,
    MissingFrontmatter,
    MalformedFrontmatter,
    DuplicateContent,
    ExpiredStatus,
    BrokenReference,
    MissingName,
    AmbiguousConfidence,
    InvalidConfidence,
    MissingDualLayerSeparator,
    MissingSourceField,
    MissingDescription,
}

/// Result of a health check.
#[derive(Debug, Default, serde::Serialize)]
pub struct HealthReport {
    pub issues: Vec<HealthIssue>,
    pub total_files: usize,
    pub healthy_files: usize,
}

impl HealthReport {
    pub fn warnings(&self) -> Vec<&HealthIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Warning)
            .collect()
    }

    pub fn errors(&self) -> Vec<&HealthIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == IssueSeverity::Error)
            .collect()
    }

    pub fn is_healthy(&self) -> bool {
        self.issues
            .iter()
            .all(|i| i.severity == IssueSeverity::Warning)
    }
}

/// Maximum recommended file size (50 KB).
const MAX_FILE_SIZE: u64 = 50 * 1024;

/// Run health check on all knowledge files.
pub fn check_health(workspace: &Path) -> HealthReport {
    let knowledge_dir = workspace.join("knowledge");
    let mut report = HealthReport::default();

    if !knowledge_dir.exists() {
        return report;
    }

    // Collect all markdown files
    let mut files: Vec<(String, String)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&knowledge_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.extension().map(|e| e == "md").unwrap_or(false) {
                continue;
            }
            let filename = entry.file_name().to_string_lossy().to_string();
            let content = fs::read_to_string(&path).unwrap_or_default();
            files.push((filename, content));
            report.total_files += 1;
        }
    }

    // Check each file
    let mut seen_filenames: Vec<String> = Vec::new();
    for (filename, content) in &files {
        let mut is_healthy = true;

        // Check empty
        let body = extract_body(content);
        if body.trim().is_empty() {
            report.issues.push(HealthIssue {
                filename: filename.clone(),
                severity: IssueSeverity::Warning,
                kind: IssueKind::EmptyFile,
                message: "File has no body content".to_string(),
            });
            is_healthy = false;
        }

        // Check size
        if let Ok(meta) = fs::metadata(knowledge_dir.join(filename)) {
            if meta.len() > MAX_FILE_SIZE {
                report.issues.push(HealthIssue {
                    filename: filename.clone(),
                    severity: IssueSeverity::Warning,
                    kind: IssueKind::OversizedFile,
                    message: format!(
                        "File is {:.1} KB (max recommended: 50 KB)",
                        meta.len() as f64 / 1024.0
                    ),
                });
                is_healthy = false;
            }
        }

        // Check frontmatter
        if !content.starts_with("---") {
            report.issues.push(HealthIssue {
                filename: filename.clone(),
                severity: IssueSeverity::Warning,
                kind: IssueKind::MissingFrontmatter,
                message: "File has no YAML frontmatter".to_string(),
            });
            is_healthy = false;
        } else {
            // Validate frontmatter structure
            if let Some(fm) = extract_frontmatter(content) {
                if !fm.lines().any(|l| l.trim().starts_with("name:")) {
                    report.issues.push(HealthIssue {
                        filename: filename.clone(),
                        severity: IssueSeverity::Warning,
                        kind: IssueKind::MissingName,
                        message: "Frontmatter missing 'name' field".to_string(),
                    });
                    is_healthy = false;
                }
            } else {
                report.issues.push(HealthIssue {
                    filename: filename.clone(),
                    severity: IssueSeverity::Error,
                    kind: IssueKind::MalformedFrontmatter,
                    message: "Frontmatter has no closing ---".to_string(),
                });
                is_healthy = false;
            }
        }

        // Check expired status
        if content.contains("status: expired") || content.contains("status: \"expired\"") {
            report.issues.push(HealthIssue {
                filename: filename.clone(),
                severity: IssueSeverity::Warning,
                kind: IssueKind::ExpiredStatus,
                message: "File has expired status — should be cleaned up".to_string(),
            });
            is_healthy = false;
        }

        // Check confidence field
        if let Some(fm) = extract_frontmatter(content) {
            let mut has_confidence = false;
            let mut has_source = false;
            let mut has_description = false;

            for line in fm.lines() {
                if let Some(value) = line.trim().strip_prefix("confidence:") {
                    has_confidence = true;
                    let conf = value.trim().trim_matches('"').trim_matches('\'');
                    match conf {
                        "EXTRACTED" | "INFERRED" | "AMBIGUOUS" => {}
                        _ => {
                            report.issues.push(HealthIssue {
                                filename: filename.clone(),
                                severity: IssueSeverity::Error,
                                kind: IssueKind::InvalidConfidence,
                                message: format!("Invalid confidence value: '{}' (must be EXTRACTED/INFERRED/AMBIGUOUS)", conf),
                            });
                            is_healthy = false;
                        }
                    }
                }
                if line.trim().starts_with("source:") {
                    has_source = true;
                }
                if line.trim().starts_with("description:") {
                    let val = line.trim().trim_start_matches("description:").trim();
                    if !val.is_empty() && val != "''" && val != "\"\"" {
                        has_description = true;
                    }
                }
            }

            if has_confidence && content.contains("confidence: AMBIGUOUS") {
                report.issues.push(HealthIssue {
                    filename: filename.clone(),
                    severity: IssueSeverity::Warning,
                    kind: IssueKind::AmbiguousConfidence,
                    message: "Knowledge has AMBIGUOUS confidence — needs human review".to_string(),
                });
            }

            // Schema recommendation: source field
            if !has_source {
                report.issues.push(HealthIssue {
                    filename: filename.clone(),
                    severity: IssueSeverity::Warning,
                    kind: IssueKind::MissingSourceField,
                    message: "Frontmatter missing 'source' field (recommended: evolution/conversation/import/manual)".to_string(),
                });
            }

            // Schema recommendation: description field
            if !has_description {
                report.issues.push(HealthIssue {
                    filename: filename.clone(),
                    severity: IssueSeverity::Warning,
                    kind: IssueKind::MissingDescription,
                    message:
                        "Frontmatter missing 'description' field — compile can auto-generate this"
                            .to_string(),
                });
            }
        }

        // Check dual-layer separator (second `---` after body)
        if content.starts_with("---") {
            if let Some(rest) = content.strip_prefix("---") {
                if let Some(fm_end) = rest.find("---") {
                    let after_frontmatter = &rest[fm_end + 3..];
                    // Should have a second `---` separator after the body section
                    let body_and_timeline = after_frontmatter.trim();
                    if !body_and_timeline.is_empty()
                        && !body_and_timeline.contains("\n---\n")
                        && !body_and_timeline.contains("\n---")
                    {
                        report.issues.push(HealthIssue {
                            filename: filename.clone(),
                            severity: IssueSeverity::Warning,
                            kind: IssueKind::MissingDualLayerSeparator,
                            message: "Missing dual-layer separator (`---`) between compiled truth and timeline".to_string(),
                        });
                    }
                }
            }
        }

        // Check duplicate filename (case-insensitive)
        let lower = filename.to_lowercase();
        if seen_filenames.iter().any(|f| f.to_lowercase() == lower) {
            report.issues.push(HealthIssue {
                filename: filename.clone(),
                severity: IssueSeverity::Error,
                kind: IssueKind::DuplicateContent,
                message: "Duplicate filename (case-insensitive)".to_string(),
            });
            is_healthy = false;
        }
        seen_filenames.push(filename.clone());

        if is_healthy {
            report.healthy_files += 1;
        }
    }

    // Check MEMORY.md references
    let memory_path = workspace.join("MEMORY.md");
    if let Ok(memory_content) = fs::read_to_string(&memory_path) {
        for line in memory_content.lines() {
            if let Some(ref_start) = line.find("](knowledge/") {
                // Extract reference like: [Title](knowledge/file.md)
                if let Some(ref_end) = line[ref_start..].find(')') {
                    let reference = &line[ref_start + 2..ref_start + ref_end];
                    let ref_filename = reference.trim_start_matches("knowledge/");
                    let full_path = knowledge_dir.join(ref_filename);
                    if !full_path.exists() {
                        report.issues.push(HealthIssue {
                            filename: ref_filename.to_string(),
                            severity: IssueSeverity::Error,
                            kind: IssueKind::BrokenReference,
                            message: format!(
                                "MEMORY.md references '{}' but file does not exist",
                                ref_filename
                            ),
                        });
                    }
                }
            }
        }
    }

    report
}

/// Auto-fix issues that can be resolved without user input.
///
/// Returns the number of issues fixed.
pub fn auto_fix(workspace: &Path, report: &HealthReport) -> usize {
    let knowledge_dir = workspace.join("knowledge");
    let mut fixed = 0;

    for issue in &report.issues {
        match issue.kind {
            IssueKind::EmptyFile => {
                // Remove empty files
                let path = knowledge_dir.join(&issue.filename);
                if path.exists() {
                    let _ = fs::remove_file(&path);
                    fixed += 1;
                }
            }
            IssueKind::ExpiredStatus => {
                // Delete expired files
                let path = knowledge_dir.join(&issue.filename);
                if path.exists() {
                    if let Ok(content) = fs::read_to_string(&path) {
                        let _ = crate::version::record_version(
                            workspace,
                            "health-delete",
                            &issue.filename,
                            Some(&content),
                            None,
                            "health",
                        );
                    }
                    let _ = fs::remove_file(&path);
                    fixed += 1;
                }
            }
            _ => {} // Other issues need manual fix or LLM
        }
    }

    // Rebuild MEMORY.md after deletions
    if fixed > 0 {
        let _ = crate::evolution::update_memory_index(workspace);
    }

    fixed
}

fn extract_body(content: &str) -> &str {
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            return rest[end + 3..].trim();
        }
    }
    content
}

fn extract_frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("---")?;
    Some(&rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_workspace(tmp: &TempDir) -> std::path::PathBuf {
        let ws = tmp.path().to_path_buf();
        fs::create_dir_all(ws.join("knowledge")).unwrap();
        fs::create_dir_all(ws.join("history")).unwrap();
        ws
    }

    #[test]
    fn test_check_health_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);
        let report = check_health(&ws);
        assert_eq!(report.total_files, 0);
        assert!(report.is_healthy());
    }

    #[test]
    fn test_check_health_healthy_file() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/good.md"),
            "---\nname: Test\nsource: manual\ndescription: A test file\ntags: [\"test\"]\n---\n\nSome content here\n\n---\n\n- 2025-01-01: created",
        ).unwrap();

        let report = check_health(&ws);
        assert_eq!(report.total_files, 1);
        assert_eq!(report.healthy_files, 1);
        assert!(report.issues.is_empty());
    }

    #[test]
    fn test_check_health_empty_file() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(ws.join("knowledge/empty.md"), "---\nname: empty\n---\n\n").unwrap();

        let report = check_health(&ws);
        assert!(report.issues.iter().any(|i| i.kind == IssueKind::EmptyFile));
    }

    #[test]
    fn test_check_health_missing_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/no-fm.md"),
            "Just content without frontmatter",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::MissingFrontmatter));
    }

    #[test]
    fn test_check_health_malformed_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/bad.md"),
            "---\nname: test\n\nNo closing marker",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::MalformedFrontmatter));
    }

    #[test]
    fn test_check_health_expired() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/expired.md"),
            "---\nname: old\nstatus: expired\n---\n\nOld content",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::ExpiredStatus));
    }

    #[test]
    fn test_check_health_missing_name() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/noname.md"),
            "---\nsource: evolution\n---\n\nContent",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::MissingName));
    }

    #[test]
    fn test_check_health_broken_reference() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        // Good file
        fs::write(
            ws.join("knowledge/good.md"),
            "---\nname: Good\n---\n\nContent",
        )
        .unwrap();

        // MEMORY.md references non-existent file
        fs::write(
            ws.join("MEMORY.md"),
            "# 知识索引\n\n## 知识\n\n- [Good](knowledge/good.md)\n- [Missing](knowledge/missing.md)\n",
        ).unwrap();

        let report = check_health(&ws);
        assert!(report
            .issues
            .iter()
            .any(|i| i.kind == IssueKind::BrokenReference));
    }

    #[test]
    fn test_auto_fix_removes_empty() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(ws.join("knowledge/empty.md"), "---\nname: empty\n---\n\n").unwrap();

        let report = check_health(&ws);
        let fixed = auto_fix(&ws, &report);
        assert_eq!(fixed, 1);
        assert!(!ws.join("knowledge/empty.md").exists());
    }

    #[test]
    fn test_auto_fix_removes_expired() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/old.md"),
            "---\nname: old\nstatus: expired\n---\n\nOld content",
        )
        .unwrap();

        let report = check_health(&ws);
        let fixed = auto_fix(&ws, &report);
        assert_eq!(fixed, 1);
        assert!(!ws.join("knowledge/old.md").exists());
    }

    #[test]
    fn test_check_health_missing_dual_layer_separator() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        // File without the second --- separator
        fs::write(
            ws.join("knowledge/no-sep.md"),
            "---\nname: test\n---\n\nSome content without dual-layer separator",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::MissingDualLayerSeparator),
            "Should detect missing dual-layer separator"
        );
    }

    #[test]
    fn test_check_health_has_dual_layer_separator_ok() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        // File with the second --- separator
        fs::write(
            ws.join("knowledge/with-sep.md"),
            "---\nname: test\nsource: manual\n---\n\nSome content\n\n---\n\n- 2025-01-01: created",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::MissingDualLayerSeparator),
            "Should NOT flag file with dual-layer separator"
        );
    }

    #[test]
    fn test_check_health_missing_source() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/no-source.md"),
            "---\nname: test\n---\n\nContent\n\n---\n",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::MissingSourceField),
            "Should detect missing source field"
        );
    }

    #[test]
    fn test_check_health_missing_description() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/no-desc.md"),
            "---\nname: test\nsource: manual\n---\n\nContent\n\n---\n",
        )
        .unwrap();

        let report = check_health(&ws);
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::MissingDescription),
            "Should detect missing description field"
        );
    }

    #[test]
    fn test_check_health_schema_complete() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/complete.md"),
            "---\nname: test\nsource: manual\ndescription: A test\ntags: [\"test\"]\n---\n\nContent\n\n---\n\n- 2025-01-01: created",
        ).unwrap();

        let report = check_health(&ws);
        assert_eq!(
            report.healthy_files, 1,
            "File with complete schema should be healthy"
        );
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::MissingSourceField),
            "Should NOT flag file with source"
        );
        assert!(
            !report
                .issues
                .iter()
                .any(|i| i.kind == IssueKind::MissingDescription),
            "Should NOT flag file with description"
        );
    }

    #[test]
    fn test_report_warnings_and_errors() {
        let mut report = HealthReport::default();
        report.issues.push(HealthIssue {
            filename: "a.md".to_string(),
            severity: IssueSeverity::Warning,
            kind: IssueKind::EmptyFile,
            message: "empty".to_string(),
        });
        report.issues.push(HealthIssue {
            filename: "b.md".to_string(),
            severity: IssueSeverity::Error,
            kind: IssueKind::MalformedFrontmatter,
            message: "bad fm".to_string(),
        });

        assert_eq!(report.warnings().len(), 1);
        assert_eq!(report.errors().len(), 1);
        assert!(!report.is_healthy());
    }
}
