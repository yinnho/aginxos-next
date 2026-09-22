//! Channel manager — lifecycle management for channel adapters.
//!
//! Replaces the old `PluginManager` for channel operations. Each channel
//! (feishu, wecom, weixin, dingtalk) is registered as a `Box<dyn Channel>`
//! and managed directly — no FFI, no plugin abstraction layer.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{error, info};
use carrier_types::channel::Channel;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::plugin::{PluginMessage, PluginStatus};
use carrier_types::tool::ToolDefinition;

use crate::kernel_handle::KernelHandle;
use crate::plugin::bridge::{ChannelDeliverFn, ChannelSendFn, PluginBridgeManager};
use crate::plugin::router::SenderRouter;
use crate::plugin::tool_dispatch::PluginToolDispatcher;

/// Manages the lifecycle of all registered channel adapters.
pub struct ChannelManager {
    /// Registered channels keyed by a unique name (e.g. "feishu", "wecom_app_kf").
    /// Wrapped in Arc<std::sync::Mutex> so the bridge's sync send_response can access them.
    channels: Arc<std::sync::Mutex<HashMap<String, Box<dyn Channel>>>>,
    /// Bridge message sender (inbound messages from channels).
    message_tx: mpsc::Sender<PluginMessage>,
    /// Bridge message receiver (moved to bridge on start).
    message_rx: Option<mpsc::Receiver<PluginMessage>>,
    /// Kernel handle for bridge routing.
    kernel: Arc<dyn KernelHandle>,
    /// Sender-based router (route_key → agent_id), set before start().
    sender_router: Option<Arc<SenderRouter>>,
    /// Cron delivery store (last-channel tracking + pending notifications).
    cron_delivery: Option<Arc<carrier_memory::CronDeliveryStore>>,
    /// Notify route store (DB-backed notification routing).
    notify_store: Option<Arc<carrier_memory::NotifyRouteStore>>,
    /// Tool dispatcher for plugin-style tools (weixin tools, etc.).
    tool_dispatcher: Arc<PluginToolDispatcher>,
}

impl ChannelManager {
    /// Create a new channel manager.
    pub fn new(kernel: Arc<dyn KernelHandle>) -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            channels: Arc::new(std::sync::Mutex::new(HashMap::new())),
            message_tx: tx,
            message_rx: Some(rx),
            kernel,
            sender_router: None,
            cron_delivery: None,
            notify_store: None,
            tool_dispatcher: Arc::new(PluginToolDispatcher::new()),
        }
    }

    /// Set the sender-based router (must be called before start()).
    pub fn set_sender_router(&mut self, router: Arc<SenderRouter>) {
        self.sender_router = Some(router);
    }

    /// Set the cron delivery store (enables last-channel tracking + buffer drain).
    pub fn set_cron_delivery(&mut self, store: Arc<carrier_memory::CronDeliveryStore>) {
        self.cron_delivery = Some(store);
    }

    /// Set the notify route store (enables DB-backed notification routing).
    pub fn set_notify_store(&mut self, store: Arc<carrier_memory::NotifyRouteStore>) {
        self.notify_store = Some(store);
    }

    /// Register a channel adapter under a unique name.
    pub fn register(&mut self, name: &str, channel: Box<dyn Channel>) {
        self.channels
            .lock()
            .unwrap()
            .insert(name.to_string(), channel);
    }

    /// Get a reference to the tool dispatcher (for registering tool providers).
    pub fn tool_dispatcher(&self) -> Arc<PluginToolDispatcher> {
        self.tool_dispatcher.clone()
    }

    /// Start all registered channels and the bridge.
    pub async fn start(&mut self) {
        // Start channel adapters
        {
            let mut channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
            for (name, channel) in channels.iter_mut() {
                match channel.start(self.message_tx.clone()) {
                    Ok(()) => {
                        info!(
                            channel = %name,
                            channel_type = %channel.channel_type(),
                            bot_id = %channel.bot_id(),
                            "Channel started"
                        );
                    }
                    Err(e) => {
                        error!(
                            channel = %name,
                            channel_type = %channel.channel_type(),
                            error = %e,
                            "Failed to start channel"
                        );
                    }
                }
            }
        }

        // Build bridge
        let mut bridge = PluginBridgeManager::new(self.kernel.clone());

        if let Some(ref router) = self.sender_router {
            bridge.set_sender_router(router.clone());
        }

        if let Some(ref store) = self.cron_delivery {
            bridge.set_cron_delivery(store.clone());
        }

        // Set up channel send function for bridge to deliver responses
        let channels_for_send = self.channels.clone();
        let send_fn: ChannelSendFn = Arc::new(move |channel_type, bot_id, user_id, text| {
            let channels = channels_for_send.lock().unwrap_or_else(|e| e.into_inner());
            for channel in channels.values() {
                if channel.channel_type() == channel_type {
                    return channel.send(bot_id, user_id, text);
                }
            }
            Err(CarrierError::InvalidInput(format!(
                "Channel not found for type: {}, bot: {}",
                channel_type, bot_id
            )))
        });
        bridge.set_channel_send_fn(send_fn);

        // Set up channel deliver function for bridge to deliver rich content
        // (`[DELIVER:key]` markers). Dispatches by channel_type to Channel::deliver,
        // which each channel overrides to pick its best-supported form.
        let channels_for_deliver = self.channels.clone();
        let deliver_fn: ChannelDeliverFn =
            Arc::new(move |channel_type, bot_id, user_id, content| {
                let channels = channels_for_deliver
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                for channel in channels.values() {
                    if channel.channel_type() == channel_type {
                        return channel.deliver(content, bot_id, user_id);
                    }
                }
                Err(CarrierError::InvalidInput(format!(
                    "Channel not found for type: {}, bot: {}",
                    channel_type, bot_id
                )))
            });
        bridge.set_channel_deliver_fn(deliver_fn);

        // Load notify routes — enables [NOTIFY:type]content[/NOTIFY] markers → cross-channel push.
        // Try DB first, fall back to notify_routes.json.
        {
            let mut loaded_from = "none";
            let routes: std::collections::HashMap<String, crate::plugin::bridge::NotifyTarget> =
                if let Some(ref store) = self.notify_store {
                    match store.load_all() {
                        Ok(rows) if !rows.is_empty() => {
                            loaded_from = "database";
                            rows.into_iter()
                                .map(|r| {
                                    (
                                        r.name,
                                        crate::plugin::bridge::NotifyTarget {
                                            channel: r.channel,
                                            bot_id: r.bot_id,
                                            user_id: r.user_id,
                                            prefix: r.prefix,
                                            recipients: r.recipients,
                                        },
                                    )
                                })
                                .collect()
                        }
                        _ => std::collections::HashMap::new(),
                    }
                } else {
                    std::collections::HashMap::new()
                };

            let routes = if !routes.is_empty() {
                routes
            } else {
                let path = carrier_types::config::home_dir().join("notify_routes.json");
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(r) = serde_json::from_str::<
                        std::collections::HashMap<String, crate::plugin::bridge::NotifyTarget>,
                    >(&content)
                    {
                        if !r.is_empty() {
                            loaded_from = "json";
                        }
                        r
                    } else {
                        std::collections::HashMap::new()
                    }
                } else {
                    std::collections::HashMap::new()
                }
            };

            if !routes.is_empty() {
                info!(
                    route_count = routes.len(),
                    from = loaded_from,
                    "Loaded notify routes"
                );
                bridge.set_notify_routes(Arc::new(routes));
            }
        }

        // Start bridge in a background task
        if let Some(rx) = self.message_rx.take() {
            tokio::spawn(async move {
                bridge.run(rx).await;
            });
        }

        let count = self
            .channels
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        info!(channels = count, "Channel manager started");
    }

    /// Get a clone of the bridge's inbound message sender.
    ///
    /// Used by webhook-based channels (e.g. weixin-oa) to inject PluginMessages
    /// received via HTTP callback directly into the bridge routing pipeline.
    pub fn bridge_sender(&self) -> mpsc::Sender<PluginMessage> {
        self.message_tx.clone()
    }

    /// Send a text message through a channel by channel type and bot ID.
    pub fn channel_send(
        &self,
        channel_type: &str,
        bot_id: &str,
        user_id: &str,
        text: &str,
    ) -> CarrierResult<()> {
        let channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        for channel in channels.values() {
            if channel.channel_type() == channel_type {
                return channel.send(bot_id, user_id, text);
            }
        }
        Err(CarrierError::InvalidInput(format!(
            "Channel not found for type: {}, bot: {}",
            channel_type, bot_id
        )))
    }

    /// Build a closure that can send messages through this manager's channels.
    /// Used by the kernel for cron delivery.
    pub fn make_channel_send_fn(&self) -> crate::plugin::bridge::ChannelSendFn {
        let channels = self.channels.clone();
        Arc::new(move |channel_type, bot_id, user_id, text| {
            let channels = channels.lock().unwrap_or_else(|e| e.into_inner());
            for channel in channels.values() {
                if channel.channel_type() == channel_type {
                    return channel.send(bot_id, user_id, text);
                }
            }
            Err(CarrierError::InvalidInput(format!(
                "Channel not found for type: {}, bot: {}",
                channel_type, bot_id
            )))
        })
    }

    /// Build a closure that delivers rich content through this manager's
    /// channels. Used by the kernel for script/cron delivery without an agent.
    pub fn make_channel_deliver_fn(&self) -> ChannelDeliverFn {
        let channels = self.channels.clone();
        Arc::new(move |channel_type, bot_id, user_id, content| {
            let channels = channels.lock().unwrap_or_else(|e| e.into_inner());
            for channel in channels.values() {
                if channel.channel_type() == channel_type {
                    return channel.deliver(content, bot_id, user_id);
                }
            }
            Err(CarrierError::InvalidInput(format!(
                "Channel not found for type: {}, bot: {}",
                channel_type, bot_id
            )))
        })
    }

    /// Build a closure that probes whether a channel type supports proactive
    /// push (sending without an inbound context).
    pub fn make_supports_proactive_fn(&self) -> Arc<dyn Fn(&str) -> bool + Send + Sync> {
        let channels = self.channels.clone();
        Arc::new(move |channel_type| {
            let channels = channels.lock().unwrap_or_else(|e| e.into_inner());
            for channel in channels.values() {
                if channel.channel_type() == channel_type {
                    return channel.supports_proactive_push();
                }
            }
            false
        })
    }

    /// Send a text message by searching all channels for a matching bot_id.
    /// This matches the old PluginManager behavior where bot_id was the primary key.
    pub fn channel_send_by_bot(
        &self,
        bot_id: &str,
        user_id: &str,
        text: &str,
    ) -> CarrierResult<()> {
        let channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        for channel in channels.values() {
            match channel.send(bot_id, user_id, text) {
                Ok(()) => return Ok(()),
                Err(_) => continue,
            }
        }
        Err(CarrierError::InvalidInput(format!(
            "No channel found for bot: {}",
            bot_id
        )))
    }

    /// Set a sender route (route_key → agent_id).
    pub fn set_sender_route(&self, route_key: &str, agent_id: &str) {
        if let Some(ref router) = self.sender_router {
            router.set_route(route_key, agent_id);
        }
    }

    /// Get all plugin tool definitions.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tool_dispatcher.definitions()
    }

    /// Get status of all registered channels.
    pub fn status(&self) -> Vec<PluginStatus> {
        let channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        channels
            .iter()
            .map(|(name, channel)| PluginStatus {
                name: name.clone(),
                version: String::new(),
                loaded: true,
                channels: vec![channel.channel_type().to_string()],
                tools: Vec::new(),
                bot_count: 0,
                last_error: None,
            })
            .collect()
    }

    /// Stop all channels and release resources.
    pub fn stop_all(&self) {
        let mut channels = self.channels.lock().unwrap_or_else(|e| e.into_inner());
        for (name, channel) in channels.iter_mut() {
            info!(channel = %name, "Stopping channel");
            channel.stop();
        }
    }
}

impl Drop for ChannelManager {
    fn drop(&mut self) {
        self.stop_all();
    }
}
