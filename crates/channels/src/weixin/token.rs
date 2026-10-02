//! Token storage and management for the WeChat iLink channel leg.
//!
//! Manages per-bot bot_tokens (24h expiry) and per-user context_tokens.
//! 会话真源=频道家 `<root>/senders/<key>/session.json`（0600，tmp+rename）。

use dashmap::DashMap;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

use super::models::*;

/// Throttle for persisting renewed sessions. The poll loop renews `expires_at`
/// in memory on every successful (often empty) getUpdates — we must not hit the
/// DB/JSON on each ~2s tick. Persist at most this often, so on restart load sees
/// an `expires_at` at most this stale (well inside the 24h window) instead of a
/// value from the last inbound message hours/days ago.
const SESSION_SAVE_INTERVAL_SECS: i64 = 1800; // 30 min

// ---------------------------------------------------------------------------
// Per-bot runtime state
// ---------------------------------------------------------------------------

/// Runtime state for a single iLink bot session (one scanned WeChat account).
pub struct BotSession {
    /// Bot ID (used as routing key).
    pub bot_id: String,
    /// iLink bot_token (from QR scan, valid 24h).
    pub bot_token: String,
    /// iLink base URL (from QR scan, usually same as ILINK_API_BASE).
    pub baseurl: String,
    /// The bot's iLink ID (e.g. "xxx@im.bot").
    pub ilink_bot_id: String,
    /// The WeChat user ID who scanned the QR code.
    pub user_id: Option<String>,
    /// Unix timestamp (seconds) when this token expires.
    pub expires_at: AtomicI64,
    /// Shared HTTP client.
    pub http: Client,
    /// Per-user context_token cache: user_id → context_token.
    context_tokens: Mutex<HashMap<String, String>>,
    /// get_updates_buf cursor for long-polling.
    pub cursor: Mutex<String>,
    /// Whether the polling loop is active.
    pub active: AtomicBool,
    /// Wall-clock (secs) of the last `save_session`. Used to throttle
    /// persistence during empty-poll renewal so we don't write every tick.
    pub last_saved: AtomicI64,
    /// Optional agent name to bind this channel to.
    pub bind_agent: Option<String>,
}

impl BotSession {
    /// Check if this bot's token has expired.
    pub fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now >= self.expires_at.load(Ordering::Relaxed)
    }

    /// Seconds remaining until expiry.
    pub fn remaining_secs(&self) -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        (self.expires_at.load(Ordering::Relaxed) - now).max(0)
    }

    /// Store a context_token for a user (from an inbound message).
    pub fn store_context_token(&self, user_id: &str, token: &str) {
        self.context_tokens
            .lock()
            .unwrap()
            .insert(user_id.to_string(), token.to_string());
    }

    /// Get the cached context_token for a user.
    pub fn get_context_token(&self, user_id: &str) -> Option<String> {
        self.context_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(user_id)
            .cloned()
    }
}

// ---------------------------------------------------------------------------
// Global state manager
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 频道会话根（#68 ②b channels/ 体系）
// ---------------------------------------------------------------------------

/// 频道模式会话根（如 `/home/channels/weixin`）。扫描与落盘全走
/// `<root>/senders/<user_key>/session.json` —— **绑定是记录里的字段**
/// （bind_agent），换绑=改字段不搬家。进程级一次定型：daemon 与 CLI
/// 入口先设根再动会话；没设根=进程拿错了入口（save 会如实 warn 跳过，
/// 扫描返回空）。
static SESSIONS_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Set the channel-mode sessions root (e.g. `/home/channels/weixin`).
/// Idempotent one-shot: first call wins, later calls are ignored (CLI and
/// daemon share the entry point; tests use the path-taking helpers below).
pub fn set_sessions_root(root: impl Into<std::path::PathBuf>) {
    let _ = SESSIONS_ROOT.set(root.into());
}

/// Channel-mode session file path for an account (`<root>/senders/<key>/`).
fn channel_session_path(root: &std::path::Path, user_key: &str) -> std::path::PathBuf {
    root.join("senders").join(user_key).join("session.json")
}

/// 扫频道布局 `<root>/senders/*/session.json`（仅 weixin）。绑定以
/// **文件字段**为真源——频道目录下无分身目录可依，字段即唯一事实。
fn scan_channel_sessions(root: &std::path::Path) -> Vec<BotTokenFile> {
    let mut tfs = Vec::new();
    let Ok(senders) = std::fs::read_dir(root.join("senders")) else {
        return tfs;
    };
    for sender_entry in senders.flatten() {
        let path = sender_entry.path().join("session.json");
        if !path.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
            warn!(path = %path.display(), "Failed to parse session file as JSON");
            continue;
        };
        if json.get("channel").and_then(|v| v.as_str()) != Some("weixin") {
            continue;
        }
        match serde_json::from_value::<BotTokenFile>(json) {
            Ok(tf) => tfs.push(tf),
            Err(e) => warn!(path = %path.display(), "Failed to parse weixin session: {e}"),
        }
    }
    tfs
}

/// Write a session file: 0600, pretty JSON, tmp + rename so a reader never
/// sees a half-written session.
fn write_session_file(path: &std::path::Path, tf: &BotTokenFile) {
    let Ok(json) = serde_json::to_string_pretty(tf) else {
        warn!("Failed to serialize bot token");
        return;
    };
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            warn!(dir = %parent.display(), "Failed to create sender directory: {e}");
            return;
        }
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &json) {
        warn!(path = %tmp.display(), "Failed to write session file: {e}");
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        warn!(path = %path.display(), "Failed to move session file into place: {e}");
    }
}

/// 扫频道布局 `<root>/senders/*/session.json`（仅 weixin）。绑定以
/// **文件字段**为真源——频道目录下无分身目录可依，字段即唯一事实。
/// 没设根=入口拿错，返回空（save 侧同律 warn——不悄悄写别处）。
pub fn scan_json_token_files() -> Vec<BotTokenFile> {
    match SESSIONS_ROOT.get() {
        Some(root) => scan_channel_sessions(root),
        None => {
            warn!("scan_json_token_files: sessions root not set — nothing to scan");
            Vec::new()
        }
    }
}

/// Global state manager for all iLink bots.
pub struct WeixinState {
    /// Per-bot state keyed by user_id (stable unique identifier for WeChat).
    pub bots: DashMap<String, BotSession>,
}

impl WeixinState {
    fn new() -> Self {
        Self {
            bots: DashMap::new(),
        }
    }

    /// Load persisted tokens from the channel home (`<root>/senders/`).
    pub fn load_from_dir(&self) {
        let tfs = scan_json_token_files();
        if !tfs.is_empty() {
            self.load_from_bot_token_files(tfs);
        }
    }

    /// Shared logic to load BotTokenFiles into the in-memory bot cache.
    fn load_from_bot_token_files(&self, tfs: Vec<BotTokenFile>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let mut count = 0;
        for tf in tfs {
            let user_id = match &tf.user_id {
                Some(uid) if !uid.is_empty() => uid.clone(),
                _ => continue,
            };
            if now >= tf.expires_at {
                continue;
            }
            let persisted_ctx = tf.context_tokens.clone();
            count += 1;
            let state = BotSession {
                bot_id: tf.bot_id.clone(),
                bot_token: tf.bot_token,
                baseurl: tf.baseurl,
                ilink_bot_id: tf.ilink_bot_id,
                user_id: Some(user_id.clone()),
                expires_at: AtomicI64::new(tf.expires_at),
                http: super::build_http_client(),
                context_tokens: Mutex::new(persisted_ctx),
                cursor: Mutex::new(String::new()),
                active: AtomicBool::new(false),
                last_saved: AtomicI64::new(tf.expires_at - SESSION_DURATION_SECS),
                bind_agent: tf.bind_agent,
            };
            self.bots.insert(user_id, state);
        }
        if count > 0 {
            info!(count, "Loaded iLink bot sessions");
        }
    }

    /// Register a new bot from a successful QR scan.
    pub fn register_from_qr(
        &self,
        bot_id: &str,
        bot_token: &str,
        baseurl: &str,
        ilink_bot_id: &str,
        user_id: Option<&str>,
        bind_agent: Option<&str>,
    ) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let state = BotSession {
            bot_id: bot_id.to_string(),
            bot_token: bot_token.to_string(),
            baseurl: baseurl.to_string(),
            ilink_bot_id: ilink_bot_id.to_string(),
            user_id: user_id.map(|s| s.to_string()),
            expires_at: AtomicI64::new(now + SESSION_DURATION_SECS),
            http: super::build_http_client(),
            context_tokens: Mutex::new(HashMap::new()),
            cursor: Mutex::new(String::new()),
            active: AtomicBool::new(true),
            last_saved: AtomicI64::new(now),
            bind_agent: bind_agent.map(|s| s.to_string()),
        };

        // Persist to disk
        self.save_session(&state);

        // Insert/update in-memory, keyed by user_id
        let key = user_id.unwrap_or(bot_id);
        if let Some(mut existing) = self.bots.get_mut(key) {
            // Preserve cursor from existing session if possible
            let old_cursor = existing
                .cursor
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            *state.cursor.lock().unwrap_or_else(|e| e.into_inner()) = old_cursor;
            *existing = state;
        } else {
            self.bots.insert(key.to_string(), state);
        }

        info!(user_id = ?user_id, bot_id = bot_id, "Registered iLink bot from QR scan");
    }

    /// Save a bot session's state to the channel home
    /// (`<root>/senders/<user_key>/session.json`).
    pub fn save_session(&self, state: &BotSession) {
        let merged_ctx = state
            .context_tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let tf = BotTokenFile {
            channel: "weixin".to_string(),
            sender_key: "openid".to_string(),
            bot_id: state.bot_id.clone(),
            bot_token: state.bot_token.clone(),
            baseurl: state.baseurl.clone(),
            ilink_bot_id: state.ilink_bot_id.clone(),
            user_id: state.user_id.clone(),
            expires_at: state.expires_at.load(Ordering::Relaxed),
            bind_agent: state.bind_agent.clone(),
            context_tokens: merged_ctx,
        };

        let Some(root) = SESSIONS_ROOT.get() else {
            warn!(bot_id = %state.bot_id, "save_session: sessions root not set — session stays in-memory only");
            return;
        };
        let user_key = state.user_id.as_deref().unwrap_or(&state.bot_id);
        let path = channel_session_path(root, user_key);
        write_session_file(&path, &tf);
    }

    /// Persist `state` only if it was last saved more than
    /// `SESSION_SAVE_INTERVAL_SECS` ago. Called from the poll loop's renewal
    /// branches (both message and empty-poll) so a long-idle bot's on-disk
    /// `expires_at` stays current without writing on every ~2s tick.
    ///
    /// Without this, idle bots' disk `expires_at` stays frozen at the time of
    /// their last *inbound message*; after 24h without a message a restart
    /// loads them as expired and drops them — the "iLink 老是断开 on restart" bug.
    pub fn persist_if_due(&self, state: &BotSession) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if now - state.last_saved.load(Ordering::Relaxed) >= SESSION_SAVE_INTERVAL_SECS {
            self.save_session(state);
            state.last_saved.store(now, Ordering::Relaxed);
        }
    }

    /// Find a bot session for sending a message to `user_id`.
    ///
    /// Routing model (verified in production 2026-08-19): iLink delivery is
    /// account-to-account and clone-agnostic — any clone can push to any
    /// recipient, provided the SENDING account has a relationship with the
    /// recipient. context_tokens doubles as the relationship ledger.
    /// Three paths, in order:
    /// 1. **Direct**: `user_id` IS a logged-in account (the 号池). Deliver
    ///    via its own session — effectively self-chat. Works for every pool
    ///    account (admin alerts ride this path); bare send, no token needed.
    /// 2. **Relationship scan**: the peer chatted with exactly one of our
    ///    accounts before — its token lives in THAT account's session. iLink
    ///    peer ids are per-account namespaces, so at most one holder; no
    ///    ambiguity.
    /// 3. **Legacy bot_id fallback**: caller-asserted explicit route. CAVEAT:
    ///    sends to peers with NO relationship upstream (never chatted, not
    ///    in the pool) are silently dropped by iLink — HTTP still returns a
    ///    message_id, so success here is best-effort, never a receipt.
    pub fn get_session_for_send(
        &self,
        _bot_id: &str,
        user_id: &str,
    ) -> Option<dashmap::mapref::one::Ref<'_, String, BotSession>> {
        // 1. Direct lookup by user_id — the target is itself a scanned account.
        if let Some(state) = self.bots.get(user_id) {
            return Some(state);
        }
        // 2. The one session whose account actually holds this peer's token.
        //    Mutex pairs only with read refs elsewhere (store/get/save), so
        //    locking inside the shard-read iteration cannot deadlock.
        let holder = self
            .bots
            .iter()
            .find(|entry| {
                entry
                    .value()
                    .context_tokens
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .contains_key(user_id)
            })
            .map(|entry| entry.key().clone());
        if let Some(key) = holder {
            return self.bots.get(&key);
        }
        // 3. Legacy fallback: first session with a matching bot_id.
        let found_key = self
            .bots
            .iter()
            .find(|entry| entry.value().bot_id == _bot_id)
            .map(|entry| entry.key().clone())?;
        self.bots.get(&found_key)
    }

    /// Load new bots — merge DB rows and workspace session.json files into
    /// the in-memory bot cache. Used by the respawn watcher each cycle to
    /// pick up QR-scanned bots without a restart: one-shot `qr-login` writes
    /// session.json (no DB in that process), the daemon's own saves go to DB.
    pub fn load_new_from_dir(&self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Channel home session.json: refresh existing (rebind / token renewal)
        // + insert new
        for tf in scan_json_token_files() {
            let Some(sender_id) = tf.user_id.clone().filter(|s| !s.is_empty()) else {
                continue;
            };
            if let Some(mut existing) = self.bots.get_mut(&sender_id) {
                if existing.bot_token != tf.bot_token || existing.bind_agent != tf.bind_agent {
                    info!(sender_id = %sender_id, "Refreshing iLink bot from updated session file (new bot_token)");
                    existing.bot_token = tf.bot_token.clone();
                    existing.baseurl = tf.baseurl;
                    existing.ilink_bot_id = tf.ilink_bot_id;
                    existing.user_id = tf.user_id;
                    existing.expires_at.store(tf.expires_at, Ordering::Relaxed);
                    existing.active.store(true, Ordering::Relaxed);
                    existing.bind_agent = tf.bind_agent.clone();
                    self.save_session(&existing);
                }
                {
                    let mut ctx = existing
                        .context_tokens
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    for (uid, tok) in &tf.context_tokens {
                        ctx.entry(uid.clone()).or_insert_with(|| tok.clone());
                    }
                }
                continue;
            }
            if now >= tf.expires_at {
                continue;
            }
            info!(sender_id = %sender_id, "Dynamic watcher loaded new iLink bot");
            let persisted_ctx = tf.context_tokens.clone();
            let state = BotSession {
                bot_id: tf.bot_id.clone(),
                bot_token: tf.bot_token,
                baseurl: tf.baseurl,
                ilink_bot_id: tf.ilink_bot_id,
                user_id: Some(sender_id.clone()),
                expires_at: AtomicI64::new(tf.expires_at),
                http: super::build_http_client(),
                context_tokens: Mutex::new(persisted_ctx),
                cursor: Mutex::new(String::new()),
                active: AtomicBool::new(false),
                last_saved: AtomicI64::new(tf.expires_at - SESSION_DURATION_SECS),
                bind_agent: tf.bind_agent,
            };
            self.bots.insert(sender_id, state);
        }
    }

    /// 该发信人是不是绑定号本人（任一扫码会话的 user_id）。
    ///
    /// v0.1.2 入站政策真源（用户 10-01 裁决「ilink 绑定的号才能发
    /// 信息」）：iLink 是伴生协议——bot 会话由扫码号本人发起，消息的
    /// group_id 是会话标记不是群聊判定（首条真考实抓 from=绑定号本人
    /// 且 group_id 非空）；只放行绑定号本人，陌生人交 daemon 弃。
    pub fn is_bound_sender(&self, sender_id: &str) -> bool {
        self.bots.contains_key(sender_id)
    }

    /// Get status of all bots for the API.
    pub fn status_list(&self) -> Vec<serde_json::Value> {
        self.bots
            .iter()
            .map(|entry| {
                let state = entry.value();
                serde_json::json!({
                    "bot_id": state.bot_id,
                    "ilink_bot_id": state.ilink_bot_id,
                    "user_id": state.user_id,
                    "expires_at": state.expires_at.load(Ordering::Relaxed),
                    "remaining_secs": state.remaining_secs(),
                    "expired": state.is_expired(),
                    "active": state.active.load(Ordering::Relaxed),
                    "bind_agent": state.bind_agent,
                })
            })
            .collect()
    }
}

/// Global singleton for iLink state management.
pub static WEIXIN_STATE: std::sync::LazyLock<WeixinState> =
    std::sync::LazyLock::new(WeixinState::new);

#[cfg(test)]
mod tests {
    use super::*;

    /// Register a fresh scanner account (never expired: register_from_qr
    /// stamps now + SESSION_DURATION_SECS). Tests never set the sessions
    /// root, so save_session stays in-memory — no writes anywhere.
    fn register(state: &WeixinState, scanner: &str) {
        state.register_from_qr(
            "default",
            "tok",
            "https://example.invalid",
            "bot@im.bot",
            Some(scanner),
            Some(&format!("agent-{scanner}")),
        );
    }

    /// Path 2: a peer's context_token lives in the ONE session whose account
    /// talked to them — a send must resolve to that session, not to an
    /// arbitrary "default"-bot_id session (all sessions share bot_id
    /// "default" in production, so the legacy fallback is a coin flip).
    #[test]
    fn get_session_for_send_resolves_the_token_holder() {
        let state = WeixinState::new();
        register(&state, "scanner-a");
        register(&state, "scanner-b");
        // peer-p only ever talked to scanner-b.
        state
            .bots
            .get("scanner-b")
            .unwrap()
            .store_context_token("peer-p", "ctx");
        let resolved = state.get_session_for_send("default", "peer-p").unwrap();
        assert_eq!(resolved.key(), "scanner-b");
        assert_eq!(resolved.get_context_token("peer-p").as_deref(), Some("ctx"));
    }

    /// Path 1 unchanged: an admin entry whose id IS a scanner account
    /// resolves directly (the self-keyed-token pattern that makes the
    /// daily admin brief deliver for scanner-id admins).
    #[test]
    fn get_session_for_send_direct_hit_for_scanner_id() {
        let state = WeixinState::new();
        register(&state, "scanner-a");
        state
            .bots
            .get("scanner-a")
            .unwrap()
            .store_context_token("scanner-a", "self-ctx");
        let resolved = state.get_session_for_send("default", "scanner-a").unwrap();
        assert_eq!(resolved.key(), "scanner-a");
    }

    /// Path 3 unchanged: a peer with no token anywhere still yields SOME
    /// session via the bot_id fallback, so callers surface the accurate
    /// "No context_token" error rather than "No session".
    #[test]
    fn get_session_for_send_unknown_peer_falls_back_to_bot_id() {
        let state = WeixinState::new();
        register(&state, "scanner-a");
        let resolved = state.get_session_for_send("default", "stranger").unwrap();
        assert_eq!(resolved.key(), "scanner-a");
        assert!(resolved.get_context_token("stranger").is_none());
    }

    /// 绑定判定：扫码号本人（bots 的键=user_id）放行，陌生人拒——
    /// v0.1.2 bound_only 政策的入站门（daemon 桥消费）。
    #[test]
    fn is_bound_sender_matches_scanner_only() {
        let state = WeixinState::new();
        register(&state, "scanner-a");
        assert!(state.is_bound_sender("scanner-a"));
        assert!(!state.is_bound_sender("stranger"));
        assert!(!state.is_bound_sender(""));
    }

    // ---- 频道会话根（②b）——直接喂路径给私有助手，绕开 OnceLock 全局
    // （set_sessions_root 进程级一次定型，测试里污染后无法重置）。

    fn sample_tf(user_id: &str, bind_agent: Option<&str>) -> BotTokenFile {
        BotTokenFile {
            channel: "weixin".to_string(),
            sender_key: "openid".to_string(),
            bot_id: "bot-alpha".to_string(),
            bot_token: "tok".to_string(),
            baseurl: "https://example.invalid".to_string(),
            ilink_bot_id: "b@im.bot".to_string(),
            user_id: Some(user_id.to_string()),
            expires_at: 4_102_444_800,
            bind_agent: bind_agent.map(|s| s.to_string()),
            context_tokens: HashMap::new(),
        }
    }

    /// 频道扫描：bind_agent 以文件字段为真源（无目录可依），非 weixin
    /// 通道的文件不收。
    #[test]
    fn channel_scan_takes_bind_from_file_field() {
        let dir = std::env::temp_dir().join(format!("ilink-ch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // uid-1 绑 system；uid-2 无绑定（默认路由走 channel.toml）。
        write_session_file(&channel_session_path(&dir, "uid-1"), &sample_tf("uid-1", Some("system")));
        write_session_file(&channel_session_path(&dir, "uid-2"), &sample_tf("uid-2", None));
        // 非 weixin 会话不收。
        let mut other = sample_tf("uid-3", Some("x"));
        other.channel = "qq".to_string();
        write_session_file(&channel_session_path(&dir, "uid-3"), &other);

        let tfs = scan_channel_sessions(&dir);
        assert_eq!(tfs.len(), 2, "non-weixin session must be skipped");
        let uid1 = tfs.iter().find(|t| t.user_id.as_deref() == Some("uid-1")).unwrap();
        assert_eq!(uid1.bind_agent.as_deref(), Some("system"));
        assert!(tfs.iter().any(|t| t.user_id.as_deref() == Some("uid-2")
            && t.bind_agent.is_none()));
        // 0600 + tmp 不残留。
        let p = channel_session_path(&dir, "uid-1");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert!(!p.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 路径形状钉死：`<root>/senders/<key>/session.json`——刀④清场前，
    /// 这个形状是频道世界与母体旧世界互不相见的构造保证。
    #[test]
    fn channel_session_path_shape() {
        let p = channel_session_path(std::path::Path::new("/home/channels/weixin"), "o9cq@im.wechat");
        assert_eq!(
            p,
            std::path::Path::new("/home/channels/weixin/senders/o9cq@im.wechat/session.json")
        );
    }
}
