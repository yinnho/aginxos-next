//! WeChat iLink built-in channel adapter and tools.
//!
//! Provides:
//! - `SessionWatcher` — dynamic session discovery and polling
//! - `WeixinQrLoginTool` — trigger QR code login
//! - `WeixinSendMessageTool` — send messages to WeChat users
//! - `WeixinSendImageTool` — send images to WeChat users
//! - `WeixinStatusTool` — show bot status

pub mod api;
pub mod auth;
pub mod channel;
pub mod crypto;
pub mod models;
pub mod token;
pub mod tools;

pub use channel::SessionWatcher;
pub use tools::{
    WeixinQrLoginTool, WeixinSendImageTool, WeixinSendMessageTool, WeixinSendVideoTool,
    WeixinStatusTool,
};

/// Build an HTTP client that bypasses ambient/system proxies and forces
/// HTTP/1.1. The iLink API must be reached directly; reqwest's default
/// `Client::new()` inherits the OS proxy (e.g. a SOCKS proxy on macOS) and
/// hangs against iLink. Use this everywhere an iLink client is constructed.
pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
