//! Plugin system — channel adapters and tools.
//!
//! Channels bridge external messaging platforms to the kernel.
//! Tools provide platform API capabilities that agents can call.

pub mod admin_store;
pub mod bridge;
pub mod builtin;
pub mod builtin_registry;
pub mod instance;
pub mod loader;
pub mod router;
pub mod tool_dispatch;

pub use builtin::{BuiltinChannel, BuiltinPlugin};
pub use builtin_registry::BuiltinPluginRegistry;
pub use instance::PluginInstance;
pub use loader::LoadedPlugin;

// ChannelManager (lives at crate root, re-exported here for transition)
pub use crate::channel_manager::ChannelManager;
