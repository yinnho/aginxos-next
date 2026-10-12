//! weixin 频道腿（iLink 协议：扫码登录、会话轮询、出站发送）。
//!
//! crates/channels 的第一频道模块；会话真源=频道家
//! `<root>/senders/<key>/session.json`（token.rs 的频道根手术）。

// api/models：iLink 线上格式全量面。v0.1.5 起 CDN 富媒体出站闭环
// （getuploadurl → novac2c PUT/POST → sendmessage image_item/video_item）
// 已接 CLI send --image/--video/--file 与 ACP 媒体回信。
#[allow(dead_code)] // 部分诊断字段仍无调用方
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
///
/// Short idle pool timeout so a Wi-Fi AP change (new DNS / new source IP)
/// sheds half-open sockets quickly; callers also recreate the client after
/// consecutive network errors (see `BotSession::refresh_http`).
pub fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .pool_max_idle_per_host(2)
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .tcp_nodelay(true)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}
