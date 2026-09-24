//! Built-in plugin — directly compiled channel adapters and tools (no FFI).
//!
//! Used for core channels (weixin, wecom, feishu) that ship with the binary.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tokio::sync::mpsc;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::plugin::{PluginMessage, PluginToolContext, PluginToolDef};
use carrier_types::tool::ToolProvider;

use super::instance::PluginInstance;
use super::loader::LoadedChannel;

/// Trait for built-in channel adapters.
///
/// Similar to `carrier_types::channel::Channel` but uses the host's
/// native `mpsc::Sender<PluginMessage>` instead of an FFI callback.
/// Prefer using `Channel` from `carrier_types::channel` for new code.
pub trait BuiltinChannel: Send + Sync {
    fn channel_type(&self) -> &str;
    fn name(&self) -> &str;
    fn bot_id(&self) -> &str;
    fn start(&mut self, sender: mpsc::Sender<PluginMessage>) -> CarrierResult<()>;
    fn send(&self, bot_id: &str, user_id: &str, text: &str) -> CarrierResult<()>;
    fn stop(&mut self);

    /// Whether this channel supports proactive push (sending without an inbound
    /// context). Channels that require a context_token / response_url should
    /// return false; cron and other server-initiated notifications must be
    /// buffered for these channels until the user sends an inbound message.
    fn supports_proactive_push(&self) -> bool {
        false
    }
}

/// A built-in plugin that directly holds Rust trait objects.
pub struct BuiltinPlugin {
    name: String,
    version: String,
    path: PathBuf,
    channels: Vec<LoadedChannel>,
    tools: Vec<PluginToolDef>,
    channel_adapters: Mutex<HashMap<String, Box<dyn BuiltinChannel>>>,
    tool_providers: Mutex<HashMap<String, Box<dyn ToolProvider>>>,
}

impl BuiltinPlugin {
    pub fn new(name: String, version: String, path: PathBuf) -> Self {
        Self {
            name,
            version,
            path,
            channels: Vec::new(),
            tools: Vec::new(),
            channel_adapters: Mutex::new(HashMap::new()),
            tool_providers: Mutex::new(HashMap::new()),
        }
    }

    pub fn register_channel(
        &mut self,
        mut adapter: Box<dyn BuiltinChannel>,
        sender: mpsc::Sender<PluginMessage>,
    ) -> CarrierResult<()> {
        let channel_type = adapter.channel_type().to_string();
        let name = adapter.name().to_string();
        let bot_id = adapter.bot_id().to_string();

        adapter.start(sender)?;

        self.channels.push(LoadedChannel {
            channel_type: channel_type.clone(),
            name,
            bot_id,
            handle: std::ptr::null_mut(),
        });

        self.channel_adapters
            .lock()
            .unwrap()
            .insert(channel_type, adapter);
        Ok(())
    }

    pub fn register_tool(&mut self, provider: Box<dyn ToolProvider>) {
        let def = provider.definition();
        self.tools.push(PluginToolDef {
            name: def.name.clone(),
            description: def.description.clone(),
            parameters_json: def.parameters_json.clone(),
        });
        self.tool_providers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(def.name, provider);
    }
}

impl PluginInstance for BuiltinPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn channels(&self) -> &[LoadedChannel] {
        &self.channels
    }

    fn tools(&self) -> &[PluginToolDef] {
        &self.tools
    }

    fn start_channel(&self, _channel: &LoadedChannel) -> CarrierResult<()> {
        Ok(())
    }

    fn channel_send(
        &self,
        channel: &LoadedChannel,
        bot_id: &str,
        user_id: &str,
        text: &str,
    ) -> CarrierResult<()> {
        let adapters = self
            .channel_adapters
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(adapter) = adapters.get(&channel.channel_type) {
            adapter.send(bot_id, user_id, text)
        } else {
            Err(CarrierError::Config(format!(
                "Built-in channel adapter '{}' not found",
                channel.channel_type
            )))
        }
    }

    fn tool_execute(
        &self,
        tool_name: &str,
        args_json: &str,
        context_json: &str,
    ) -> CarrierResult<String> {
        let providers = self
            .tool_providers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let provider = providers.get(tool_name).ok_or_else(|| {
            CarrierError::Config(format!("Built-in tool '{}' not found", tool_name))
        })?;

        let args: serde_json::Value = serde_json::from_str(args_json)
            .map_err(|e| CarrierError::Serialization(format!("Args deserialization: {}", e)))?;
        let ctx: PluginToolContext = serde_json::from_str(context_json)
            .map_err(|e| CarrierError::Serialization(format!("Context deserialization: {}", e)))?;

        provider
            .execute(&args, &ctx)
            .map_err(|e| CarrierError::ToolExecution {
                tool_id: tool_name.to_string(),
                reason: e.to_string(),
            })
    }

    fn stop(&self) {
        let mut adapters = self
            .channel_adapters
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (name, adapter) in adapters.iter_mut() {
            adapter.stop();
            tracing::info!(channel = %name, "Built-in channel stopped");
        }
    }

    fn is_stopped(&self) -> bool {
        self.channel_adapters
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }
}
