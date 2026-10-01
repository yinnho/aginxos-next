//! weixin 频道腿（iLink 协议：扫码登录、会话轮询、出站发送）。
//!
//! crates/channels 的第一频道模块；会话真源=频道家
//! `<root>/senders/<key>/session.json`（token.rs 的频道根手术）。

// api/models 是 iLink 线上格式的全量面：富媒体出站（send_image/video
// 族）与诊断字段本轮母体退役后暂无调用方，属协议保留面不是腐坏。
#[allow(dead_code)]
pub mod api;
pub mod auth;
pub mod channel;
pub mod crypto;
#[allow(dead_code)] // 线上格式 struct：serde 反序列化驱动，字段不逐一消费
pub mod models;
pub mod token;

pub use channel::SessionWatcher;

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
