//! Workspace filesystem sandboxing.
//!
//! Confines agent file operations to their workspace directory.
//! Prevents path traversal, symlink escapes, and access outside the sandbox.

use std::path::{Path, PathBuf};
use carrier_types::error::{CarrierError, CarrierResult};

/// Check if a relative path is an internal workspace path that should NOT be
/// auto-routed to the sender's output directory.
pub fn is_internal_path(rel: &str) -> bool {
    matches!(
        rel,
        "agent.toml" | "SOUL.md" | "system_prompt.md" | "profile.md" | "style.md" | "evolution.md"
    ) || rel.starts_with("knowledge/")
        || rel.starts_with("flows/")
        || rel.starts_with("sessions/")
        || rel.starts_with("senders/")
        || rel.starts_with("workspaces/")
        || rel.starts_with("data/")
        // staging/ holds in-progress clone definition layers (clone-creator
        // writes each file incrementally; a [CLONE_INSTALL:<name>] marker in
        // the reply lets the kernel move staging/<name>/ into a real workspace
        // at turn end). Must land INSIDE the workspace, not the sender-scoped
        // output dir, so it survives across channels/sessions and the kernel
        // marker handler can resolve it from the manifest workspace alone.
        || rel.starts_with("staging/")
}

/// Resolve a user-supplied path within a workspace sandbox.
///
/// - Rejects `..` components outright.
/// - Relative paths are joined with `workspace_root`.
/// - Absolute paths are checked against the workspace root after canonicalization.
/// - For new files: canonicalizes the parent directory and appends the filename.
/// - The final canonical path must start with the canonical workspace root.
pub fn resolve_sandbox_path(user_path: &str, workspace_root: &Path) -> CarrierResult<PathBuf> {
    let path = Path::new(user_path);

    // Reject any `..` components
    for component in path.components() {
        if matches!(component, std::path::Component::ParentDir) {
            return Err(CarrierError::InvalidInput(
                "Path traversal denied: '..' components are forbidden".to_string(),
            ));
        }
    }

    // Build the candidate path
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    };

    // Canonicalize the workspace root
    let canon_root = workspace_root
        .canonicalize()
        .map_err(|e| CarrierError::Internal(format!("Failed to resolve workspace root: {e}")))?;

    // Canonicalize the candidate (or its parent for new files)
    let canon_candidate = if candidate.exists() {
        candidate
            .canonicalize()
            .map_err(|e| CarrierError::Internal(format!("Failed to resolve path: {e}")))?
    } else {
        // For new files: find the nearest existing ancestor, canonicalize it,
        // then re-append the remaining path components and create intermediate dirs.
        // Collect path components from leaf to ancestor (e.g. ["file.md", "subdir", "knowledge"])
        let mut ancestor = candidate.clone();
        let mut components: Vec<std::ffi::OsString> = Vec::new();

        loop {
            let name = ancestor
                .file_name()
                .ok_or_else(|| CarrierError::InvalidInput("Invalid path: no filename".to_string()))?
                .to_os_string();
            components.push(name);

            let parent = ancestor.parent().ok_or_else(|| {
                CarrierError::InvalidInput("Invalid path: no parent directory".to_string())
            })?;

            if parent.exists() {
                let canon_parent = parent.canonicalize().map_err(|e| {
                    CarrierError::Internal(format!("Failed to resolve parent directory: {e}"))
                })?;
                // Verify the existing ancestor is inside the sandbox
                if !canon_parent.starts_with(&canon_root) {
                    return Err(CarrierError::InvalidInput(format!(
                        "Access denied: path '{}' resolves outside workspace",
                        user_path
                    )));
                }

                // components was collected leaf-to-ancestor, rev gives ancestor-to-leaf
                // e.g. ["knowledge", "subdir", "file.md"]
                // Create directories for all but the last component (the filename)
                let rev: Vec<_> = components.into_iter().rev().collect();
                let mut current = canon_parent.clone();
                for part in rev.iter().take(rev.len() - 1) {
                    current = current.join(part);
                    if !current.exists() {
                        std::fs::create_dir(&current).map_err(|e| {
                            CarrierError::Internal(format!(
                                "Failed to create directory '{}': {e}",
                                current.display()
                            ))
                        })?;
                    }
                }
                // Append the filename (last component)
                break current.join(rev.last().unwrap());
            }
            ancestor = parent.to_path_buf();
        }
    };

    // Verify the canonical path is inside the workspace
    if !canon_candidate.starts_with(&canon_root) {
        return Err(CarrierError::InvalidInput(format!(
            "Access denied: path '{}' resolves outside workspace. \
             file_read/file_write/file_list only work within the workspace directory.",
            user_path
        )));
    }

    Ok(canon_candidate)
}

/// Resolve a user-supplied path for write operations within a workspace sandbox.
///
/// Enforces:
/// - **Blocked**: `agent.toml`, `SOUL.md` (only trainer tools may modify these)
/// - **Blocked**: identity-frozen files when EVOLUTION.md declares identity freeze
///
/// Note: output/, memory/, and catch-all (non-internal) paths are handled by the
/// filesystem tools directly via `resolve_user_data_path()`, which writes to the
/// top-level `~/.aginx/carrier/senders/` directory. This sandbox function only
/// handles workspace-internal paths (knowledge/, flows/, etc.).
pub fn resolve_sandbox_path_for_write(
    user_path: &str,
    workspace_root: &Path,
    _sender_id: Option<&str>,
    _agent_name: Option<&str>,
    is_clone_admin: bool,
) -> CarrierResult<PathBuf> {
    let normalized = user_path.replace('\\', "/");
    let path = Path::new(&normalized);

    // Extract the relative path components for permission checking
    let relative = if path.is_absolute() {
        path.strip_prefix(workspace_root)
            .map_err(|_| CarrierError::InvalidInput("Absolute path outside workspace".to_string()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    };

    let rel_str = relative.to_string_lossy();

    // Block writes to protected config files (unless clone admin).
    // These files define the clone's identity — only trainers should modify them.
    if (rel_str == "agent.toml" || rel_str == "SOUL.md" || rel_str == "system_prompt.md")
        && !is_clone_admin
    {
        return Err(CarrierError::InvalidInput(format!(
            "Write denied: '{}' is a protected config file (only trainer may modify)",
            rel_str
        )));
    }

    // Block writes to identity-frozen files when EVOLUTION.md declares identity freeze
    const IDENTITY_FILES: &[&str] = &[
        "MENTAL-MODELS.md",
        "DECISION-HEURISTICS.md",
        "EXPRESSION-DNA.md",
        "TIMELINE.md",
        "system_prompt.md",
    ];
    if IDENTITY_FILES.contains(&rel_str.as_ref()) {
        let evolution_path = workspace_root.join("EVOLUTION.md");
        if let Ok(content) = std::fs::read_to_string(&evolution_path) {
            if content.contains("身份层不可修改") || content.contains("identity_frozen:") {
                return Err(CarrierError::InvalidInput(format!(
                    "Write denied: '{}' is a frozen identity file (evolution system may not modify)",
                    rel_str
                )));
            }
        }
    }

    // Delegate to the existing sandbox for path resolution and traversal checks
    resolve_sandbox_path(&rel_str, workspace_root)
}

/// Resolve a user-supplied path for read operations within a workspace sandbox.
///
/// Note: output/, memory/, and catch-all paths are handled by the filesystem tools
/// directly via `resolve_user_data_path()`. This sandbox function only handles
/// workspace-internal paths.
pub fn resolve_sandbox_path_for_read(
    user_path: &str,
    workspace_root: &Path,
    _sender_id: Option<&str>,
    _agent_name: Option<&str>,
) -> CarrierResult<PathBuf> {
    let normalized = user_path.replace('\\', "/");
    let path = Path::new(&normalized);

    let relative = if path.is_absolute() {
        path.strip_prefix(workspace_root)
            .map_err(|_| CarrierError::InvalidInput("Absolute path outside workspace".to_string()))?
            .to_path_buf()
    } else {
        path.to_path_buf()
    };

    let rel_str = relative.to_string_lossy();
    resolve_sandbox_path(&rel_str, workspace_root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_relative_path_inside_workspace() {
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("test.txt"), "hello").unwrap();

        let result = resolve_sandbox_path("data/test.txt", dir.path());
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert!(resolved.starts_with(dir.path().canonicalize().unwrap()));
    }

    #[test]
    fn test_absolute_path_inside_workspace() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("file.txt"), "ok").unwrap();
        let abs_path = dir.path().join("file.txt");

        let result = resolve_sandbox_path(abs_path.to_str().unwrap(), dir.path());
        assert!(result.is_ok());
    }

    #[test]
    fn test_absolute_path_outside_workspace_blocked() {
        let dir = TempDir::new().unwrap();
        let outside = std::env::temp_dir().join("outside_test.txt");
        std::fs::write(&outside, "nope").unwrap();

        let result = resolve_sandbox_path(outside.to_str().unwrap(), dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Access denied"));

        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn test_dotdot_component_blocked() {
        let dir = TempDir::new().unwrap();
        let result = resolve_sandbox_path("../../../etc/passwd", dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Path traversal denied"));
    }

    #[test]
    fn test_nonexistent_file_with_valid_parent() {
        let dir = TempDir::new().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();

        let result = resolve_sandbox_path("data/new_file.txt", dir.path());
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert!(resolved.starts_with(dir.path().canonicalize().unwrap()));
        assert!(resolved.ends_with("new_file.txt"));
    }

    #[test]
    fn test_nonexistent_file_with_missing_parent_dirs() {
        let dir = TempDir::new().unwrap();

        // knowledge/ does NOT exist yet — this is the failing case
        let result = resolve_sandbox_path("knowledge/city-beijing.md", dir.path());
        assert!(result.is_ok(), "Expected OK, got: {:?}", result);
        let resolved = result.unwrap();
        assert!(resolved.starts_with(dir.path().canonicalize().unwrap()));
        assert!(resolved.ends_with("city-beijing.md"));
        // The intermediate directory should have been created
        assert!(resolved.parent().unwrap().exists());
    }

    #[test]
    fn test_nonexistent_file_with_deeply_missing_parents() {
        let dir = TempDir::new().unwrap();

        // Neither flows/ nor sub/ exists
        let result = resolve_sandbox_path("flows/sub/deep/file.md", dir.path());
        assert!(result.is_ok(), "Expected OK, got: {:?}", result);
        let resolved = result.unwrap();
        assert!(resolved.starts_with(dir.path().canonicalize().unwrap()));
        assert!(resolved.ends_with("file.md"));
        assert!(resolved.parent().unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_symlink_escape_blocked() {
        let dir = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();

        // Create a symlink inside the workspace pointing outside
        let link_path = dir.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link_path).unwrap();

        let result = resolve_sandbox_path("escape/secret.txt", dir.path());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Access denied"));
    }
}
