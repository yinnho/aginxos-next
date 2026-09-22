//! Plugin dependency resolution.
//!
//! Handles downloading missing plugins from Hub while preserving user data.

use crate::kernel::CarrierKernel;

impl CarrierKernel {
    /// Resolve plugin dependencies for a newly installed clone.
    /// Downloads missing plugins from Hub.
    pub async fn resolve_plugin_dependencies(&self, plugins: &[String]) {
        let plugins_dir = match &self.config.plugins_dir {
            Some(dir) => dir.clone(),
            None => {
                let dir = self.config.home_dir.join("plugins");
                tracing::info!(
                    "No plugins_dir configured, using default: {}",
                    dir.display()
                );
                dir
            }
        };

        // Ensure plugins directory exists
        if let Err(e) = std::fs::create_dir_all(&plugins_dir) {
            tracing::warn!(
                "Failed to create plugins dir {}: {e}",
                plugins_dir.display()
            );
            return;
        }

        let hub_url = self.config.hub.url.trim_end_matches('/').to_string();
        let api_key_env = &self.config.hub.api_key_env;
        let api_key = match std::env::var(api_key_env) {
            Ok(k) => k,
            Err(_) => {
                tracing::warn!(
                    "Hub API key not set (env: {}), skipping plugin dependency resolution",
                    api_key_env
                );
                return;
            }
        };

        let mut installed = Vec::new();
        let mut failed = Vec::new();

        for plugin_name in plugins {
            if carrier_clone::hub::is_plugin_installed(&plugins_dir, plugin_name) {
                tracing::info!(plugin = %plugin_name, "Plugin already installed, skipping");
                continue;
            }

            tracing::info!(plugin = %plugin_name, "Downloading missing plugin from Hub...");
            match carrier_clone::hub::install_plugin(&hub_url, &api_key, plugin_name, None, &plugins_dir)
                .await
            {
                Ok(_) => {
                    tracing::info!(plugin = %plugin_name, "Plugin installed successfully");
                    installed.push(plugin_name.clone());
                }
                Err(e) => {
                    tracing::warn!(plugin = %plugin_name, error = %e, "Failed to install plugin");
                    failed.push(plugin_name.clone());
                }
            }
        }

        if !installed.is_empty() || !failed.is_empty() {
            tracing::info!(
                installed = ?installed,
                failed = ?failed,
                "Plugin dependency resolution complete (restart required to load new plugins)"
            );
        }
    }
}
