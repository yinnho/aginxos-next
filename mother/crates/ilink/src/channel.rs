//! WeChat iLink session watcher — dynamic session discovery, polling, and send.

use crate::api;
use crate::crypto;
use crate::models::*;
use crate::token::WEIXIN_STATE;
use dashmap::DashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use carrier_types::plugin::{PluginContent, PluginMessage};
use uuid::Uuid;

use carrier_types::channel::Channel;
use carrier_types::error::{CarrierError, CarrierResult};

/// Drop redelivered getUpdates items (same message_id / seq / content window).
/// Without this, iLink can hand the same inbound item multiple times within
/// seconds and each spawn a full agent reply — users see "repeated answers".
const INBOUND_DEDUP_TTL: Duration = Duration::from_secs(120);
const INBOUND_DEDUP_MAX: usize = 10_000;

static INBOUND_DEDUP: std::sync::LazyLock<DashMap<String, Instant>> =
    std::sync::LazyLock::new(DashMap::new);

/// Build one or more dedup keys for an inbound iLink message.
/// Any previously claimed key → drop. Prefer platform ids; fall back to content.
fn inbound_dedup_keys(bot_id: &str, msg: &ILnkMessage, content: &PluginContent) -> Vec<String> {
    let mut keys = Vec::with_capacity(3);
    if let Some(id) = msg.message_id {
        if id != 0 {
            keys.push(format!("mid:{bot_id}:{id}"));
        }
    }
    if let Some(seq) = msg.seq {
        if seq != 0 {
            keys.push(format!("seq:{bot_id}:{seq}"));
        }
    }
    // Content fingerprint catches redeliveries when mid/seq are missing or
    // unstable. Include create_time_ms when present so legitimate repeats of
    // short texts ("好的") still work if sent as separate messages later.
    let from = msg.from_user_id.as_deref().unwrap_or("");
    let ts = msg.create_time_ms.unwrap_or(0);
    let snip = content_snippet(content);
    keys.push(format!("fp:{bot_id}:{from}:{ts}:{snip}"));
    keys
}

fn content_snippet(content: &PluginContent) -> String {
    match content {
        PluginContent::Text(t) => {
            let preview: String = t.chars().take(128).collect();
            format!("t{}:{preview}", t.len())
        }
        PluginContent::Image { caption, .. } => {
            format!("img:{}", caption.as_deref().unwrap_or(""))
        }
        PluginContent::File { filename, .. } => format!("file:{filename}"),
        PluginContent::Voice { .. } => "voice".into(),
        PluginContent::Video { caption, .. } => {
            format!("vid:{}", caption.as_deref().unwrap_or(""))
        }
        PluginContent::Location { lat, lon } => format!("loc:{lat}:{lon}"),
        PluginContent::Command { name, args } => format!("cmd:{name}:{}", args.len()),
    }
}

/// Returns true if this message should be processed (newly claimed).
/// Returns false if any key was already seen within the TTL window.
fn claim_inbound(keys: &[String]) -> bool {
    evict_inbound_dedup();
    for k in keys {
        if INBOUND_DEDUP.contains_key(k) {
            return false;
        }
    }
    let now = Instant::now();
    for k in keys {
        INBOUND_DEDUP.insert(k.clone(), now);
    }
    true
}

fn evict_inbound_dedup() {
    let now = Instant::now();
    INBOUND_DEDUP.retain(|_, at| now.duration_since(*at) < INBOUND_DEDUP_TTL);
    if INBOUND_DEDUP.len() > INBOUND_DEDUP_MAX {
        let overflow = INBOUND_DEDUP.len() - INBOUND_DEDUP_MAX;
        let stale: Vec<String> = INBOUND_DEDUP
            .iter()
            .take(overflow)
            .map(|e| e.key().clone())
            .collect();
        for k in stale {
            INBOUND_DEDUP.remove(&k);
        }
    }
}

/// Main polling loop (runs in a dedicated thread with its own runtime).
/// `session_key` is the user_id used as the DashMap key in WEIXIN_STATE.bots.
fn run_poll_loop(session_key: &str, sender: mpsc::Sender<PluginMessage>, shutdown: &AtomicBool) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            error!(
                session_key = session_key,
                "Failed to create tokio runtime: {e}"
            );
            return;
        }
    };

    rt.block_on(async move {
        poll_loop_inner(session_key, sender, shutdown).await;
    });
}

async fn poll_loop_inner(
    session_key: &str,
    sender: mpsc::Sender<PluginMessage>,
    shutdown: &AtomicBool,
) {
    info!(session_key = session_key, "Poll loop started");

    loop {
        if shutdown.load(Ordering::Relaxed) {
            info!(
                session_key = session_key,
                "Shutdown signal received, exiting poll loop"
            );
            return;
        }

        let (bot_token, baseurl, http, bot_id) = {
            let state = match WEIXIN_STATE.bots.get(session_key) {
                Some(s) => s,
                None => {
                    for _ in 0..10 {
                        if shutdown.load(Ordering::Relaxed) {
                            info!(session_key = session_key, "Shutdown during wait, exiting");
                            return;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                    continue;
                }
            };

            if !state.active.load(Ordering::Relaxed) || state.is_expired() {
                for _ in 0..10 {
                    if shutdown.load(Ordering::Relaxed) {
                        info!(
                            session_key = session_key,
                            "Shutdown during inactive wait, exiting"
                        );
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                continue;
            }

            (
                state.bot_token.clone(),
                state.baseurl.clone(),
                state.http.clone(),
                state.bot_id.clone(),
            )
        };

        let cursor = WEIXIN_STATE
            .bots
            .get(session_key)
            .map(|s| s.cursor.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .unwrap_or_default();

        match api::get_updates(&http, &bot_token, &baseurl, &cursor).await {
            Ok(resp) => {
                if resp.errcode == Some(SESSION_EXPIRED_ERRCODE)
                    || resp.ret == Some(SESSION_EXPIRED_ERRCODE)
                {
                    warn!(session_key = session_key, "Session expired, stopping poll");
                    if let Some(state) = WEIXIN_STATE.bots.get(session_key) {
                        state.active.store(false, Ordering::Relaxed);
                        state.expires_at.store(0, Ordering::Relaxed);
                    }
                    continue;
                }

                if let Some(ret) = resp.ret {
                    if ret != 0 {
                        warn!(
                            session_key = session_key,
                            ret, "getUpdates returned non-zero ret"
                        );
                        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                        continue;
                    }
                }

                if let Some(new_cursor) = &resp.get_updates_buf {
                    if !new_cursor.is_empty() {
                        if let Some(state) = WEIXIN_STATE.bots.get(session_key) {
                            *state.cursor.lock().unwrap_or_else(|e| e.into_inner()) =
                                new_cursor.clone();
                        }
                    }
                }

                if let Some(msgs) = resp.msgs {
                    // Log raw item types for debugging
                    for msg in &msgs {
                        if let Some(items) = &msg.item_list {
                            for item in items {
                                info!(
                                    session_key = session_key,
                                    item_type = item.type_.unwrap_or(-1i32 as u32),
                                    has_file = item.file_item.is_some(),
                                    has_image = item.image_item.is_some(),
                                    has_text = item.text_item.is_some(),
                                    "Raw WeChat item"
                                );
                            }
                        } else {
                            info!(
                                session_key = session_key,
                                "WeChat message with no item_list"
                            );
                        }
                    }
                    // Renew session expiry on every successful getUpdates
                    if let Some(state) = WEIXIN_STATE.bots.get(session_key) {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        state.expires_at.store(
                            now + SESSION_DURATION_SECS,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        state
                            .active
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                        WEIXIN_STATE.persist_if_due(&state);
                    }
                    for msg in msgs {
                        process_inbound_message(&bot_id, session_key, &msg, &sender, &http).await;
                    }
                } else {
                    // No messages but successful poll — still renew to keep session alive.
                    // persist_if_due (not save_session) so we don't write every 2s tick,
                    // but often enough that a restart sees a fresh expires_at instead of
                    // dropping the bot as expired.
                    if let Some(state) = WEIXIN_STATE.bots.get(session_key) {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        state.expires_at.store(
                            now + SESSION_DURATION_SECS,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        WEIXIN_STATE.persist_if_due(&state);
                    }
                }
            }
            Err(e) => {
                error!(session_key = session_key, "getUpdates error: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    }
}

/// Download a CDN media file, AES-decrypt it, and return raw bytes.
async fn download_cdn_raw(http: &reqwest::Client, media: &CDNMedia) -> CarrierResult<Vec<u8>> {
    let eqp = media
        .encrypt_query_param
        .as_deref()
        .ok_or_else(|| CarrierError::InvalidInput("No encrypt_query_param".to_string()))?;
    let aes_key_b64 = media
        .aes_key
        .as_deref()
        .ok_or_else(|| CarrierError::InvalidInput("No aes_key".to_string()))?;
    let key = crypto::parse_aes_key(aes_key_b64)
        .ok_or_else(|| CarrierError::Internal("Invalid AES key".to_string()))?;

    let url = crypto::cdn_download_url(eqp);
    crypto::cdn_download(http, &url, &key).await
}

async fn process_inbound_message(
    bot_id: &str,
    session_key: &str,
    msg: &ILnkMessage,
    sender: &mpsc::Sender<PluginMessage>,
    http: &reqwest::Client,
) {
    if msg.message_type != Some(MSG_TYPE_USER) {
        return;
    }
    if msg.message_state != Some(MSG_STATE_FINISH) {
        return;
    }

    let from_user_id = match &msg.from_user_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return,
    };

    if let Some(ctx_token) = &msg.context_token {
        if let Some(state) = WEIXIN_STATE.bots.get(session_key) {
            state.store_context_token(&from_user_id, ctx_token);
        }
    }

    // Build content from the first item in item_list
    let content = match msg.item_list.as_ref() {
        Some(items) if !items.is_empty() => {
            let item = &items[0];
            let item_type = item.type_.unwrap_or(0);
            match item_type {
                ITEM_TYPE_TEXT => {
                    let text = item
                        .text_item
                        .as_ref()
                        .and_then(|t| t.text.clone())
                        .unwrap_or_default();
                    PluginContent::Text(text)
                }
                ITEM_TYPE_IMAGE => {
                    // Download image bytes for local save. Do NOT embed base64 in
                    // `url` — vision will use a public view_url after save.
                    let image_data = match item.image_item.as_ref().and_then(|i| i.media.as_ref()) {
                        Some(media) => match download_cdn_raw(http, media).await {
                            Ok(bytes) => Some(bytes),
                            Err(e) => {
                                warn!(error = %e, "Failed to download WeChat image from CDN");
                                None
                            }
                        },
                        None => None,
                    };
                    PluginContent::Image {
                        url: String::new(),
                        caption: None,
                        data: image_data,
                    }
                }
                ITEM_TYPE_VOICE => {
                    // If voice has text transcription, use it directly
                    if let Some(text) = item.voice_item.as_ref().and_then(|v| v.text.clone()) {
                        if !text.is_empty() {
                            PluginContent::Text(text)
                        } else {
                            PluginContent::Voice {
                                url: String::new(),
                                duration_seconds: 0,
                            }
                        }
                    } else {
                        PluginContent::Voice {
                            url: String::new(),
                            duration_seconds: 0,
                        }
                    }
                }
                ITEM_TYPE_FILE => {
                    let file_item = item.file_item.as_ref();
                    let filename = file_item
                        .and_then(|f| f.file_name.clone())
                        .unwrap_or_default();
                    info!(filename = %filename, has_media = file_item.and_then(|f| f.media.as_ref()).is_some(), "WeChat file message received");
                    let data = match file_item.and_then(|f| f.media.as_ref()) {
                        Some(media) => match download_cdn_raw(http, media).await {
                            Ok(bytes) => {
                                info!(filename = %filename, size = bytes.len(), "WeChat file downloaded from CDN");
                                Some(bytes)
                            }
                            Err(e) => {
                                warn!(filename = %filename, error = %e, "Failed to download WeChat file from CDN");
                                None
                            }
                        },
                        None => None,
                    };
                    PluginContent::File {
                        url: String::new(),
                        filename,
                        data,
                    }
                }
                ITEM_TYPE_VIDEO => PluginContent::Video {
                    url: String::new(),
                    duration_seconds: item
                        .video_item
                        .as_ref()
                        .and_then(|v| v.play_length)
                        .map(|d| d as u32),
                    caption: None,
                },
                _ => {
                    warn!(
                        item_type,
                        "Unknown WeChat item type, treating as empty text"
                    );
                    PluginContent::Text(String::new())
                }
            }
        }
        _ => PluginContent::Text(String::new()),
    };

    // Dedup redelivered getUpdates items before they hit the bridge/agent.
    let keys = inbound_dedup_keys(bot_id, msg, &content);
    if !claim_inbound(&keys) {
        info!(
            bot_id = bot_id,
            from = %from_user_id,
            message_id = ?msg.message_id,
            seq = ?msg.seq,
            keys = ?keys,
            "Dropping duplicate WeChat inbound message"
        );
        return;
    }

    info!(
        bot_id = bot_id,
        from = %from_user_id,
        message_id = ?msg.message_id,
        seq = ?msg.seq,
        item_type = match msg.item_list.as_ref() {
            Some(items) if !items.is_empty() => items[0].type_.unwrap_or(0),
            _ => 0,
        },
        "Inbound WeChat message"
    );

    let plugin_msg = PluginMessage {
        channel_type: "weixin".to_string(),
        platform_message_id: msg.message_id.map(|id| id.to_string()).unwrap_or_default(),
        sender_id: from_user_id.clone(),
        sender_name: from_user_id.clone(),
        bot_id: bot_id.to_string(),
        content,
        timestamp_ms: msg.create_time_ms.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64
        }),
        is_group: msg.group_id.is_some(),
        thread_id: msg.group_id.clone(),
        metadata: Default::default(),
    };

    if let Err(e) = sender.try_send(plugin_msg) {
        warn!(error = %e, "Plugin message channel full, dropping message");
    }
}

// ---------------------------------------------------------------------------
// SessionWatcher — monitors for new bots added after plugin startup
// ---------------------------------------------------------------------------

/// Dynamic session watcher: reloads sessions from DB/disk each cycle
/// (picking up new `qr-login` bindings without a restart) and respawns
/// poll threads for inactive-but-valid bots.
pub struct SessionWatcher {
    shutdown: Arc<AtomicBool>,
    thread_handle: Option<std::thread::JoinHandle<()>>,
}

impl Default for SessionWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionWatcher {
    pub fn new() -> Self {
        Self {
            shutdown: Arc::new(AtomicBool::new(false)),
            thread_handle: None,
        }
    }
}

impl Channel for SessionWatcher {
    fn channel_type(&self) -> &str {
        "weixin"
    }

    fn supports_proactive_push(&self) -> bool {
        // iLink can push proactively when a context_token is available
        // (persisted in session.json). The send() method returns an error
        // when no context_token exists, and the caller falls back to buffering.
        true
    }

    fn name(&self) -> &str {
        "__watcher__"
    }

    fn bot_id(&self) -> &str {
        ""
    }

    fn start(&mut self, sender: mpsc::Sender<PluginMessage>) -> CarrierResult<()> {
        // Initial load + spawn all discovered bots
        WEIXIN_STATE.load_from_dir();
        spawn_all_bots(&sender);

        // Start respawn watcher (handles reconnection of inactive-but-valid bots)
        let shutdown = self.shutdown.clone();
        let handle = std::thread::Builder::new()
            .name("weixin-respawn-watcher".to_string())
            .spawn(move || {
                respawn_watcher_loop(sender, shutdown);
            })
            .map_err(|e| {
                CarrierError::Internal(format!("Failed to spawn respawn watcher thread: {e}"))
            })?;
        self.thread_handle = Some(handle);
        info!("WeChat SessionWatcher started");
        Ok(())
    }

    fn send(&self, bot_id: &str, user_id: &str, text: &str) -> CarrierResult<()> {
        let state = WEIXIN_STATE
            .get_session_for_send(bot_id, user_id)
            .ok_or_else(|| {
                CarrierError::InvalidInput(format!("No session for bot {bot_id}, user {user_id}"))
            })?;

        if state.is_expired() {
            return Err(CarrierError::Network(format!(
                "Token expired for bot {bot_id}"
            )));
        }

        // context_token is optional in the iLink protocol (verified
        // 2026-08-19): bare sends deliver when the sender account has a
        // relationship with the recipient. Keep a cached token when we have
        // one, but never block the send on its absence.
        let context_token = state.get_context_token(user_id);

        let client_id = format!("openclaw-weixin-{}", Uuid::new_v4().as_simple());
        let bot_token = state.bot_token.clone();
        let baseurl = state.baseurl.clone();
        let http = state.http.clone();
        let user_id = user_id.to_string();
        let text = text.to_string();

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = tx.send(Err(CarrierError::Internal(format!(
                        "Failed to create send runtime: {e}"
                    ))));
                    return;
                }
            };
            let result = rt.block_on(async {
                api::send_message_auto(
                    &http,
                    &bot_token,
                    &baseurl,
                    &user_id,
                    context_token.as_deref(),
                    &client_id,
                    &text,
                )
                .await
                .map_err(|e| CarrierError::Network(e.to_string()))
            });
            let _ = tx.send(result);
        });

        rx.recv()
            .map_err(|e| CarrierError::Internal(format!("Send thread disconnected: {e}")))?
    }

    fn deliver(
        &self,
        content: &carrier_types::content::ContentDescriptor,
        bot_id: &str,
        user_id: &str,
    ) -> CarrierResult<()> {
        let state = WEIXIN_STATE
            .get_session_for_send(bot_id, user_id)
            .ok_or_else(|| {
                CarrierError::InvalidInput(format!("No session for bot {bot_id}, user {user_id}"))
            })?;

        if state.is_expired() {
            return Err(CarrierError::Network(format!(
                "Token expired for bot {bot_id}"
            )));
        }

        // context_token optional — see send() for the verified protocol model.
        let context_token = state.get_context_token(user_id);

        // iLink supports video_url, image_url and text only. Pick the best
        // representation that has a public URL (link is not a native iLink card).
        let (send_kind, payload) = if let Some(v) = content.video.as_ref() {
            if let Some(url) = v.url.as_deref().filter(|u| !u.is_empty()) {
                ("video", url.to_string())
            } else {
                return Err(CarrierError::InvalidInput(
                    "iLink video requires a public URL".into(),
                ));
            }
        } else if let Some(img) = content.image.as_ref() {
            if let Some(url) = img.url.as_deref().filter(|u| !u.is_empty()) {
                ("image", url.to_string())
            } else {
                return Err(CarrierError::InvalidInput(
                    "iLink image requires a public URL".into(),
                ));
            }
        } else if let Some(text) = content.as_text() {
            return self.send(bot_id, user_id, &text);
        } else {
            return Err(CarrierError::InvalidInput(
                "iLink: content has no video URL, image URL, or text representation".into(),
            ));
        };

        let client_id = format!("openclaw-weixin-{}", Uuid::new_v4().as_simple());
        let bot_token = state.bot_token.clone();
        let baseurl = state.baseurl.clone();
        let http = state.http.clone();
        let user_id = user_id.to_string();

        carrier_types::channel::block_on_detached(async move {
            match send_kind {
                "video" => api::send_video_auto(
                    &http,
                    &bot_token,
                    &baseurl,
                    &user_id,
                    context_token.as_deref(),
                    &client_id,
                    &payload,
                )
                .await
                .map_err(|e| CarrierError::Network(e.to_string())),
                "image" => api::send_image_auto(
                    &http,
                    &bot_token,
                    &baseurl,
                    &user_id,
                    context_token.as_deref(),
                    &client_id,
                    &payload,
                )
                .await
                .map_err(|e| CarrierError::Network(e.to_string())),
                _ => unreachable!(),
            }
        })
    }

    fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(()) => info!("SessionWatcher thread joined cleanly"),
                Err(e) => error!("SessionWatcher thread panicked: {e:?}"),
            }
        }
        info!("SessionWatcher stopped");
    }

}

/// Spawn poll threads for all bots that are loaded but not yet active.
fn spawn_all_bots(sender: &mpsc::Sender<PluginMessage>) {
    for entry in WEIXIN_STATE.bots.iter() {
        let user_id = entry.key().clone();
        let state = entry.value();
        if state.active.load(Ordering::Relaxed) || state.is_expired() {
            continue;
        }
        state.active.store(true, Ordering::Relaxed);
        let s = sender.clone();
        let poll_key = user_id.clone();
        info!(user_id = %user_id, "Spawning poll thread for bot");
        if let Err(e) = std::thread::Builder::new()
            .name(format!("weixin-dyn-{user_id}"))
            .spawn(move || {
                let shutdown = Arc::new(AtomicBool::new(false));
                run_poll_loop(&poll_key, s, &shutdown);
            })
        {
            error!(user_id = %user_id, "Failed to spawn poll thread: {e}");
        }
    }
}

/// Spawn a specific bot by user_id (if loaded and not yet active).
/// Background loop that respawns poll threads for inactive-but-valid bots.
/// This handles the case where a bot's poll loop exits (e.g. session expired
/// in iLink) but the session file has been refreshed with a new token.
fn respawn_watcher_loop(sender: mpsc::Sender<PluginMessage>, shutdown: Arc<AtomicBool>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            error!("Failed to create respawn watcher tokio runtime: {e}");
            return;
        }
    };

    rt.block_on(async move {
        loop {
            if shutdown.load(Ordering::Relaxed) {
                info!("Respawn watcher shutdown signal received");
                return;
            }

            // Pick up sessions that appeared since the last cycle — one-shot
            // `qr-login` writes session.json in its own process; DB rows can
            // also change. Without this, a new binding is invisible until a
            // daemon restart.
            WEIXIN_STATE.load_new_from_dir();

            for entry in WEIXIN_STATE.bots.iter() {
                let user_id = entry.key().clone();
                let state = entry.value();
                // Respawn: inactive but not expired (session refreshed with new token)
                if !state.active.load(Ordering::Relaxed) && !state.is_expired() {
                    state.active.store(true, Ordering::Relaxed);
                    let s = sender.clone();
                    let poll_key = user_id.clone();
                    info!(user_id = %user_id, "Respawning poll thread for inactive bot");
                    if let Err(e) = std::thread::Builder::new()
                        .name(format!("weixin-dyn-{user_id}"))
                        .spawn(move || {
                            let shutdown = Arc::new(AtomicBool::new(false));
                            run_poll_loop(&poll_key, s, &shutdown);
                        })
                    {
                        error!(user_id = %user_id, "Failed to respawn poll thread: {e}");
                    }
                }
            }

            for _ in 0..10 {
                if shutdown.load(Ordering::Relaxed) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    });
}

#[cfg(test)]
mod dedup_tests {
    use super::*;

    #[test]
    fn claim_inbound_drops_second_same_keys() {
        // Isolate keys with unique bot id so parallel tests don't clash.
        let bot = format!("test-bot-{}", uuid::Uuid::new_v4());
        let keys = vec![format!("mid:{bot}:42"), format!("fp:{bot}:u:1:t3:abc")];
        assert!(claim_inbound(&keys), "first claim should succeed");
        assert!(
            !claim_inbound(&keys),
            "second claim should be dropped as duplicate"
        );
    }

    #[test]
    fn content_snippet_text_includes_len() {
        let c = PluginContent::Text("你好世界".into());
        let s = content_snippet(&c);
        assert!(s.starts_with("t"), "{s}");
        assert!(s.contains('4') || s.contains("你好"), "{s}");
    }
}
