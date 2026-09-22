//! `dup restore` - bring back files from a local commit's object store.
//!
//! Rollback is purely local: the remote is stateless (current-state only), so
//! history lives in `.dup/objects/` + the commit manifests in state.json.
//!
//! - `dup restore <hash>` - full rollback to that commit's snapshot: every
//!   file in the commit manifest is restored, files added since are deleted.
//! - `dup restore <hash> <path...>` - restore only the given paths.
//!
//! Never destructive: if the working tree has uncommitted changes, an
//! automatic backup commit ("自动快照: restore 前备份") is created first, so
//! the pre-restore state is itself recoverable. After restoring, the changes
//! show up in `dup status`; propagate with the usual commit + push.

use std::path::Path;

use anyhow::{Context, Result};

use crate::diff::diff_manifests;
use crate::manifest::build_manifest;
use crate::objects;
use crate::state::{CommitEntry, DupState};
use crate::workspace;

pub fn run(target: &str, paths: &[String]) -> Result<()> {
    let ws = workspace::require_workspace()?;
    if !workspace::state_path(&ws).exists() {
        anyhow::bail!("未初始化。请先运行 'dup init'。");
    }
    let mut state = DupState::load(&ws)?;

    // Resolve the commit by hash prefix. Auto-backup snapshots can share a
    // hash with an existing commit (identical content), so dedup by hash:
    // multiple entries with the SAME hash are one restorable state.
    let matches: Vec<&CommitEntry> = state
        .commits
        .iter()
        .filter(|c| c.hash.starts_with(target))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let mut unique: Vec<&CommitEntry> = Vec::new();
    for c in matches {
        if seen.insert(c.hash.as_str()) {
            unique.push(c);
        }
    }
    let commit = match unique.len() {
        0 => anyhow::bail!("找不到 commit: {}（用 'dup log' 查看历史）", target),
        1 => unique[0].clone(),
        n => anyhow::bail!("hash 前缀 '{}' 不唯一（{} 个匹配），请加长前缀", target, n),
    };
    let short = &commit.hash[..commit.hash.len().min(12)];

    // Determine which paths to restore.
    let targets: Vec<(String, String)> = if paths.is_empty() {
        commit.manifest.files.iter().map(|(p, s)| (p.clone(), s.clone())).collect()
    } else {
        let mut v = Vec::new();
        for p in paths {
            let sha = commit
                .manifest
                .files
                .get(p)
                .with_context(|| format!("commit {} 中没有文件 {}", short, p))?;
            v.push((p.clone(), sha.clone()));
        }
        v
    };

    // Pre-flight: ALL needed objects must exist before we touch the tree, so
    // a restore never leaves a half-restored workspace.
    let missing: Vec<&str> = targets
        .iter()
        .filter(|(_, sha)| !objects::has_object(&ws, sha))
        .map(|(p, _)| p.as_str())
        .collect();
    if !missing.is_empty() {
        eprintln!("以下文件在本地对象库中缺失，无法恢复:");
        for p in &missing {
            eprintln!("  {}", p);
        }
        anyhow::bail!(
            "commit {} 早于对象库功能（旧版 dup 只存哈希不存内容），无法回滚到它",
            short
        );
    }

    // Never lose data: dirty working tree -> auto backup commit first.
    let current = build_manifest(&ws)?;
    let dirty = match state.last_commit() {
        Some(last) => !diff_manifests(&last.manifest, &current).is_empty(),
        None => !current.files.is_empty(),
    };
    if dirty {
        objects::store_objects(&ws, &current)?;
        state.add_commit(current.clone(), "自动快照: restore 前备份");
        println!(
            "工作区有未提交变更，已自动备份为 [{}]",
            &current.hash[..current.hash.len().min(12)]
        );
    }

    // Restore files.
    for (rel, sha) in &targets {
        let data = objects::read_object(&ws, sha)?;
        atomic_write(&ws, rel, &data)?;
    }

    // Full rollback: delete files that don't exist in the target commit.
    let mut deleted = 0usize;
    if paths.is_empty() {
        for rel in current.files.keys() {
            if !commit.manifest.files.contains_key(rel) {
                let p = ws.join(rel);
                if p.exists() {
                    std::fs::remove_file(&p).ok();
                    deleted += 1;
                }
            }
        }
    }

    state.save(&ws)?;

    println!(
        "已从 [{}] {} 恢复 {} 个文件{}",
        short,
        commit.message,
        targets.len(),
        if deleted > 0 {
            format!("，删除 {} 个多余文件", deleted)
        } else {
            String::new()
        }
    );
    println!("变更在工作区中（dup status 可见）。确认后用 commit + push 推到远程。");
    Ok(())
}

/// Write `content` to `workspace/rel` atomically (tmp + rename), creating dirs.
fn atomic_write(workspace: &Path, rel: &str, content: &[u8]) -> Result<()> {
    let target = workspace.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = target.with_extension("duptmp");
    std::fs::write(&tmp, content).with_context(|| format!("写入 {} 失败", rel))?;
    std::fs::rename(&tmp, &target).with_context(|| format!("落盘 {} 失败", rel))?;
    Ok(())
}
