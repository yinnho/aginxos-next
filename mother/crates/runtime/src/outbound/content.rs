//! Per-agent `content.toml` loader with mtime cache.
//!
//! Shared by the interactive bridge, cron delivery, and `/api/deliver` so there
//! is one parse path for deliverable descriptors.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use dashmap::DashMap;
use tracing::warn;
use carrier_types::content::ContentConfig;

/// Cached loader for agent `content.toml` files.
///
/// Cache key is `agent_id`; entries are invalidated when the file's mtime changes.
#[derive(Debug, Default)]
pub struct ContentRegistry {
    cache: DashMap<String, (SystemTime, Arc<ContentConfig>)>,
}

impl ContentRegistry {
    pub fn new() -> Self {
        Self {
            cache: DashMap::new(),
        }
    }

    /// Process-wide registry so bridge, cron, and API share one mtime cache.
    pub fn global() -> &'static ContentRegistry {
        static REG: OnceLock<ContentRegistry> = OnceLock::new();
        REG.get_or_init(ContentRegistry::new)
    }

    /// Load `workspace/content.toml` for `agent_id`, reusing the cache when the
    /// file mtime is unchanged. Returns `None` if the agent id is empty, the
    /// file is missing, or TOML parse fails.
    pub fn load(&self, agent_id: &str, workspace: &Path) -> Option<Arc<ContentConfig>> {
        if agent_id.is_empty() {
            return None;
        }
        let path = workspace.join("content.toml");
        let mtime = std::fs::metadata(&path).ok()?.modified().ok()?;
        if let Some(entry) = self.cache.get(agent_id) {
            if entry.0 == mtime {
                return Some(entry.1.clone());
            }
        }
        let text = std::fs::read_to_string(&path).ok()?;
        let config: ContentConfig = match toml::from_str(&text) {
            Ok(c) => c,
            Err(e) => {
                warn!(%agent_id, path = %path.display(), error = %e, "Failed to parse content.toml");
                return None;
            }
        };
        let config = Arc::new(config);
        self.cache
            .insert(agent_id.to_string(), (mtime, config.clone()));
        Some(config)
    }
}
