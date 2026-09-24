//! Built-in plugin registry — maps plugin names to factory functions.
//!
//! Registered at compile time. Used by `PluginLoader::load_all()` to
//! construct `BuiltinPlugin` instances for channels that are shipped
//! inside the main binary (weixin, wecom, feishu).

use std::collections::HashMap;

use super::builtin::BuiltinChannel;
use carrier_types::tool::ToolProvider;

/// Factory function for a built-in channel adapter.
pub type ChannelFactory = Box<dyn Fn() -> Box<dyn BuiltinChannel> + Send + Sync>;

/// Factory function for a built-in tool provider.
pub type ToolFactory = Box<dyn Fn() -> Box<dyn ToolProvider> + Send + Sync>;

/// Per-plugin registry entry — all channels and tools for one built-in plugin.
struct RegistryEntry {
    channels: Vec<ChannelFactory>,
    tools: Vec<ToolFactory>,
}

/// Compile-time registry of built-in channels and tools, keyed by plugin name.
pub struct BuiltinPluginRegistry {
    entries: HashMap<String, RegistryEntry>,
}

impl BuiltinPluginRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Register a channel adapter factory for a given plugin.
    pub fn register_channel(
        &mut self,
        plugin_name: &str,
        factory: impl Fn() -> Box<dyn BuiltinChannel> + Send + Sync + 'static,
    ) {
        self.entries
            .entry(plugin_name.to_string())
            .or_insert_with(|| RegistryEntry {
                channels: Vec::new(),
                tools: Vec::new(),
            })
            .channels
            .push(Box::new(factory));
    }

    /// Register a tool provider factory for a given plugin.
    pub fn register_tool(
        &mut self,
        plugin_name: &str,
        factory: impl Fn() -> Box<dyn ToolProvider> + Send + Sync + 'static,
    ) {
        self.entries
            .entry(plugin_name.to_string())
            .or_insert_with(|| RegistryEntry {
                channels: Vec::new(),
                tools: Vec::new(),
            })
            .tools
            .push(Box::new(factory));
    }

    /// Look up a plugin entry by name.
    pub fn get_plugin(&self, plugin_name: &str) -> Option<BuiltinPluginFactories<'_>> {
        self.entries
            .get(plugin_name)
            .map(|entry| BuiltinPluginFactories {
                channels: &entry.channels,
                tools: &entry.tools,
            })
    }
}

impl Default for BuiltinPluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Borrowed view of factories for a single built-in plugin.
pub struct BuiltinPluginFactories<'a> {
    pub channels: &'a [ChannelFactory],
    pub tools: &'a [ToolFactory],
}
