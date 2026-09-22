//! Three-way file-level merge (base, ours, theirs) by manifest hashes.
//!
//! Used by `dup pull` to reconcile remote changes with local changes since the
//! merge base (= last pulled manifest). Decisions are per-file by SHA-256; the
//! caller fetches theirs content / applies writes / writes conflict sidecars.

use std::collections::BTreeSet;

use crate::manifest::Manifest;

/// What to do with a path during a pull merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeAction {
    /// No change needed (ours already matches, or we won and theirs is same/unchanged).
    Keep,
    /// Take the remote's version (fetch theirs content + write).
    TakeTheirs,
    /// Remove the file locally (remote deleted, we didn't change it).
    Delete,
    /// Both sides changed differently - keep ours, write theirs to a sidecar.
    Conflict,
}

/// A per-path merge decision for a path that needs an action.
#[derive(Debug, Clone)]
pub struct MergedFile {
    pub path: String,
    pub action: MergeAction,
}

/// Compute merge actions for every path that needs one (pure Keeps are omitted).
pub fn three_way(base: &Manifest, ours: &Manifest, theirs: &Manifest) -> Vec<MergedFile> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(base.files.keys().cloned());
    paths.extend(ours.files.keys().cloned());
    paths.extend(theirs.files.keys().cloned());

    let mut out = Vec::new();
    for path in paths {
        let b = base.files.get(&path);
        let o = ours.files.get(&path);
        let t = theirs.files.get(&path);
        let action = merge_one(b, o, t);
        if action != MergeAction::Keep {
            out.push(MergedFile { path, action });
        }
    }
    out
}

fn merge_one(base: Option<&String>, ours: Option<&String>, theirs: Option<&String>) -> MergeAction {
    match (base, ours, theirs) {
        // --- not in base ---
        (None, None, None) => MergeAction::Keep,
        (None, Some(_), None) => MergeAction::Keep, // ours added
        (None, None, Some(_)) => MergeAction::TakeTheirs, // theirs added
        (None, Some(o), Some(t)) => {
            if o == t {
                MergeAction::Keep // both added same
            } else {
                MergeAction::Conflict
            }
        }
        // --- in base ---
        (Some(_), None, None) => MergeAction::Keep, // both deleted (already gone)
        (Some(b), Some(o), None) => {
            // theirs deleted
            if o == b {
                MergeAction::Delete // we unchanged, they deleted
            } else {
                MergeAction::Conflict // we changed, they deleted
            }
        }
        (Some(b), None, Some(t)) => {
            // ours deleted
            if t == b {
                MergeAction::Keep // they unchanged, we deleted -> stay deleted
            } else {
                MergeAction::Conflict // they changed, we deleted
            }
        }
        (Some(b), Some(o), Some(t)) => {
            if o == t {
                MergeAction::Keep // same content
            } else if o == b {
                MergeAction::TakeTheirs // we unchanged, they changed
            } else if t == b {
                MergeAction::Keep // they unchanged, we changed -> keep ours
            } else {
                MergeAction::Conflict // both changed differently
            }
        }
    }
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

    fn action_of<'a>(merged: &'a [MergedFile], path: &str) -> Option<&'a MergeAction> {
        merged.iter().find(|m| m.path == path).map(|m| &m.action)
    }

    #[test]
    fn remote_changed_take_theirs() {
        let base = manifest(&[("SOUL.md", "b")]);
        let ours = manifest(&[("SOUL.md", "b")]);
        let theirs = manifest(&[("SOUL.md", "t")]);
        let m = three_way(&base, &ours, &theirs);
        assert_eq!(action_of(&m, "SOUL.md"), Some(&MergeAction::TakeTheirs));
    }

    #[test]
    fn local_changed_keep_ours() {
        let base = manifest(&[("SOUL.md", "b")]);
        let ours = manifest(&[("SOUL.md", "o")]);
        let theirs = manifest(&[("SOUL.md", "b")]);
        let m = three_way(&base, &ours, &theirs);
        assert_eq!(action_of(&m, "SOUL.md"), None); // Keep -> omitted
    }

    #[test]
    fn both_changed_conflict() {
        let base = manifest(&[("SOUL.md", "b")]);
        let ours = manifest(&[("SOUL.md", "o")]);
        let theirs = manifest(&[("SOUL.md", "t")]);
        let m = three_way(&base, &ours, &theirs);
        assert_eq!(action_of(&m, "SOUL.md"), Some(&MergeAction::Conflict));
    }

    #[test]
    fn remote_added_take_theirs() {
        let base = manifest(&[]);
        let ours = manifest(&[]);
        let theirs = manifest(&[("knowledge/new.md", "t")]);
        let m = three_way(&base, &ours, &theirs);
        assert_eq!(action_of(&m, "knowledge/new.md"), Some(&MergeAction::TakeTheirs));
    }

    #[test]
    fn remote_deleted_delete() {
        let base = manifest(&[("old.md", "b")]);
        let ours = manifest(&[("old.md", "b")]);
        let theirs = manifest(&[]);
        let m = three_way(&base, &ours, &theirs);
        assert_eq!(action_of(&m, "old.md"), Some(&MergeAction::Delete));
    }
}
