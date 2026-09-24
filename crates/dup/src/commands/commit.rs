//! `dup commit` - snapshot the working tree as a local commit.
//!
//! File-level: builds the current manifest and compares to the last commit's
//! manifest. Skips if nothing changed, otherwise stores every file's content
//! in `.dup/objects/` (content-addressed, deduped) and appends a commit.
//! The object store makes commits REAL history: any committed state can be
//! brought back with `dup restore`, independent of the stateless remote.
//! Local versioning only - does not touch the remote.

use anyhow::Result;

use crate::diff::diff_manifests;
use crate::manifest::build_manifest;
use crate::objects;
use crate::state::DupState;
use crate::workspace;

pub fn run(message: &str) -> Result<()> {
    let ws = workspace::require_workspace()?;
    if !workspace::state_path(&ws).exists() {
        anyhow::bail!("未初始化。请先运行 'dup init'。");
    }
    let mut state = DupState::load(&ws)?;

    let current = build_manifest(&ws)?;

    if let Some(last) = state.last_commit() {
        let changes = diff_manifests(&last.manifest, &current);
        if changes.is_empty() {
            // No new commit, but backfill objects for the current state: a
            // workspace whose last commit predates the object store becomes
            // restorable from this point on. Idempotent (existing objects
            // are skipped).
            let backfilled = objects::store_objects(&ws, &current)?;
            if backfilled > 0 {
                println!("没有变更，无需提交（已补齐对象库: {} 个对象）", backfilled);
            } else {
                println!("没有变更，无需提交");
            }
            return Ok(());
        }
    }

    // Store content blobs BEFORE recording the commit: a commit without its
    // objects would be unrestorable. Failing here aborts the commit.
    let new_objects = objects::store_objects(&ws, &current)?;

    state.add_commit(current.clone(), message);
    state.save(&ws)?;

    println!("[{}] {}", &current.hash[..current.hash.len().min(12)], message);
    println!(
        "  {} 个文件, {} 个新对象",
        current.files.len(),
        new_objects
    );
    Ok(())
}
