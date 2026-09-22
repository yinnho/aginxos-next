//! 绑定即路由——sender_id → agent 名的内存路由表。
//!
//! 路由不再有独立的磁盘真源（老 config.json 已退役）：weixin 会话的
//! `bind_agent`（DB / workspaces 下 session.json）与 webhook 的 config.toml
//! 在启动时种入这里。一个绑定对应一个分身；多分身管理走 webui/桌面端，
//! 不在聊天协议里（命名流/@名字切换//list 已随 opencarrier 多分身模型
//! 一起删除）。

use dashmap::DashMap;
use tracing::info;

/// In-memory sender routing: sender_id → agent_name.
///
/// Thread-safe via DashMap. Seeded at boot (wiring); the bridge reads it
/// to resolve inbound messages.
pub struct SenderRouter {
    routes: DashMap<String, String>,
}

impl Default for SenderRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl SenderRouter {
    pub fn new() -> Self {
        Self {
            routes: DashMap::new(),
        }
    }

    /// Resolve which agent handles a sender. No route → None（未绑定的
    /// sender 消息会被丢弃并告警，不静默指派）。
    pub fn resolve(&self, sender_id: &str) -> Option<String> {
        self.routes.get(sender_id).map(|r| r.value().clone())
    }

    /// Set the route for a sender (boot seeding / runtime rebind).
    pub fn set_route(&self, sender_id: &str, agent_id: &str) {
        self.routes
            .insert(sender_id.to_string(), agent_id.to_string());
        info!(sender = %sender_id, agent = %agent_id, "Sender route set");
    }
}
