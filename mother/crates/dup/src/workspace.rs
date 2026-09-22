/// Workspace detection — walk up from cwd looking for `.dup/`.
use std::path::{Path, PathBuf};

/// Find the workspace root by walking up from cwd looking for `.dup/`,
/// or a valid clone workspace (has template.json, profile.md, or SOUL.md).
pub fn find_workspace() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_workspace_from(&cwd)
}

/// Find workspace root from a given starting path.
/// Priority:
/// 1. Current dir if it has `.dup/` → it's a dup workspace
/// 2. Current dir if it has clone markers (template.json, profile.md, SOUL.md) → dup workspace
/// 3. Walk up looking for `.dup/` → dup workspace parent
/// 4. Walk up looking for clone markers
pub fn find_workspace_from(start: &Path) -> Option<PathBuf> {
    let mut current = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };

    loop {
        if is_dup_workspace(&current) {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Check if a directory can be considered a dup workspace.
fn is_dup_workspace(dir: &Path) -> bool {
    // Has .dup/ directory → definitely a dup workspace
    if dir.join(".dup").is_dir() {
        return true;
    }
    // Has clone marker files → can be initialized as dup workspace
    if dir.join("template.json").is_file() {
        return true;
    }
    if dir.join("profile.md").is_file() {
        return true;
    }
    if dir.join("SOUL.md").is_file() {
        return true;
    }
    false
}

/// Require a workspace (return error if not found).
pub fn require_workspace() -> anyhow::Result<PathBuf> {
    find_workspace().ok_or_else(|| {
        anyhow::anyhow!(
            "不在 dup workspace 中。请进入分身目录或使用 'dup clone <name>' 创建一个。"
        )
    })
}

/// Get the .dup directory path for a workspace.
pub fn dup_dir(workspace: &Path) -> PathBuf {
    workspace.join(".dup")
}

/// Get the state.json path.
pub fn state_path(workspace: &Path) -> PathBuf {
    dup_dir(workspace).join("state.json")
}

/// Get the content-addressed object store dir (`.dup/objects/`).
pub fn objects_dir(workspace: &Path) -> PathBuf {
    dup_dir(workspace).join("objects")
}
