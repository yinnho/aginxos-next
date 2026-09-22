//! `.dup/state.json` - VCS state for dup workspaces.
//!
//! Local file-level VCS state. `remote_base` is the manifest last pulled from
//! the opencarrier remote (= merge base for 3-way sync). `commits` is the local
//! version history; each commit's file contents live in the content-addressed
//! `.dup/objects/` store (written at commit time), so any committed state can
//! be restored locally even though the remote keeps no history.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::workspace;

/// The complete VCS state stored in `.dup/state.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DupState {
    /// opencarrier base URL (the "origin" remote).
    #[serde(default)]
    pub remote_url: String,
    /// Clone name on the remote.
    #[serde(default)]
    pub remote_name: String,
    /// API key for the remote (Bearer). Stored locally for convenience.
    #[serde(default)]
    pub remote_api_key: String,
    /// Manifest last pulled from the remote (= merge base). None before first pull.
    #[serde(default)]
    pub remote_base: Option<Manifest>,
    /// Local commit history, newest first.
    #[serde(default)]
    pub commits: Vec<CommitEntry>,
}

/// A single local commit (snapshot of the working tree at commit time).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitEntry {
    /// Manifest hash (state id) at this commit.
    pub hash: String,
    /// Manifest snapshot (path -> sha256) at this commit.
    pub manifest: Manifest,
    pub message: String,
    pub timestamp: String,
    pub author: String,
}

impl DupState {
    /// Load state from a workspace's `.dup/state.json`.
    pub fn load(workspace: &Path) -> Result<Self> {
        let path = workspace::state_path(workspace);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("无法读取状态文件: {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("解析状态文件失败: {}", path.display()))
    }

    /// Save state to disk.
    pub fn save(&self, workspace: &Path) -> Result<()> {
        let dup = workspace::dup_dir(workspace);
        std::fs::create_dir_all(&dup)?;
        let path = workspace::state_path(workspace);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
            .with_context(|| format!("无法写入状态文件: {}", path.display()))
    }

    /// Create a fresh state pointing at a remote (no commits, no base yet).
    pub fn new(remote_url: &str, remote_name: &str, remote_api_key: &str) -> Self {
        DupState {
            remote_url: remote_url.to_string(),
            remote_name: remote_name.to_string(),
            remote_api_key: remote_api_key.to_string(),
            remote_base: None,
            commits: Vec::new(),
        }
    }

    /// The most recent commit, if any.
    pub fn last_commit(&self) -> Option<&CommitEntry> {
        self.commits.first()
    }

    /// Append a commit (newest first).
    pub fn add_commit(&mut self, manifest: Manifest, message: &str) {
        let hash = manifest.hash.clone();
        self.commits.insert(
            0,
            CommitEntry {
                hash,
                manifest,
                message: message.to_string(),
                timestamp: now_rfc3339(),
                author: whoami(),
            },
        );
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}
