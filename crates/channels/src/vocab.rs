//! 频道词表——channels 自己家的小词典（#69 改形）。
//!
//! 原是母体侧 carrier-types 的 channel/plugin/error 三页；母体退役后
//! 频道体系不再借引擎的词：入站消息、出站发送、错误语言都在这里。
//! 旧版的 `deliver`（富媒体投递）/`supports_proactive_push`（引擎主动
//! 推送探针）随母体面一起死——频道只认「收文本、发文本」。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;
use thiserror::Error;

// ---------------------------------------------------------------------------
// 错误
// ---------------------------------------------------------------------------

/// 频道体系顶层错误。
#[derive(Error, Debug)]
pub enum CarrierError {
    #[error("Capability denied: {0}")]
    CapabilityDenied(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

pub type CarrierResult<T> = Result<T, CarrierError>;

// ---------------------------------------------------------------------------
// 入站消息
// ---------------------------------------------------------------------------

/// 频道交换的内容类型。
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

/// 任一频道来的统一入站消息。
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
// 频道腿接口
// ---------------------------------------------------------------------------

/// 一条频道协议腿（weixin 现在；telegram/email 将来）：
/// 收消息（start → mpsc）+ 发文本（send）+ 停（stop）。
pub trait Channel: Send + Sync {
    /// 频道类型标识（与 channel.toml 的 type 词表同源，如 "weixin"）。
    fn channel_type(&self) -> &str;

    /// 人读频道名。
    fn name(&self) -> &str;

    /// 本腿所属 bot 标识。
    fn bot_id(&self) -> &str;

    /// 开始收消息：入站经 `sender` 交给桥。
    fn start(&mut self, sender: mpsc::Sender<PluginMessage>) -> CarrierResult<()>;

    /// 发一条文本消息。
    fn send(&self, bot_id: &str, user_id: &str, text: &str) -> CarrierResult<()>;

    /// 停腿、放资源。
    fn stop(&mut self);
}

// ---------------------------------------------------------------------------
// 家
// ---------------------------------------------------------------------------

/// AGINX_HOME（设备=/home；host 开发可指到别处）。
pub fn home_dir() -> std::path::PathBuf {
    if let Ok(home) = std::env::var("AGINX_HOME") {
        return std::path::PathBuf::from(home);
    }
    std::path::PathBuf::from("/home")
}
