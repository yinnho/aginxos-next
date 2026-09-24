//! `dup log` - show local commit history.

use anyhow::Result;

use crate::state::DupState;
use crate::workspace;

pub fn run() -> Result<()> {
    let ws = workspace::require_workspace()?;
    if !workspace::state_path(&ws).exists() {
        anyhow::bail!("未初始化。请先运行 'dup init'。");
    }
    let state = DupState::load(&ws)?;

    if state.commits.is_empty() {
        println!("暂无提交记录");
        return Ok(());
    }

    for commit in &state.commits {
        let short_hash = &commit.hash[..commit.hash.len().min(12)];
        let short_date = &commit.timestamp[..commit.timestamp.len().min(19)];
        println!("\x1b[33mcommit {}\x1b[0m", short_hash);
        println!("Author: {}", commit.author);
        println!("Date:   {}", short_date);
        println!();
        println!("    {}", commit.message);
        println!();
    }

    Ok(())
}
