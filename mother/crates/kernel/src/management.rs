//! Kernel management — handles, bindings, config reload, events, shutdown, brain.
//!
//! Groups the miscellaneous kernel management methods: self-handle setup,
//! agent binding CRUD, config hot-reload, event publishing, graceful shutdown,
//! and Brain read/write/reload operations.

use crate::brain::Brain;
use crate::error::{KernelError, KernelResult};
use crate::kernel::CarrierKernel;
use carrier_runtime::llm_driver::LlmDriver;
use std::sync::Arc;
use tracing::{info, warn};
use carrier_types::agent::*;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::event::*;

impl CarrierKernel {
    // ── Self-handle ───────────────────────────────────────────

    /// Get a kernel handle for passing to agent loop operations.
    ///
    /// Returns `None` if `set_self_handle` hasn't been called yet.
    pub fn get_kernel_handle(
        self: &Arc<Self>,
    ) -> Option<Arc<dyn carrier_runtime::kernel_handle::KernelHandle>> {
        self.coordination
            .self_handle
            .get()
            .and_then(|w| w.upgrade())
            .map(|arc| arc as Arc<dyn carrier_runtime::kernel_handle::KernelHandle>)
    }

    pub fn set_self_handle(self: &Arc<Self>) {
        let _ = self.coordination.self_handle.set(Arc::downgrade(self));
    }

    // ── Agent Binding management ──────────────────────────────

    // ── Config hot-reload ─────────────────────────────────────

    /// Reload configuration: read the config file, diff against current, and
    /// apply hot-reloadable actions. Returns the reload plan for API response.
    pub fn reload_config(&self) -> CarrierResult<crate::config_reload::ReloadPlan> {
        use crate::config_reload::{
            build_reload_plan, should_apply_hot, validate_config_for_reload,
        };

        // Read and parse config file (using load_config to process $include directives)
        let config_path = self.config.home_dir.join("config.toml");
        let new_config = if config_path.exists() {
            crate::config::load_config(Some(&config_path))
        } else {
            return Err(CarrierError::Internal("Config file not found".into()));
        };

        // Validate new config
        if let Err(errors) = validate_config_for_reload(&new_config) {
            return Err(CarrierError::Internal(format!(
                "Validation failed: {}",
                errors.join("; ")
            )));
        }

        // Build the reload plan
        let plan = build_reload_plan(&self.config, &new_config);
        plan.log_summary();

        // Apply hot actions if the reload mode allows it
        if should_apply_hot(self.config.reload.mode, &plan) {
            self.apply_hot_actions(&plan, &new_config);
        }

        Ok(plan)
    }

    /// Apply hot-reload actions to the running kernel.
    fn apply_hot_actions(
        &self,
        plan: &crate::config_reload::ReloadPlan,
        new_config: &carrier_types::config::KernelConfig,
    ) {
        use crate::config_reload::HotAction;

        for action in &plan.hot_actions {
            match action {
                HotAction::UpdateCronConfig => {
                    info!(
                        "Hot-reload: updating cron config (max_jobs={})",
                        new_config.max_cron_jobs
                    );
                    self.cron_scheduler
                        .set_max_total_jobs(new_config.max_cron_jobs);
                }
                HotAction::ReloadMcpServers => {
                    info!("Hot-reload: reloading MCP servers");
                    let new_mcp = &new_config.mcp_servers;
                    let old_names: Vec<String> = self
                        .plugins
                        .effective_mcp_servers
                        .read()
                        .map(|s| s.iter().map(|c| c.name.clone()).collect())
                        .unwrap_or_default();
                    let new_names: Vec<String> = new_mcp.iter().map(|c| c.name.clone()).collect();

                    // Remove connections for servers no longer in config
                    {
                        let removed: Vec<&str> = old_names
                            .iter()
                            .filter(|n| !new_names.contains(n))
                            .map(|s| s.as_str())
                            .collect();
                        if !removed.is_empty() {
                            for name in &removed {
                                info!(server = %name, "MCP server removed from config — will disconnect on next health check");
                            }
                        }
                    }

                    // Update effective config
                    if let Ok(mut effective) = self.plugins.effective_mcp_servers.write() {
                        *effective = new_mcp.clone();
                    }
                    info!(
                        old_count = old_names.len(),
                        new_count = new_names.len(),
                        "MCP server config updated — changes take effect on next health check cycle"
                    );
                }
                _ => {
                    info!(
                        "Hot-reload: action {:?} noted but not yet auto-applied",
                        action
                    );
                }
            }
        }
    }

    // ── Events ────────────────────────────────────────────────

    /// Publish an event to the event bus.
    pub async fn publish_event(&self, event: Event) -> Vec<(AgentId, String)> {
        self.coordination.event_bus.publish(event).await;
        Vec::new()
    }

    // ── Shutdown ──────────────────────────────────────────────

    /// Gracefully shutdown the kernel.
    ///
    /// This cleanly shuts down in-memory state but preserves persistent agent
    /// data so agents are restored on the next boot.
    pub fn shutdown(&self) {
        info!("Shutting down Carrier kernel...");

        self.runtime.supervisor.shutdown();

        // Persist all agents with their current state so latest config is
        // preserved across restarts. Do NOT alter state — Running agents
        // should resume as Running after reboot.
        for entry in self.registry.list() {
            if let Some(updated) = self.registry.get(entry.id) {
                let _ = self.memory.save_agent(&updated);
            }
        }

        info!(
            "Carrier kernel shut down ({} agents preserved)",
            self.registry.list().len()
        );
    }

    // ── Brain access ──────────────────────────────────────────

    /// Return a cloned Arc<Brain> for the API (None if not loaded).
    pub fn brain_info(&self) -> Arc<Brain> {
        Arc::clone(&*self.brain.brain.read().unwrap_or_else(|e| {
            warn!("Brain RwLock poisoned, recovering");
            e.into_inner()
        }))
    }

    /// Acquire a read lock on the Brain (for validation before updates).
    pub fn brain_read(&self) -> std::sync::RwLockReadGuard<'_, Arc<Brain>> {
        self.brain.brain.read().unwrap_or_else(|e| {
            warn!("Brain RwLock poisoned, recovering");
            e.into_inner()
        })
    }

    /// Resolve a human-readable (modality, model_name) pair for display.
    pub fn resolve_model_label(&self, modality: &str) -> (String, String) {
        let brain = self.brain.brain.read().unwrap_or_else(|e| {
            warn!("Brain RwLock poisoned, recovering");
            e.into_inner()
        });
        let model = brain.model_for(modality);
        (modality.to_string(), model)
    }

    pub fn resolve_driver(&self, manifest: &AgentManifest) -> KernelResult<Arc<dyn LlmDriver>> {
        let brain = self.brain.brain.read().unwrap_or_else(|e| {
            warn!("Brain RwLock poisoned, recovering");
            e.into_inner()
        });
        let modality = if manifest.model.modality.is_empty() {
            "chat"
        } else {
            &manifest.model.modality
        };

        // Check if modality exists at all
        if !brain.has_modality(modality) {
            return Err(KernelError::Carrier(CarrierError::LlmDriver(format!(
                "Modality '{modality}' not configured in brain.json"
            ))));
        }

        let endpoints = brain.endpoints_for(modality);
        if let Some(ep) = endpoints.first() {
            if let Some(driver) = brain.driver_for_endpoint(&ep.id) {
                return Ok(driver);
            }
        }

        // endpoints_for returned empty — all circuit-broken or no drivers
        let status = brain.status();
        let broken: Vec<String> = status
            .endpoints
            .iter()
            .filter(|e| e.circuit_open)
            .map(|e| {
                format!(
                    "{} ({} consecutive failures)",
                    e.endpoint, e.consecutive_failures
                )
            })
            .collect();

        if broken.is_empty() {
            Err(KernelError::Carrier(CarrierError::LlmDriver(format!(
                "No driver available for modality '{modality}' — endpoints have no live drivers"
            ))))
        } else {
            Err(KernelError::Carrier(CarrierError::LlmDriver(format!(
                "No available endpoints for modality '{modality}' — circuit-broken: [{}]",
                broken.join(", ")
            ))))
        }
    }

    /// Reload Brain from disk (brain.json). Used by the API to hot-reload after config changes.
    pub fn reload_brain(&self) -> CarrierResult<()> {
        let json_str = std::fs::read_to_string(&self.brain.brain_path).map_err(|e| {
            CarrierError::Internal(format!(
                "Cannot read {}: {e}",
                self.brain.brain_path.display()
            ))
        })?;
        let config: carrier_types::brain::BrainConfig = serde_json::from_str(&json_str)
            .map_err(|e| CarrierError::Internal(format!("Invalid brain.json: {e}")))?;
        let brain = Brain::new(config)
            .map_err(|e| CarrierError::Internal(format!("Brain init failed: {e}")))?;
        *self.brain.brain.write().unwrap_or_else(|e| {
            warn!("Brain RwLock poisoned, recovering");
            e.into_inner()
        }) = Arc::new(brain);
        info!("Brain reloaded from {}", self.brain.brain_path.display());
        Ok(())
    }

    /// Update Brain config: clone config, apply mutation, persist to disk, hot-reload.
    pub fn update_brain<F>(&self, f: F) -> CarrierResult<()>
    where
        F: FnOnce(&mut carrier_types::brain::BrainConfig),
    {
        // Read current config
        let mut config = {
            let guard = self.brain.brain.read().unwrap_or_else(|e| {
                warn!("Brain RwLock poisoned, recovering");
                e.into_inner()
            });
            guard.config().clone()
        };

        // Apply mutation
        f(&mut config);

        // Persist to disk
        let json_str = serde_json::to_string_pretty(&config)
            .map_err(|e| CarrierError::Internal(format!("Cannot serialize brain config: {e}")))?;
        std::fs::write(&self.brain.brain_path, &json_str).map_err(|e| {
            CarrierError::Internal(format!(
                "Cannot write {}: {e}",
                self.brain.brain_path.display()
            ))
        })?;

        // Hot-reload: create new Brain from updated config
        let brain = Brain::new(config)
            .map_err(|e| CarrierError::Internal(format!("Brain init failed after update: {e}")))?;
        *self.brain.brain.write().unwrap_or_else(|e| {
            warn!("Brain RwLock poisoned, recovering");
            e.into_inner()
        }) = Arc::new(brain);
        info!("Brain config updated and reloaded");
        Ok(())
    }

    /// Return the path to brain.json.
    pub fn brain_path(&self) -> &std::path::Path {
        &self.brain.brain_path
    }
}
