//! Configuration management - `~/.dupconfig` (global).
//!
//! The remote is duphub (templates 形状) or an opencarrier runtime
//! (clones 形状) — the "origin" for file-level sync.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Configuration container.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DupConfig {
    #[serde(default)]
    pub remote: RemoteSection,
    /// Backward-compat alias for older configs that used `[hub]`.
    #[serde(default)]
    pub hub: RemoteSection,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RemoteSection {
    /// Remote base URL (e.g. "https://duphub.com" or a private runtime).
    #[serde(default)]
    pub url: String,
    /// API key (Bearer) for the remote.
    #[serde(default)]
    pub api_key: String,
    /// Endpoint shape: "templates" (duphub, default) or "clones" (runtime).
    #[serde(default)]
    pub api: String,
}

impl DupConfig {
    /// Load global config from `~/.dupconfig`.
    pub fn load_global() -> Result<Self> {
        let path = global_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&content).unwrap_or_default())
        } else {
            Ok(DupConfig::default())
        }
    }

    /// Save config to `~/.dupconfig`.
    pub fn save_global(&self) -> Result<()> {
        let path = global_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml_str = toml::to_string_pretty(self)?;
        std::fs::write(&path, toml_str)?;
        Ok(())
    }

    /// The effective remote section (prefer `[remote]`, fall back to `[hub]`).
    fn remote(&self) -> &RemoteSection {
        if !self.remote.url.is_empty() || !self.remote.api_key.is_empty() {
            &self.remote
        } else {
            &self.hub
        }
    }

    /// Resolve the effective remote URL (env var first, then config).
    /// Port default is duphub — the phone pulls 分身 from the hub.
    pub fn resolve_url(&self) -> String {
        if let Ok(v) = std::env::var("OPENCARRIER_URL") {
            if !v.trim().is_empty() {
                return v.trim_end_matches('/').to_string();
            }
        }
        let url = self.remote().url.trim_end_matches('/');
        if !url.is_empty() {
            url.to_string()
        } else {
            "https://duphub.com".to_string()
        }
    }

    /// Resolve the effective endpoint shape ("templates" or "clones").
    /// templates = duphub `/api/templates/{name}/dup/*` (default);
    /// clones = runtime `/api/clones/{name}/dup/*` (upstream dup default).
    pub fn resolve_api(&self) -> String {
        let api = self.remote().api.trim();
        if api == "clones" {
            "clones".to_string()
        } else {
            "templates".to_string()
        }
    }

    /// Resolve the effective API key (env var first, then config).
    pub fn resolve_api_key(&self) -> Result<String> {
        if let Ok(v) = std::env::var("OPENCARRIER_API_KEY") {
            if !v.trim().is_empty() {
                return Ok(v);
            }
        }
        let key = self.remote().api_key.trim();
        if !key.is_empty() {
            Ok(key.to_string())
        } else {
            Err(anyhow::anyhow!(
                "API Key 未设置。请运行 'dup config remote.api_key <key>' 设置。"
            ))
        }
    }

    /// Set a config key (e.g. `remote.url`, `remote.api_key`, `remote.api`).
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "remote.url" => self.remote.url = value.to_string(),
            "remote.api_key" => self.remote.api_key = value.to_string(),
            "remote.api" => self.remote.api = value.to_string(),
            "hub.url" => self.hub.url = value.to_string(),
            "hub.api_key" => self.hub.api_key = value.to_string(),
            _ => anyhow::bail!("未知配置项: {key}（支持: remote.url, remote.api_key, remote.api）"),
        }
        Ok(())
    }
}

fn global_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".dupconfig")
}
