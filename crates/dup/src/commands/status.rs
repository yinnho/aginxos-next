//! `dup status` - show working tree changes since last commit (or vs remote).

use anyhow::Result;

use crate::diff::{diff_manifests, ChangeKind};
use crate::manifest::{build_manifest, Manifest};
use crate::state::DupState;
use crate::workspace;

pub fn run(remote: bool) -> Result<()> {
    let ws = workspace::require_workspace()?;
    let state = load_state(&ws)?;

    let current = build_manifest(&ws)?;

    let (changes, label) = if remote {
        let base = state
            .remote_base
            .ok_or_else(|| anyhow::anyhow!("尚无远程基准。先 'dup pull'。"))?;
        (diff_manifests(&base, &current), "相对 opencarrier 远程")
    } else {
        let base = state
            .last_commit()
            .map(|c| c.manifest.clone())
            .unwrap_or_else(Manifest::empty);
        (diff_manifests(&base, &current), "相对本地最新提交")
    };

    if changes.is_empty() {
        println!("工作区干净（{}）", label);
        return Ok(());
    }

    let added = changes.iter().filter(|c| c.kind == ChangeKind::Added).count();
    let removed = changes.iter().filter(|c| c.kind == ChangeKind::Removed).count();
    let modified = changes.iter().filter(|c| c.kind == ChangeKind::Modified).count();

    for c in &changes {
        let marker = match c.kind {
            ChangeKind::Modified => "\x1b[33m  modified:\x1b[0m",
            ChangeKind::Added => "\x1b[32m  new file:\x1b[0m",
            ChangeKind::Removed => "\x1b[31m  deleted:\x1b[0m ",
        };
        println!("{} {}", marker, c.path);
    }

    println!(
        "\n{} 个变更（{}）: +{} ~{} -{}",
        changes.len(),
        label,
        added,
        modified,
        removed,
    );
    println!("用 'dup commit -m \"...\"' 提交，'dup push' 推送到远程");
    Ok(())
}

/// Load state, requiring prior `dup init`.
fn load_state(ws: &std::path::Path) -> Result<DupState> {
    if !workspace::state_path(ws).exists() {
        anyhow::bail!("未初始化。请先运行 'dup init'。");
    }
    DupState::load(ws)
}
