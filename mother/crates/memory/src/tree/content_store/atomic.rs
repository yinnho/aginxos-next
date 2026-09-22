//! Atomic file writes: tempfile + fsync + rename.

use std::fs;
use std::io::Write;
use std::path::Path;

use carrier_types::error::{CarrierError, CarrierResult};

/// Write content to a file atomically using tempfile + fsync + rename.
///
/// If the target file already exists, returns `Ok(false)` (immutable-body contract).
/// Otherwise writes to a temp file, fsyncs, then renames.
pub fn write_if_new(path: &Path, content: &str) -> CarrierResult<bool> {
    // Fast path: if target exists, skip (immutable body)
    if path.exists() {
        return Ok(false);
    }

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| CarrierError::Internal(format!("mkdir {}: {e}", parent.display())))?;
    }

    // Write to temp file in same directory (so rename is atomic on same FS)
    let tmp_name = format!(".tmp_{}.md", uuid::Uuid::new_v4().simple());
    let tmp_path = path.with_file_name(&tmp_name);

    let mut f = fs::File::create(&tmp_path)
        .map_err(|e| CarrierError::Internal(format!("create {}: {e}", tmp_path.display())))?;

    f.write_all(content.as_bytes())
        .map_err(|e| CarrierError::Internal(format!("write {}: {e}", tmp_path.display())))?;

    f.sync_all()
        .map_err(|e| CarrierError::Internal(format!("fsync {}: {e}", tmp_path.display())))?;

    drop(f);

    // Atomic rename
    match fs::rename(&tmp_path, path) {
        Ok(()) => {}
        Err(e) => {
            // If target appeared concurrently, just clean up
            let _ = fs::remove_file(&tmp_path);
            if path.exists() {
                return Ok(false);
            }
            return Err(CarrierError::Internal(format!(
                "rename {} → {}: {e}",
                tmp_path.display(),
                path.display()
            )));
        }
    }

    // Best-effort parent dir fsync for durability
    if let Some(parent) = path.parent() {
        if let Ok(dir_f) = fs::File::open(parent) {
            let _ = dir_f.sync_all();
        }
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_write_if_new_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.md");

        let written = write_if_new(&path, "hello").unwrap();
        assert!(written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
    }

    #[test]
    fn test_write_if_new_skips_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.md");

        write_if_new(&path, "original").unwrap();
        let written = write_if_new(&path, "updated").unwrap();
        assert!(!written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
    }

    #[test]
    fn test_write_if_new_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a/b/c/test.md");

        let written = write_if_new(&path, "deep").unwrap();
        assert!(written);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "deep");
    }
}
