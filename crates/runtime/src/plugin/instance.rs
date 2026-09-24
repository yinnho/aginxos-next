//! Plugin instance trait — unifies built-in and external FFI plugins.

use std::path::Path;

use carrier_types::error::CarrierResult;
use carrier_types::plugin::PluginToolDef;

use super::loader::LoadedChannel;

/// Runtime plugin abstraction — unified interface for built-in and external FFI plugins.
pub trait PluginInstance: Send + Sync {
    /// Plugin name.
    fn name(&self) -> &str;
    /// Plugin version.
    fn version(&self) -> &str;
    /// Plugin directory path.
    fn path(&self) -> &Path;
    /// Loaded channels.
    fn channels(&self) -> &[LoadedChannel];
    /// Tool definitions.
    fn tools(&self) -> &[PluginToolDef];

    /// Start a channel (begin receiving messages).
    fn start_channel(&self, channel: &LoadedChannel) -> CarrierResult<()>;
    /// Send a text message through a channel.
    fn channel_send(
        &self,
        channel: &LoadedChannel,
        bot_id: &str,
        user_id: &str,
        text: &str,
    ) -> CarrierResult<()>;
    /// Execute a plugin tool.
    fn tool_execute(
        &self,
        tool_name: &str,
        args_json: &str,
        context_json: &str,
    ) -> CarrierResult<String>;
    /// Stop the plugin and release resources.
    fn stop(&self);
    /// Check whether this plugin has been stopped.
    fn is_stopped(&self) -> bool;
}
