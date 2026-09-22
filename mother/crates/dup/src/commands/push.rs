//! `dup push` - fast-forward push of local changes to the opencarrier remote.
//!
//! Discipline: the remote must not have evolved since the last pull. We verify
//! by fetching the remote manifest and requiring it to equal `remote_base`; if
//! it moved, we refuse (pull first). On a clean fast-forward we POST only the
//! changed files + deletes relative to the base, then advance `remote_base` to
//! our new manifest.

use std::collections::BTreeMap;

use anyhow::{Context, Result};

use crate::config::DupConfig;
use crate::manifest::build_manifest;
use crate::remote;
use crate::state::DupState;
use crate::workspace;

pub async fn run() -> Result<()> {
    let ws = workspace::require_workspace()?;
    if !workspace::state_path(&ws).exists() {
        anyhow::bail!("未初始化。请先运行 'dup init'。");
    }
    let mut state = DupState::load(&ws)?;

    let base = state
        .remote_base
        .clone()
        .ok_or_else(|| anyhow::anyhow!("尚无基准（未 pull 过）。请先运行 'dup pull'。"))?;

    let config = DupConfig::load_global()?;
    let url = config.resolve_url();
    let api = config.resolve_api();
    let api_key = config.resolve_api_key()?;
    let name = state.remote_name.clone();

    // Fast-forward check: remote manifest must equal our base.
    println!("正在校验远程状态 ...");
    let remote_manifest = remote::get_manifest(&url, &api_key, &api, &name).await?;
    if remote_manifest.hash != base.hash {
        anyhow::bail!(
            "opencarrier 已演进（远程 manifest 变了）。先 'dup pull' 对齐再推送。"
        );
    }

    let ours = build_manifest(&ws).context("构建本地 manifest 失败")?;

    // Changes relative to base: files whose hash differs (or new) -> send;
    // files in base but missing locally -> delete.
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut deletes: Vec<String> = Vec::new();
    for (path, sha) in &ours.files {
        if base.files.get(path) != Some(sha) {
            let data = std::fs::read(ws.join(path))
                .with_context(|| format!("读取 {} 失败", path))?;
            files.insert(path.clone(), data);
        }
    }
    for path in base.files.keys() {
        if !ours.files.contains_key(path) {
            deletes.push(path.clone());
        }
    }

    if files.is_empty() && deletes.is_empty() {
        println!("没有变更可推送");
        return Ok(());
    }

    println!(
        "推送 {} 个文件变更, {} 个删除 到 {} ...",
        files.len(),
        deletes.len(),
        url,
    );

    let new_manifest =
        remote::post_push(&url, &api_key, &api, &name, &base.hash, &files, &deletes).await?;

    state.remote_base = Some(new_manifest.clone());
    state.save(&ws)?;

    println!("推送成功。远程 manifest: {}", &new_manifest.hash[..new_manifest.hash.len().min(16)]);
    Ok(())
}
