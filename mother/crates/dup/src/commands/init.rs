//! `dup init` - link a workspace to the opencarrier remote.
//!
//! Sets `remote_url` / `remote_name` / `remote_api_key` in `.dup/state.json`
//! (resolving URL+key from config/env). Preserves existing commits if state
//! already exists (e.g. after `dup create`). Does not pull - run `dup pull`
//! afterwards to align with the remote.


use anyhow::Result;

use crate::config::DupConfig;
use crate::state::DupState;
use crate::workspace;

pub fn run(name: Option<String>) -> Result<()> {
    let ws = workspace::require_workspace()?;
    let config = DupConfig::load_global()?;
    let url = config.resolve_url();
    let api_key = config.resolve_api_key()?;
    let remote_name = name.unwrap_or_else(|| {
        ws.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });

    let mut state = if workspace::state_path(&ws).exists() {
        DupState::load(&ws)?
    } else {
        DupState::new(&url, &remote_name, &api_key)
    };
    state.remote_url = url.clone();
    state.remote_name = remote_name.clone();
    state.remote_api_key = api_key;
    state.save(&ws)?;

    println!("已配置 dup 远程:");
    println!("  remote: {}", url);
    println!("  name:   {}", remote_name);
    println!("  用 'dup pull' 拉取远程对齐");
    Ok(())
}

