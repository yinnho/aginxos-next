//! Capability manager — enforces capability-based security.

use dashmap::DashMap;
use carrier_types::agent::AgentId;
use carrier_types::capability::Capability;

/// Manages capability grants for all agents.
pub struct CapabilityManager {
    /// Granted capabilities per agent.
    grants: DashMap<AgentId, Vec<Capability>>,
}

impl CapabilityManager {
    /// Create a new capability manager.
    pub fn new() -> Self {
        Self {
            grants: DashMap::new(),
        }
    }

    /// Grant capabilities to an agent.
    pub fn grant(&self, agent_id: AgentId, capabilities: Vec<Capability>) {
        self.grants.insert(agent_id, capabilities);
    }

    /// Remove all capabilities for an agent.
    pub fn revoke_all(&self, agent_id: AgentId) {
        self.grants.remove(&agent_id);
    }
}

impl Default for CapabilityManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert an AgentManifest's capability config into a flat list of Capability enum values.
/// Handles profile-based implied capabilities with explicit overrides.
pub fn manifest_to_capabilities(manifest: &carrier_types::agent::AgentManifest) -> Vec<Capability> {
    let mut caps = Vec::new();

    // Profile expansion: use profile's implied capabilities when no explicit tools
    let effective_caps = if let Some(ref profile) = manifest.profile {
        if manifest.capabilities.tools.is_empty() {
            let mut merged = profile.implied_capabilities();
            if !manifest.capabilities.network.is_empty() {
                merged.network = manifest.capabilities.network.clone();
            }
            if !manifest.capabilities.shell.is_empty() {
                merged.shell = manifest.capabilities.shell.clone();
            }
            if !manifest.capabilities.agent_message.is_empty() {
                merged.agent_message = manifest.capabilities.agent_message.clone();
            }
            if manifest.capabilities.agent_spawn {
                merged.agent_spawn = true;
            }
            if !manifest.capabilities.memory_read.is_empty() {
                merged.memory_read = manifest.capabilities.memory_read.clone();
            }
            if !manifest.capabilities.memory_write.is_empty() {
                merged.memory_write = manifest.capabilities.memory_write.clone();
            }
            if manifest.capabilities.ofp_discover {
                merged.ofp_discover = true;
            }
            if !manifest.capabilities.ofp_connect.is_empty() {
                merged.ofp_connect = manifest.capabilities.ofp_connect.clone();
            }
            merged
        } else {
            manifest.capabilities.clone()
        }
    } else {
        manifest.capabilities.clone()
    };

    for host in &effective_caps.network {
        caps.push(Capability::NetConnect(host.clone()));
    }
    for tool in &effective_caps.tools {
        caps.push(Capability::ToolInvoke(tool.clone()));
    }
    for scope in &effective_caps.memory_read {
        caps.push(Capability::MemoryRead(scope.clone()));
    }
    for scope in &effective_caps.memory_write {
        caps.push(Capability::MemoryWrite(scope.clone()));
    }
    if effective_caps.agent_spawn {
        caps.push(Capability::AgentSpawn);
    }
    for pattern in &effective_caps.agent_message {
        caps.push(Capability::AgentMessage(pattern.clone()));
    }
    for cmd in &effective_caps.shell {
        caps.push(Capability::ShellExec(cmd.clone()));
    }
    if effective_caps.ofp_discover {
        caps.push(Capability::OfpDiscover);
    }
    for peer in &effective_caps.ofp_connect {
        caps.push(Capability::OfpConnect(peer.clone()));
    }

    caps
}
