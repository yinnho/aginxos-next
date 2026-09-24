//! `dup diff` - file-level diff of working tree changes.
//!
//! By default compares the working tree to the last local commit. With
//! `--remote`, compares to the remote base (divergence from opencarrier).

use anyhow::Result;

use crate::diff::{diff_manifests, ChangeKind};
use crate::manifest::{build_manifest, Manifest};
use crate::state::DupState;
use crate::workspace;

pub fn run(remote: bool) -> Result<()> {
    let ws = workspace::require_workspace()?;
    if !workspace::state_path(&ws).exists() {
        anyhow::bail!("未初始化。请先运行 'dup init'。");
    }
    let state = DupState::load(&ws)?;
    let current = build_manifest(&ws)?;

    let (changes, label) = if remote {
        let base = state
            .remote_base
            .ok_or_else(|| anyhow::anyhow!("尚无远程基准。先 'dup pull'。"))?;
        (diff_manifests(&base, &current), "远程基准")
    } else {
        let base = state
            .last_commit()
            .map(|c| c.manifest.clone())
            .unwrap_or_else(Manifest::empty);
        (diff_manifests(&base, &current), "最新提交")
    };

    if changes.is_empty() {
        println!("没有差异（{}）", label);
        return Ok(());
    }

    for c in &changes {
        let m = match c.kind {
            ChangeKind::Modified => 'M',
            ChangeKind::Added => 'A',
            ChangeKind::Removed => 'D',
        };
        println!("{} {}", m, c.path);
    }
    Ok(())
}
