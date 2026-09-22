//! `.dup/objects/` - content-addressed blob store for local history.
//!
//! The opencarrier remote is stateless (current-state only, no history), so
//! rollback capability lives HERE, client-side. At commit time every file in
//! the manifest has its content stored under `.dup/objects/<sha256>`.
//! Content addressing dedups automatically: a file unchanged across 50
//! commits is stored once. This makes local commits REAL history -
//! `dup restore` can bring back any committed state even if the remote
//! workspace itself was damaged.

use std::path::Path;

use anyhow::{Context, Result};

use crate::manifest::Manifest;
use crate::workspace;

/// Write blobs for every file in `manifest`. Objects already stored are
/// skipped (same hash = same content). Returns the number of new objects.
pub fn store_objects(workspace: &Path, manifest: &Manifest) -> Result<usize> {
    let dir = workspace::objects_dir(workspace);
    std::fs::create_dir_all(&dir).context("创建 .dup/objects 失败")?;
    let mut written = 0usize;
    for (rel, sha) in &manifest.files {
        let obj = dir.join(sha);
        if obj.exists() {
            continue;
        }
        let data = std::fs::read(workspace.join(rel))
            .with_context(|| format!("读取 {} 失败", rel))?;
        // Atomic: tmp + rename so a crash never leaves a truncated object.
        let tmp = dir.join(format!(".{sha}.tmp"));
        std::fs::write(&tmp, &data).with_context(|| format!("写入对象 {} 失败", sha))?;
        std::fs::rename(&tmp, &obj).with_context(|| format!("落盘对象 {} 失败", sha))?;
        written += 1;
    }
    Ok(written)
}

/// Read a blob by content hash.
pub fn read_object(workspace: &Path, sha: &str) -> Result<Vec<u8>> {
    let obj = workspace::objects_dir(workspace).join(sha);
    std::fs::read(&obj).with_context(|| format!("对象缺失: {}", &sha[..sha.len().min(12)]))
}

/// True if a blob exists locally.
pub fn has_object(workspace: &Path, sha: &str) -> bool {
    workspace::objects_dir(workspace).join(sha).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::build_manifest;
    use sha2::{Digest, Sha256};

    fn sha256_hex(data: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(data);
        format!("{:x}", h.finalize())
    }

    /// Unique temp workspace per test (parallel-safe).
    fn temp_ws() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dup-objects-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn store_then_read_roundtrip() {
        let ws = temp_ws();
        std::fs::write(ws.join("SOUL.md"), b"hello soul").unwrap();
        let m = build_manifest(&ws).unwrap();

        let written = store_objects(&ws, &m).unwrap();
        assert_eq!(written, 1);

        let sha = &m.files["SOUL.md"];
        assert!(has_object(&ws, sha));
        assert_eq!(read_object(&ws, sha).unwrap(), b"hello soul");

        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn unchanged_content_dedups() {
        let ws = temp_ws();
        std::fs::write(ws.join("a.md"), b"same").unwrap();
        let m = build_manifest(&ws).unwrap();

        assert_eq!(store_objects(&ws, &m).unwrap(), 1);
        // Second store of identical content: no new objects.
        assert_eq!(store_objects(&ws, &m).unwrap(), 0);

        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn edited_content_gets_new_object() {
        let ws = temp_ws();
        std::fs::write(ws.join("a.md"), b"v1").unwrap();
        let m1 = build_manifest(&ws).unwrap();
        store_objects(&ws, &m1).unwrap();

        std::fs::write(ws.join("a.md"), b"v2").unwrap();
        let m2 = build_manifest(&ws).unwrap();
        assert_eq!(store_objects(&ws, &m2).unwrap(), 1);

        // Both versions recoverable -> real rollback.
        assert_eq!(read_object(&ws, &m1.files["a.md"]).unwrap(), b"v1");
        assert_eq!(read_object(&ws, &m2.files["a.md"]).unwrap(), b"v2");

        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn missing_object_errors() {
        let ws = temp_ws();
        let sha = sha256_hex(b"never stored");
        assert!(!has_object(&ws, &sha));
        assert!(read_object(&ws, &sha).is_err());

        std::fs::remove_dir_all(&ws).ok();
    }
}
