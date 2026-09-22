//! `dup pull` - fetch remote changes and 3-way merge into the working tree.
//!
//! base = `remote_base` (last pulled manifest), ours = local working manifest,
//! theirs = remote's current manifest. Non-destructive: files only change where
//! the remote moved and we didn't; conflicts keep ours and write theirs to a
//! `{path}.dup-theirs` sidecar. After applying, `remote_base` advances to theirs.

use std::path::Path;

use anyhow::{Context, Result};

use crate::config::DupConfig;
use crate::manifest::{build_manifest, Manifest};
use crate::merge::{three_way, MergeAction};
use crate::remote;
use crate::state::DupState;
use crate::workspace;

pub async fn run() -> Result<()> {
    let ws = workspace::require_workspace()?;
    if !workspace::state_path(&ws).exists() {
        anyhow::bail!("未初始化。请先运行 'dup init'。");
    }
    let mut state = DupState::load(&ws)?;

    let config = DupConfig::load_global()?;
    let url = config.resolve_url();
    let api = config.resolve_api();
    let api_key = config.resolve_api_key()?;
    let name = state.remote_name.clone();

    let base = state.remote_base.clone().unwrap_or_else(Manifest::empty);
    let ours = build_manifest(&ws).context("构建本地 manifest 失败")?;

    println!("正在从 {} 拉取 {} ...", url, name);
    let theirs = remote::get_manifest(&url, &api_key, &api, &name).await?;

    if theirs.hash == base.hash {
        println!("远程无新变更");
        return Ok(());
    }

    let merged = three_way(&base, &ours, &theirs);

    let mut applied = 0usize;
    let mut deleted = 0usize;
    let mut conflicts = Vec::new();

    for m in &merged {
        match m.action {
            MergeAction::TakeTheirs => {
                let content = remote::get_file(&url, &api_key, &api, &name, &m.path).await?;
                atomic_write(&ws, &m.path, &content)?;
                applied += 1;
            }
            MergeAction::Delete => {
                let p = ws.join(&m.path);
                if p.exists() {
                    std::fs::remove_file(&p).ok();
                }
                deleted += 1;
            }
            MergeAction::Conflict => {
                let content = remote::get_file(&url, &api_key, &api, &name, &m.path).await?;
                let sidecar = format!("{}.dup-theirs", m.path);
                atomic_write(&ws, &sidecar, &content)?;
                conflicts.push(m.path.clone());
            }
            MergeAction::Keep => {}
        }
    }

    // The new merge base is theirs (everything we took/kept is now consistent
    // with theirs; local-only mods remain as uncommitted working changes).
    state.remote_base = Some(theirs.clone());
    state.save(&ws)?;

    println!(
        "拉取完成: {} 更新, {} 删除, {} 冲突",
        applied,
        deleted,
        conflicts.len(),
    );
    if !conflicts.is_empty() {
        println!("\n冲突文件 (theirs 已写到 *.dup-theirs，ours 保留):");
        for c in &conflicts {
            println!("  \x1b[31mCONFLICT\x1b[0m {}", c);
        }
        println!("\n解决后用 'dup commit -m \"merge\"' 提交。");
    }
    Ok(())
}

/// Write `content` to `workspace/rel` atomically (tmp + rename), creating dirs.
fn atomic_write(workspace: &Path, rel: &str, content: &[u8]) -> Result<()> {
    let target = workspace.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("duptmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, &target)?;
    Ok(())
}
