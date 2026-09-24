//! Manifest-based file-level diff (no CloneData / .agx dependency).
//!
//! Compares two [`Manifest`] snapshots and reports per-path changes. Used by
//! `dup status` / `dup diff` to show working-tree changes vs the last commit
//! (or vs the remote base with `--remote`).

use std::collections::BTreeSet;

use crate::manifest::Manifest;

/// Kind of change detected between two manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
}

/// A single file-level change.
#[derive(Debug, Clone)]
pub struct FileChange {
    /// Relative path within the workspace (e.g. "SOUL.md", "knowledge/foo.md").
    pub path: String,
    pub kind: ChangeKind,
}

/// Compare two manifests, returning all file-level changes (sorted by path).
pub fn diff_manifests(old: &Manifest, new: &Manifest) -> Vec<FileChange> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(old.files.keys().cloned());
    paths.extend(new.files.keys().cloned());

    let mut changes = Vec::new();
    for path in paths {
        match (old.files.get(&path), new.files.get(&path)) {
            (None, Some(_)) => changes.push(FileChange {
                path,
                kind: ChangeKind::Added,
            }),
            (Some(_), None) => changes.push(FileChange {
                path,
                kind: ChangeKind::Removed,
            }),
            (Some(o), Some(n)) if o != n => changes.push(FileChange {
                path,
                kind: ChangeKind::Modified,
            }),
            _ => {}
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn manifest(pairs: &[(&str, &str)]) -> Manifest {
        let files: BTreeMap<String, String> =
            pairs.iter().map(|(p, h)| (p.to_string(), h.to_string())).collect();
        Manifest {
            hash: crate::manifest::manifest_hash(&files),
            files,
        }
    }

    #[test]
    fn added_removed_modified() {
        let old = manifest(&[("a.md", "1"), ("b.md", "2"), ("c.md", "3")]);
        let new = manifest(&[("a.md", "1"), ("b.md", "9"), ("d.md", "4")]);
        let changes = diff_manifests(&old, &new);
        let by_path: std::collections::HashMap<&str, &ChangeKind> =
            changes.iter().map(|c| (c.path.as_str(), &c.kind)).collect();
        assert_eq!(by_path.get("b.md"), Some(&&ChangeKind::Modified));
        assert_eq!(by_path.get("c.md"), Some(&&ChangeKind::Removed));
        assert_eq!(by_path.get("d.md"), Some(&&ChangeKind::Added));
        assert!(!by_path.contains_key("a.md"));
    }

    #[test]
    fn identical_manifest_no_changes() {
        let m = manifest(&[("a.md", "1")]);
        assert!(diff_manifests(&m, &m).is_empty());
    }
}
