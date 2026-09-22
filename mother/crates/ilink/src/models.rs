//! iLink Bot API protocol types.
//!
//! Mirrors the WeChat iLink JSON structures used by `ilinkai.weixin.qq.com`.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// iLink API base URL.
pub const ILINK_API_BASE: &str = "https://ilinkai.weixin.qq.com";

/// Protocol channel version string sent in every request.
pub const CHANNEL_VERSION: &str = "1.0.2";

/// CDN base URL for media downloads (Phase 2).
pub const CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";

/// Bot type parameter for QR code login.
pub const BOT_TYPE: &str = "3";

/// Session duration in seconds (24 hours).
pub const SESSION_DURATION_SECS: i64 = 24 * 3600;

/// iLink session-expired error code (from getUpdates response).
pub const SESSION_EXPIRED_ERRCODE: i64 = -14;

/// Long-poll timeout in milliseconds.
pub const LONG_POLL_TIMEOUT_MS: u64 = 35_000;

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// Message type: user message (inbound from WeChat user).
pub const MSG_TYPE_USER: u32 = 1;
/// Message type: bot message (outbound from our bot).
pub const MSG_TYPE_BOT: u32 = 2;

/// Message state: finished (complete message).
pub const MSG_STATE_FINISH: u32 = 2;

/// Item type: text.
pub const ITEM_TYPE_TEXT: u32 = 1;
/// Item type: image.
pub const ITEM_TYPE_IMAGE: u32 = 2;
/// Item type: voice.
pub const ITEM_TYPE_VOICE: u32 = 3;
/// Item type: file attachment.
pub const ITEM_TYPE_FILE: u32 = 4;
/// Item type: video.
pub const ITEM_TYPE_VIDEO: u32 = 5;

// ---------------------------------------------------------------------------
// Common
// ---------------------------------------------------------------------------

/// Request metadata attached to every iLink API request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaseInfo {
    pub channel_version: String,
}

impl Default for BaseInfo {
    fn default() -> Self {
        Self {
            channel_version: CHANNEL_VERSION.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// QR login
// ---------------------------------------------------------------------------

/// Response from `GET /ilink/bot/get_bot_qrcode?bot_type=3`.
#[derive(Debug, Clone, Deserialize)]
pub struct QrCodeResponse {
    pub qrcode: String,
    pub qrcode_img_content: String,
}

/// Response from `GET /ilink/bot/get_qrcode_status?qrcode=xxx`.
#[derive(Debug, Clone, Deserialize)]
pub struct QrCodeStatusResponse {
    pub status: String,
    /// Present when status == "confirmed".
    pub bot_token: Option<String>,
    /// Present when status == "confirmed".
    pub baseurl: Option<String>,
    /// The bot's iLink ID (e.g. "xxx@im.bot").
    pub ilink_bot_id: Option<String>,
    /// The WeChat user ID who scanned the QR code.
    pub ilink_user_id: Option<String>,
    /// Present when status == "scaned_but_redirect" (IDC redirect host).
    pub redirect_host: Option<String>,
}

// ---------------------------------------------------------------------------
// GetUpdates (long-poll receive)
// ---------------------------------------------------------------------------

/// Request body for `POST /ilink/bot/getupdates`.
#[derive(Debug, Clone, Serialize)]
pub struct GetUpdatesRequest {
    pub get_updates_buf: String,
    pub base_info: BaseInfo,
}

/// Response from `POST /ilink/bot/getupdates`.
#[derive(Debug, Clone, Deserialize)]
pub struct GetUpdatesResponse {
    pub ret: Option<i64>,
    pub errcode: Option<i64>,
    pub errmsg: Option<String>,
    pub msgs: Option<Vec<ILnkMessage>>,
    pub get_updates_buf: Option<String>,
    pub longpolling_timeout_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// Message types
// ---------------------------------------------------------------------------

/// A single iLink message (inbound from getUpdates).
#[derive(Debug, Clone, Deserialize)]
pub struct ILnkMessage {
    pub seq: Option<u64>,
    pub message_id: Option<u64>,
    pub from_user_id: Option<String>,
    pub to_user_id: Option<String>,
    pub client_id: Option<String>,
    pub create_time_ms: Option<u64>,
    pub message_type: Option<u32>,
    pub message_state: Option<u32>,
    pub context_token: Option<String>,
    pub item_list: Option<Vec<ILnkItem>>,
    pub group_id: Option<String>,
}

/// A single item within a message's `item_list`.
#[derive(Debug, Clone, Deserialize)]
pub struct ILnkItem {
    #[serde(rename = "type")]
    pub type_: Option<u32>,
    pub text_item: Option<TextItem>,
    pub image_item: Option<ImageItem>,
    pub voice_item: Option<VoiceItem>,
    pub file_item: Option<FileItem>,
    pub video_item: Option<VideoItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextItem {
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageItem {
    pub media: Option<CDNMedia>,
    pub mid_size: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceItem {
    pub media: Option<CDNMedia>,
    pub encode_type: Option<u32>,
    pub playtime: Option<u64>,
    /// Voice-to-text transcription.
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileItem {
    pub media: Option<CDNMedia>,
    pub file_name: Option<String>,
    pub len: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VideoItem {
    pub media: Option<CDNMedia>,
    pub video_size: Option<u64>,
    pub play_length: Option<u64>,
}

/// CDN media reference with AES encryption parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct CDNMedia {
    pub encrypt_query_param: Option<String>,
    pub aes_key: Option<String>,
    pub encrypt_type: Option<u32>,
}

// ---------------------------------------------------------------------------
// SendMessage (outbound)
// ---------------------------------------------------------------------------

/// Request body for `POST /ilink/bot/sendmessage`.
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageRequest {
    pub msg: SendMessageMsg,
    pub base_info: BaseInfo,
}

/// The `msg` field within a SendMessageRequest.
#[derive(Debug, Clone, Serialize)]
pub struct SendMessageMsg {
    pub from_user_id: String,
    pub to_user_id: String,
    pub client_id: String,
    pub message_type: u32,
    pub message_state: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_list: Option<Vec<SendItem>>,
}

/// A single item in an outbound message.
#[derive(Debug, Clone, Serialize)]
pub struct SendItem {
    #[serde(rename = "type")]
    pub type_: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_item: Option<SendTextItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_item: Option<SendImageItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_item: Option<SendVideoItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SendTextItem {
    pub text: String,
}

/// Outbound image item — just a URL to the image.
#[derive(Debug, Clone, Serialize)]
pub struct SendImageItem {
    pub image_url: String,
}

/// Outbound video item — just a URL to the video.
#[derive(Debug, Clone, Serialize)]
pub struct SendVideoItem {
    pub video_url: String,
}

// ---------------------------------------------------------------------------
// Token persistence
// ---------------------------------------------------------------------------

/// Serialized form of a bot's iLink credentials, stored on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotTokenFile {
    #[serde(default)]
    pub channel: String, // "weixin"
    #[serde(default)]
    pub sender_key: String, // "openid"
    pub bot_id: String,
    pub bot_token: String,
    pub baseurl: String,
    pub ilink_bot_id: String,
    pub user_id: Option<String>,
    /// Unix timestamp (seconds) when this token expires.
    pub expires_at: i64,
    /// Optional agent name to bind this channel to.
    pub bind_agent: Option<String>,
    /// Per-user context_tokens (user_id → token), persisted so iLink
    /// pushes survive service restarts. Updated on save_session.
    #[serde(default)]
    pub context_tokens: std::collections::HashMap<String, String>,
}
