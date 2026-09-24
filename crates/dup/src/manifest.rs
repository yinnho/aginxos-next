//! File-level manifest of a clone workspace's definition layer.
//!
//! Git-style file-level sync (no packed archive): a map of relative path ->
//! SHA-256 for every definition-layer file, plus a `hash` state id used for
//! fast-forward comparison. Mirrors `opencarrier/crates/clone/src/manifest.rs`
//! (kept duplicated across the two repos, same as `extract_agx`).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Runtime dirs/files excluded from the manifest (mirror extractor::SKIP_PACK
/// plus `.dup/` VCS state dir).
const SKIP: &[&str] = &[
    "agent.toml",
    "AGENT.json",
    "admins.json",
    "output",
    "sessions",
    "history",
    "logs",
    "users",
    "data",
    "senders",
    ".lifecycle",
    ".dup",
];

/// True if a top-level entry is a test-workspace dir: `test`, `test2`, ... or
/// `test-foo`. (Catches `test`/`testN` that the old `test-` prefix missed.)
pub fn is_test_dir(top: &str) -> bool {
    top == "test"
        || top.starts_with("test-")
        || (top.starts_with("test") && top[4..].chars().all(|c| c.is_ascii_digit()))
}

/// True if a file name is a backup: `foo.bak` or `foo.bak.<timestamp>`.
pub fn is_bak(top: &str) -> bool {
    top.ends_with(".bak") || top.contains(".bak.")
}

/// A file-level snapshot of a workspace's definition layer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    /// relative path -> hex SHA-256 of file content
    pub files: BTreeMap<String, String>,
    /// SHA-256 of the sorted `path:hash` serialization - state id
    pub hash: String,
}

impl Manifest {
    /// An empty manifest.
    pub fn empty() -> Self {
        Manifest {
            files: BTreeMap::new(),
            hash: manifest_hash(&BTreeMap::new()),
        }
    }
}

/// Build a manifest by walking `workspace` and hashing every definition-layer
/// file. Selection mirrors `pack_workspace_as_agx` (same skip rules).
pub fn build_manifest(workspace: &Path) -> Result<Manifest> {
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    walk(workspace, workspace, &mut files)?;
    let hash = manifest_hash(&files);
    Ok(Manifest { files, hash })
}

fn walk(base: &Path, cur: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    let entries = std::fs::read_dir(cur).with_context(|| format!("read_dir {}", cur.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name_str = entry.file_name().to_string_lossy().into_owned();

        if name_str.starts_with("._") || name_str == ".DS_Store" {
            continue;
        }
        // Skip dup VCS artifacts: conflict sidecars + transient write tmps.
        if name_str.ends_with(".dup-theirs") || name_str.ends_with(".duptmp") {
            continue;
        }

        if path.is_dir() {
            walk(base, &path, files)?;
        } else {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let top = rel_str.split('/').next().unwrap_or(&rel_str);
            if SKIP.contains(&top) || is_test_dir(top) || is_bak(top) {
                continue;
            }
            let data = std::fs::read(&path)
                .with_context(|| format!("read {}", path.display()))?;
            let mut h = Sha256::new();
            h.update(&data);
            files.insert(rel_str, format!("{:x}", h.finalize()));
        }
    }
    Ok(())
}

pub fn manifest_hash(files: &BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    for (p, sha) in files {
        h.update(p.as_bytes());
        h.update(b":");
        h.update(sha.as_bytes());
        h.update(b"\n");
    }
    format!("{:x}", h.finalize())
}
