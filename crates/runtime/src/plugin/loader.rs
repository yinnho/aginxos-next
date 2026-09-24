//! Plugin loader — dlopen-based shared library loading.
//!
//! Scans the plugins directory, loads shared libraries, binds C ABI symbols,
//! and initializes plugin instances.
//!
//! All data crossing the FFI boundary uses JSON C strings for ABI stability.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};

use super::instance::PluginInstance;
use libloading::Library;
use tokio::sync::mpsc;
use tracing::{info, warn};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::plugin::{
    BotConfig, ChannelDescriptor, FfiJsonCallback, PluginConfig, PluginToolDef, PLUGIN_ABI_VERSION,
};

/// Maximum size for tool execution result buffer.
const TOOL_RESULT_BUF_SIZE: u32 = 64 * 1024;

// ---------------------------------------------------------------------------
// C ABI function pointer types
// ---------------------------------------------------------------------------

type FnPluginName = unsafe extern "C" fn() -> *const c_char;
type FnPluginVersion = unsafe extern "C" fn() -> *const c_char;
type FnPluginAbiVersion = unsafe extern "C" fn() -> u32;
type FnPluginInit = unsafe extern "C" fn(
    config_json: *const c_char,
    message_cb: FfiJsonCallback,
    user_data: *mut std::os::raw::c_void,
) -> *mut std::os::raw::c_void;
type FnPluginStop = unsafe extern "C" fn(handle: *mut std::os::raw::c_void);
type FnPluginChannels = unsafe extern "C" fn(handle: *mut std::os::raw::c_void) -> *const c_char;
type FnPluginTools = unsafe extern "C" fn(handle: *mut std::os::raw::c_void) -> *const c_char;
type FnChannelStart = unsafe extern "C" fn(channel_handle: *mut std::os::raw::c_void) -> i32;
type FnChannelSend = unsafe extern "C" fn(
    channel_handle: *mut std::os::raw::c_void,
    message_json: *const c_char,
) -> i32;
type FnToolExecute = unsafe extern "C" fn(
    plugin_handle: *mut std::os::raw::c_void,
    tool_name: *const c_char,
    args_json: *const c_char,
    context_json: *const c_char,
    result_buf: *mut c_char,
    result_buf_len: u32,
) -> i32;

// ---------------------------------------------------------------------------
// Loaded plugin
// ---------------------------------------------------------------------------

/// A successfully loaded plugin with its library, symbols, and metadata.
pub struct LoadedPlugin {
    /// Plugin name.
    pub name: String,
    /// Plugin version.
    pub version: String,
    /// Plugin directory path.
    pub path: PathBuf,
    /// Opaque handle returned by `oc_plugin_init`.
    handle: std::cell::UnsafeCell<*mut std::os::raw::c_void>,
    /// user_data Box pointer — must be reclaimed on stop.
    user_data: std::cell::UnsafeCell<*mut std::os::raw::c_void>,
    /// Loaded channels with their opaque handles.
    pub channels: Vec<LoadedChannel>,
    /// Tool definitions provided by this plugin.
    pub tools: Vec<PluginToolDef>,
    /// The loaded shared library (kept alive for symbol resolution).
    _library: Library,
    // Cached function pointers
    fn_stop: Option<FnPluginStop>,
    fn_channel_start: Option<FnChannelStart>,
    fn_channel_send: Option<FnChannelSend>,
    fn_tool_execute: Option<FnToolExecute>,
}

/// A loaded channel from a plugin.
#[derive(Clone)]
pub struct LoadedChannel {
    /// Channel type identifier (e.g. "wecom").
    pub channel_type: String,
    /// Human-readable name.
    pub name: String,
    /// Bot ID this channel is bound to.
    pub bot_id: String,
    /// Opaque channel handle from the plugin.
    pub handle: *mut std::os::raw::c_void,
}

// SAFETY: LoadedChannel contains an opaque pointer from the plugin.
// It is only used to pass back to the plugin's own functions.
// Plugins MUST ensure their channel_start/channel_send implementations
// are thread-safe if called from multiple threads concurrently.
unsafe impl Send for LoadedChannel {}
unsafe impl Sync for LoadedChannel {}

// SAFETY: LoadedPlugin owns the Library and all handles are from that library.
// The UnsafeCell fields (handle, user_data) are only dereferenced during
// FFI calls. Plugins MUST ensure their C implementations are thread-safe
// when their functions are invoked concurrently. If a plugin is not
// thread-safe, callers should serialize access through an external Mutex.
unsafe impl Send for LoadedPlugin {}
unsafe impl Sync for LoadedPlugin {}

impl LoadedPlugin {
    /// Start a channel (begin receiving messages).
    pub fn start_channel(&self, channel: &LoadedChannel) -> CarrierResult<()> {
        if let Some(fn_start) = self.fn_channel_start {
            let ret = unsafe { fn_start(channel.handle) };
            if ret != 0 {
                return Err(CarrierError::Internal(format!(
                    "Channel {} start returned error code {}",
                    channel.channel_type, ret
                )));
            }
            Ok(())
        } else {
            Err(CarrierError::Internal(
                "Plugin does not export oc_channel_start".to_string(),
            ))
        }
    }

    /// Send a text message through a channel.
    pub fn channel_send(
        &self,
        channel: &LoadedChannel,
        bot_id: &str,
        user_id: &str,
        text: &str,
    ) -> CarrierResult<()> {
        if let Some(fn_send) = self.fn_channel_send {
            let msg = serde_json::json!({
                "bot_id": bot_id,
                "user_id": user_id,
                "text": text,
            });
            let c_msg = CString::new(
                serde_json::to_string(&msg)
                    .map_err(|e| CarrierError::Serialization(e.to_string()))?,
            )
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
            let ret = unsafe { fn_send(channel.handle, c_msg.as_ptr()) };
            if ret != 0 {
                return Err(CarrierError::Internal(format!(
                    "Channel {} send returned error code {}",
                    channel.channel_type, ret
                )));
            }
            Ok(())
        } else {
            Err(CarrierError::Internal(
                "Plugin does not export oc_channel_send".to_string(),
            ))
        }
    }

    /// Execute a tool.
    pub fn tool_execute(
        &self,
        tool_name: &str,
        args_json: &str,
        context_json: &str,
    ) -> CarrierResult<String> {
        if let Some(fn_exec) = self.fn_tool_execute {
            let c_tool =
                CString::new(tool_name).map_err(|e| CarrierError::Internal(e.to_string()))?;
            let c_args =
                CString::new(args_json).map_err(|e| CarrierError::Internal(e.to_string()))?;
            let c_ctx =
                CString::new(context_json).map_err(|e| CarrierError::Internal(e.to_string()))?;

            let mut buf = vec![0u8; TOOL_RESULT_BUF_SIZE as usize];
            let ret = unsafe {
                fn_exec(
                    *self.handle.get(),
                    c_tool.as_ptr(),
                    c_args.as_ptr(),
                    c_ctx.as_ptr(),
                    buf.as_mut_ptr() as *mut c_char,
                    TOOL_RESULT_BUF_SIZE,
                )
            };

            if ret < 0 {
                let error_msg = unsafe {
                    CStr::from_ptr(buf.as_ptr() as *const c_char)
                        .to_string_lossy()
                        .into_owned()
                };
                return Err(CarrierError::ToolExecution {
                    tool_id: tool_name.to_string(),
                    reason: if error_msg.is_empty() {
                        format!("Tool {} returned error code {}", tool_name, ret)
                    } else {
                        error_msg
                    },
                });
            }

            let len = ret as usize;
            if len > buf.len() {
                return Err(CarrierError::ToolExecution {
                    tool_id: tool_name.to_string(),
                    reason: format!(
                        "Tool {} returned {} bytes, exceeding buffer size {}",
                        tool_name,
                        len,
                        buf.len()
                    ),
                });
            }

            let result = String::from_utf8_lossy(&buf[..len]).into_owned();
            Ok(result)
        } else {
            Err(CarrierError::Internal(
                "Plugin does not export oc_plugin_tool_execute".to_string(),
            ))
        }
    }

    /// Stop the plugin and release resources.
    pub fn stop(&self) {
        if let Some(fn_stop) = self.fn_stop {
            unsafe {
                let handle = *self.handle.get();
                if !handle.is_null() {
                    fn_stop(handle);
                    *self.handle.get() = std::ptr::null_mut();
                }
            }
        }
        // Reclaim the user_data Box to prevent leak
        unsafe {
            let user_data = *self.user_data.get();
            if !user_data.is_null() {
                let _ = Box::from_raw(user_data as *mut mpsc::Sender<carrier_types::plugin::PluginMessage>);
                *self.user_data.get() = std::ptr::null_mut();
            }
        }
    }

    /// Check whether this plugin has been stopped.
    pub fn is_stopped(&self) -> bool {
        unsafe { (*self.handle.get()).is_null() }
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// PluginInstance trait implementation (adapts FFI calls to unified interface)
// ---------------------------------------------------------------------------

impl super::instance::PluginInstance for LoadedPlugin {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn channels(&self) -> &[LoadedChannel] {
        &self.channels
    }

    fn tools(&self) -> &[PluginToolDef] {
        &self.tools
    }

    fn start_channel(&self, channel: &LoadedChannel) -> CarrierResult<()> {
        self.start_channel(channel)
    }

    fn channel_send(
        &self,
        channel: &LoadedChannel,
        bot_id: &str,
        user_id: &str,
        text: &str,
    ) -> CarrierResult<()> {
        self.channel_send(channel, bot_id, user_id, text)
    }

    fn tool_execute(
        &self,
        tool_name: &str,
        args_json: &str,
        context_json: &str,
    ) -> CarrierResult<String> {
        self.tool_execute(tool_name, args_json, context_json)
    }

    fn stop(&self) {
        LoadedPlugin::stop(self);
    }

    fn is_stopped(&self) -> bool {
        self.is_stopped()
    }
}

// ---------------------------------------------------------------------------
// Plugin loader
// ---------------------------------------------------------------------------

/// Scans the plugins directory and loads all valid plugins.
pub struct PluginLoader;

impl PluginLoader {
    /// Load all plugins from the given directory.
    ///
    /// Each subdirectory should contain:
    /// - `plugin.toml` (metadata + configuration)
    /// - A shared library (`.so` / `.dylib` / `.dll`)
    pub fn load_all(
        plugins_dir: &Path,
        message_tx: mpsc::Sender<carrier_types::plugin::PluginMessage>,
    ) -> Vec<CarrierResult<LoadedPlugin>> {
        let mut results = Vec::new();

        let entries = match std::fs::read_dir(plugins_dir) {
            Ok(entries) => entries,
            Err(e) => {
                info!("Plugins directory not found or not readable: {}", e);
                return results;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let plugin_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            // Skip built-in plugins — they are loaded separately by load_builtin_plugins()
            let config_path = path.join("plugin.toml");
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(config) = toml::from_str::<PluginConfig>(&content) {
                    if config.meta.builtin {
                        continue;
                    }
                }
            }

            info!(plugin = %plugin_name, "Loading plugin");

            match Self::load_plugin(&path, &message_tx) {
                Ok(plugin) => {
                    info!(
                        plugin = %plugin.name,
                        version = %plugin.version,
                        channels = plugin.channels.len(),
                        tools = plugin.tools.len(),
                        "Plugin loaded successfully"
                    );
                    results.push(Ok(plugin));
                }
                Err(e) => {
                    warn!(plugin = %plugin_name, error = %e, "Failed to load plugin");
                    results.push(Err(e));
                }
            }
        }

        results
    }

    /// Load built-in plugins from the given directory.
    ///
    /// Scans subdirectories for `plugin.toml` with `builtin = true` and
    /// constructs `BuiltinPlugin` instances using the registry factories.
    pub fn load_builtin_plugins(
        plugins_dir: &Path,
        message_tx: mpsc::Sender<carrier_types::plugin::PluginMessage>,
        registry: &super::builtin_registry::BuiltinPluginRegistry,
    ) -> Vec<super::builtin::BuiltinPlugin> {
        let mut builtins = Vec::new();

        let entries = match std::fs::read_dir(plugins_dir) {
            Ok(entries) => entries,
            Err(_) => return builtins,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let plugin_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            let config_path = path.join("plugin.toml");
            if !config_path.exists() {
                continue;
            }

            let config_content = match std::fs::read_to_string(&config_path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let config: PluginConfig = match toml::from_str(&config_content) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if !config.meta.builtin {
                continue;
            }

            let Some(factories) = registry.get_plugin(plugin_name) else {
                warn!(plugin = %plugin_name, "Built-in plugin registered in plugin.toml but no factories in registry");
                continue;
            };

            let mut builtin = super::builtin::BuiltinPlugin::new(
                config.meta.name.clone(),
                config.meta.version.clone(),
                path.clone(),
            );

            for factory in factories.channels {
                let adapter = factory();
                if let Err(e) = builtin.register_channel(adapter, message_tx.clone()) {
                    warn!(plugin = %plugin_name, error = %e, "Failed to register built-in channel");
                }
            }

            for factory in factories.tools {
                let provider = factory();
                builtin.register_tool(provider);
            }

            info!(
                plugin = %plugin_name,
                channels = builtin.channels().len(),
                tools = builtin.tools().len(),
                "Built-in plugin loaded"
            );

            builtins.push(builtin);
        }

        builtins
    }

    fn load_plugin(
        plugin_dir: &Path,
        message_tx: &mpsc::Sender<carrier_types::plugin::PluginMessage>,
    ) -> CarrierResult<LoadedPlugin> {
        // 1. Read plugin.toml
        let config_path = plugin_dir.join("plugin.toml");
        let config_content = std::fs::read_to_string(&config_path).map_err(|e| {
            CarrierError::Internal(format!("Failed to read {}: {}", config_path.display(), e))
        })?;
        let mut config: PluginConfig = toml::from_str(&config_content)
            .map_err(|e| CarrierError::Config(format!("Invalid plugin.toml: {}", e)))?;

        // 2. Discover bot configs from <plugin-dir>/<uuid>/bot.toml
        let discovered_bots = Self::discover_bots(plugin_dir);
        config.bots = discovered_bots
            .into_iter()
            .map(|(bot_id, bot_config)| {
                let mut obj = serde_json::to_value(&bot_config)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                if let Some(map) = obj.as_object_mut() {
                    map.insert("_bot_id".to_string(), serde_json::Value::String(bot_id));
                }
                obj
            })
            .collect();

        // 3. Check ABI version
        if config.meta.abi_version != 0 && config.meta.abi_version != PLUGIN_ABI_VERSION {
            return Err(CarrierError::Internal(format!(
                "ABI version mismatch: plugin expects {}, host provides {}",
                config.meta.abi_version, PLUGIN_ABI_VERSION
            )));
        }

        // 4. Find shared library
        let lib_path = Self::find_shared_library(plugin_dir).ok_or_else(|| {
            CarrierError::Internal(format!(
                "No shared library found in {}",
                plugin_dir.display()
            ))
        })?;

        // 4. dlopen
        let library = unsafe { Library::new(&lib_path) }.map_err(|e| {
            CarrierError::Internal(format!("Failed to load {}: {}", lib_path.display(), e))
        })?;

        // 5. Load symbols
        let fn_name: FnPluginName = unsafe {
            *library
                .get(b"oc_plugin_name\0")
                .map_err(|e| CarrierError::Internal(format!("Missing oc_plugin_name: {}", e)))?
        };
        let fn_version: FnPluginVersion = unsafe {
            *library
                .get(b"oc_plugin_version\0")
                .map_err(|e| CarrierError::Internal(format!("Missing oc_plugin_version: {}", e)))?
        };
        let fn_abi: Option<FnPluginAbiVersion> =
            unsafe { library.get(b"oc_plugin_abi_version\0").ok().map(|s| *s) };
        let fn_init: FnPluginInit = unsafe {
            *library
                .get(b"oc_plugin_init\0")
                .map_err(|e| CarrierError::Internal(format!("Missing oc_plugin_init: {}", e)))?
        };
        let fn_stop: Option<FnPluginStop> =
            unsafe { library.get(b"oc_plugin_stop\0").ok().map(|s| *s) };
        let fn_channels: Option<FnPluginChannels> =
            unsafe { library.get(b"oc_plugin_channels\0").ok().map(|s| *s) };
        let fn_tools: Option<FnPluginTools> =
            unsafe { library.get(b"oc_plugin_tools\0").ok().map(|s| *s) };
        let fn_channel_start: Option<FnChannelStart> =
            unsafe { library.get(b"oc_channel_start\0").ok().map(|s| *s) };
        let fn_channel_send: Option<FnChannelSend> =
            unsafe { library.get(b"oc_channel_send\0").ok().map(|s| *s) };
        let fn_tool_execute: Option<FnToolExecute> =
            unsafe { library.get(b"oc_plugin_tool_execute\0").ok().map(|s| *s) };

        // 6. Verify ABI version via exported function
        if let Some(fn_abi) = fn_abi {
            let abi = unsafe { fn_abi() };
            if abi != PLUGIN_ABI_VERSION {
                return Err(CarrierError::Internal(format!(
                    "ABI version mismatch: plugin reports {}, host expects {}",
                    abi, PLUGIN_ABI_VERSION
                )));
            }
        }

        // 7. Read plugin name/version from library
        let name = unsafe { CStr::from_ptr(fn_name()) }
            .to_string_lossy()
            .into_owned();
        let version = unsafe { CStr::from_ptr(fn_version()) }
            .to_string_lossy()
            .into_owned();

        // 8. Initialize plugin
        let config_json_str = serde_json::to_string(&config)
            .map_err(|e| CarrierError::Serialization(format!("Config serialization: {}", e)))?;
        let c_config =
            CString::new(config_json_str).map_err(|e| CarrierError::Internal(e.to_string()))?;

        // Create a boxed sender that we pass as user_data
        let tx_box = Box::new(message_tx.clone());
        let user_data = Box::into_raw(tx_box) as *mut std::os::raw::c_void;

        let handle = unsafe { fn_init(c_config.as_ptr(), message_callback, user_data) };
        if handle.is_null() {
            return Err(CarrierError::Internal(
                "oc_plugin_init returned null handle".to_string(),
            ));
        }

        // 9. Load channels (JSON)
        let channels = if let Some(fn_ch) = fn_channels {
            let json_ptr = unsafe { fn_ch(handle) };
            Self::read_channels_json(json_ptr)
        } else {
            Vec::new()
        };

        // 10. Load tools (JSON)
        let tools = if let Some(fn_t) = fn_tools {
            let json_ptr = unsafe { fn_t(handle) };
            Self::read_tools_json(json_ptr)
        } else {
            Vec::new()
        };

        Ok(LoadedPlugin {
            name,
            version,
            path: plugin_dir.to_path_buf(),
            handle: std::cell::UnsafeCell::new(handle),
            user_data: std::cell::UnsafeCell::new(user_data),
            channels,
            tools,
            _library: library,
            fn_stop,
            fn_channel_start,
            fn_channel_send,
            fn_tool_execute,
        })
    }

    /// Discover bot configs from `<plugin-dir>/bot/<uuid>/bot.toml` subdirectories.
    pub fn discover_bots(plugin_dir: &Path) -> Vec<(String, BotConfig)> {
        let mut bots = Vec::new();
        let bot_root = plugin_dir.join("bot");
        let entries = match std::fs::read_dir(&bot_root) {
            Ok(e) => e,
            Err(_) => return bots,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let bot_toml = path.join("bot.toml");
            if !bot_toml.exists() {
                continue;
            }
            let bot_id = match path.file_name().and_then(|n| n.to_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            match std::fs::read_to_string(&bot_toml)
                .map_err(|e| e.to_string())
                .and_then(|s| toml::from_str::<BotConfig>(&s).map_err(|e| e.to_string()))
            {
                Ok(config) => {
                    info!(bot_id = %bot_id, name = %config.name, "Discovered bot");
                    bots.push((bot_id, config));
                }
                Err(e) => {
                    warn!(path = %bot_toml.display(), error = %e, "Failed to parse bot.toml");
                }
            }
        }
        bots
    }

    /// Find the shared library file in a plugin directory.
    fn find_shared_library(plugin_dir: &Path) -> Option<PathBuf> {
        let extensions = if cfg!(target_os = "macos") {
            &["dylib"][..]
        } else if cfg!(target_os = "linux") {
            &["so"][..]
        } else if cfg!(target_os = "windows") {
            &["dll"][..]
        } else {
            &["so", "dylib", "dll"][..]
        };

        if let Ok(entries) = std::fs::read_dir(plugin_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if extensions.contains(&ext) {
                        return Some(path);
                    }
                }
            }
        }

        None
    }

    /// Parse JSON channel descriptors returned by the plugin.
    fn read_channels_json(json_ptr: *const c_char) -> Vec<LoadedChannel> {
        if json_ptr.is_null() {
            return Vec::new();
        }
        let json_str = unsafe { CStr::from_ptr(json_ptr).to_string_lossy() };
        let descs: Vec<ChannelDescriptor> = match serde_json::from_str(&json_str) {
            Ok(d) => d,
            Err(e) => {
                warn!(error = %e, "Failed to parse channels JSON");
                return Vec::new();
            }
        };
        descs
            .into_iter()
            .enumerate()
            .map(|(i, desc)| LoadedChannel {
                channel_type: desc.channel_type,
                name: desc.name,
                bot_id: desc.bot_id,
                handle: (i + 1) as *mut std::os::raw::c_void,
            })
            .collect()
    }

    /// Parse JSON tool definitions returned by the plugin.
    fn read_tools_json(json_ptr: *const c_char) -> Vec<PluginToolDef> {
        if json_ptr.is_null() {
            return Vec::new();
        }
        let json_str = unsafe { CStr::from_ptr(json_ptr).to_string_lossy() };
        match serde_json::from_str(&json_str) {
            Ok(tools) => tools,
            Err(e) => {
                warn!(error = %e, "Failed to parse tools JSON");
                Vec::new()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Message callback (C ABI → Rust)
// ---------------------------------------------------------------------------

/// Global callback that plugins call to deliver inbound messages.
///
/// The `user_data` pointer is a `Box<mpsc::Sender<PluginMessage>>` that we
/// leaked in `load_plugin`. This function parses the JSON C string into a
/// `PluginMessage` and sends it through the channel.
unsafe extern "C" fn message_callback(user_data: *mut std::os::raw::c_void, json: *const c_char) {
    if user_data.is_null() || json.is_null() {
        return;
    }

    let tx = &*(user_data as *const mpsc::Sender<carrier_types::plugin::PluginMessage>);

    let json_str = CStr::from_ptr(json).to_string_lossy();
    let message: carrier_types::plugin::PluginMessage = match serde_json::from_str(&json_str) {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "Failed to parse plugin message JSON");
            return;
        }
    };

    // Non-blocking send — drop the message if the channel is full
    if let Err(e) = tx.try_send(message) {
        warn!(error = %e, "Plugin message channel full, dropping message");
    }
}
