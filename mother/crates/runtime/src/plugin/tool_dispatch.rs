//! Plugin tool dispatcher — routes tool calls to loaded plugins.

use std::sync::Arc;

use dashmap::DashMap;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::plugin::{PluginToolContext, PluginToolDef};
use carrier_types::tool::ToolDefinition;

use super::instance::PluginInstance;

// ---------------------------------------------------------------------------
// Tool entry
// ---------------------------------------------------------------------------

/// Entry mapping a tool name to its owning plugin.
struct PluginToolEntry {
    /// The tool definition (description + parameter schema).
    definition: PluginToolDef,
    /// Reference to the loaded plugin (for execution).
    plugin: Arc<dyn PluginInstance>,
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Dispatches plugin tool calls to the appropriate loaded plugin.
pub struct PluginToolDispatcher {
    tools: DashMap<String, PluginToolEntry>,
}

impl PluginToolDispatcher {
    /// Create a new empty dispatcher.
    pub fn new() -> Self {
        Self {
            tools: DashMap::new(),
        }
    }

    /// Register all tools from a loaded plugin.
    pub fn register(&self, plugin: Arc<dyn PluginInstance>) {
        for tool_def in plugin.tools() {
            let tool_name = tool_def.name.clone();
            self.tools.insert(
                tool_name,
                PluginToolEntry {
                    definition: tool_def.clone(),
                    plugin: plugin.clone(),
                },
            );
        }
    }

    /// Check if a tool name is provided by any plugin.
    pub fn has_tool(&self, tool_name: &str) -> bool {
        self.tools.contains_key(tool_name)
    }

    /// Get all plugin tool definitions (for LLM tool list).
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .filter_map(|entry| {
                let schema: serde_json::Value =
                    serde_json::from_str(&entry.definition.parameters_json).ok()?;
                Some(ToolDefinition {
                    name: entry.definition.name.clone(),
                    description: entry.definition.description.clone(),
                    input_schema: schema,
                })
            })
            .collect()
    }

    /// Execute a plugin tool via C ABI.
    pub fn execute(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        context: &PluginToolContext,
    ) -> CarrierResult<String> {
        let entry = self
            .tools
            .get(tool_name)
            .ok_or_else(|| CarrierError::Internal(format!("Unknown plugin tool: {}", tool_name)))?;

        let args_json =
            serde_json::to_string(args).map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let context_json = serde_json::to_string(context)
            .map_err(|e| CarrierError::Serialization(e.to_string()))?;

        entry
            .plugin
            .tool_execute(tool_name, &args_json, &context_json)
            .map_err(|e| CarrierError::ToolExecution {
                tool_id: tool_name.to_string(),
                reason: e.to_string(),
            })
    }
}

impl Default for PluginToolDispatcher {
    fn default() -> Self {
        Self::new()
    }
}
