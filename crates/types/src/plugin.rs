//! Plugin system types — owned by carrier-types.
//!
//! These types define the data that flows between channels, the bridge, and the kernel.
//! Previously re-exported from carrier-plugin-sdk, now owned directly.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Plugin metadata
// ---------------------------------------------------------------------------

/// Current plugin ABI version. Bumped on breaking changes.
pub const PLUGIN_ABI_VERSION: u32 = 3;

/// Plugin metadata from `plugin.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub min_host_version: String,
    #[serde(default)]
    pub abi_version: u32,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub builtin: bool,
}

/// Full plugin configuration loaded from `plugin.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(rename = "plugin")]
    pub meta: PluginMeta,
    #[serde(default)]
    pub bots: Vec<serde_json::Value>,
    #[serde(default)]
    pub extra: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Channel types
// ---------------------------------------------------------------------------

/// Descriptor for a channel provided by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelDescriptor {
    pub channel_type: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub bot_id: String,
}

/// Content types that can be exchanged with a channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginContent {
    Text(String),
    Image {
        url: String,
        caption: Option<String>,
        #[serde(default)]
        data: Option<Vec<u8>>,
    },
    File {
        url: String,
        filename: String,
        #[serde(default)]
        data: Option<Vec<u8>>,
    },
    Voice {
        url: String,
        duration_seconds: u32,
    },
    Video {
        url: String,
        duration_seconds: Option<u32>,
        caption: Option<String>,
    },
    Location {
        lat: f64,
        lon: f64,
    },
    Command {
        name: String,
        args: Vec<String>,
    },
}

impl PluginContent {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            PluginContent::Text(t) => Some(t),
            _ => None,
        }
    }
}

/// A unified message from any channel plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMessage {
    pub channel_type: String,
    pub platform_message_id: String,
    pub sender_id: String,
    #[serde(default)]
    pub sender_name: String,
    #[serde(default)]
    pub bot_id: String,
    pub content: PluginContent,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub is_group: bool,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Tool types
// ---------------------------------------------------------------------------

/// A tool definition provided by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
}

/// Context provided when executing a plugin tool.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginToolContext {
    #[serde(default)]
    pub bot_id: String,
    #[serde(default)]
    pub sender_id: String,
    #[serde(default)]
    pub agent_id: String,
    #[serde(default)]
    pub channel_type: String,
}

// ---------------------------------------------------------------------------
// Per-bot configuration (stored in <plugin-dir>/<bot-uuid>/bot.toml)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotConfig {
    pub name: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub bind_agent: Option<String>,
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

// ---------------------------------------------------------------------------
// Plugin status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatus {
    pub name: String,
    pub version: String,
    pub loaded: bool,
    pub channels: Vec<String>,
    pub tools: Vec<String>,
    pub bot_count: usize,
    #[serde(default)]
    pub last_error: Option<String>,
}

// ---------------------------------------------------------------------------
// FFI callback type (kept for loader.rs compatibility)
// ---------------------------------------------------------------------------

/// Callback function type: plugin sends a JSON-encoded PluginMessage to the host.
pub type FfiJsonCallback =
    unsafe extern "C" fn(user_data: *mut std::os::raw::c_void, json: *const std::os::raw::c_char);
