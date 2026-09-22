//! `dup clone <name>` - create a local workspace from the opencarrier remote.
//!
//! Replaces the old DupHub install: fetches the remote manifest + every file
//! into a fresh directory, then initializes `.dup/` state with `remote_base` =
//! the fetched manifest (we're aligned). Equivalent to `dup init` + first
//! `dup pull` into an empty dir.

use anyhow::Result;

use crate::config::DupConfig;
use crate::manifest::build_manifest;
use crate::remote;
use crate::state::DupState;

pub async fn run(name: &str) -> Result<()> {
    let config = DupConfig::load_global()?;
    let url = config.resolve_url();
    let api = config.resolve_api();
    let api_key = config.resolve_api_key()?;

    let cwd = std::env::current_dir()?;
    let target_dir = cwd.join(name);
    if target_dir.exists() {
        anyhow::bail!("目录 '{}' 已存在", target_dir.display());
    }
    std::fs::create_dir_all(&target_dir)?;

    println!("正在从 {} 克隆 {} ...", url, name);

    let theirs = remote::get_manifest(&url, &api_key, &api, name).await?;
    for path in theirs.files.keys() {
        let content = remote::get_file(&url, &api_key, &api, name, path).await?;
        let target = target_dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &content)?;
    }

    // Sanity: local manifest should match the remote we just wrote.
    let local = build_manifest(&target_dir)?;

    // Init state: aligned with remote, so remote_base = theirs. Record an
    // initial commit so `dup status` starts clean (working == last commit).
    let mut state = DupState::new(&url, name, &api_key);
    state.remote_base = Some(theirs.clone());
    state.add_commit(local.clone(), "clone: 初始同步");
    state.save(&target_dir)?;

    if local.hash != theirs.hash {
        eprintln!(
            "警告: 克隆后本地 manifest 与远程不一致 ({} vs {})",
            &local.hash[..local.hash.len().min(8)],
            &theirs.hash[..theirs.hash.len().min(8)],
        );
    } else {
        println!(
            "已克隆 '{}'（{} 文件）到 {}",
            name,
            theirs.files.len(),
            target_dir.display(),
        );
    }
    Ok(())
}
