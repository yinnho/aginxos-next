//! Automation rules: per-app/channel "trigger -> fixed action" rules that the
//! channel layer matches on inbound events and executes WITHOUT routing to the
//! agent LLM (e.g. weixin-oa subscribe -> push welcome text; keyword "月卡" ->
//! push miniprogram card).
//!
//! Lived in `types` (not `memory`) so `runtime`/`api`/`kernel` can name the
//! structs without a circular dep, mirroring `types::scheduler` / `types::content`.
//!
//! `task_payload` is a `ContentDescriptor`-shaped JSON object so the same
//! `execute_push` path serves both inbound-trigger (Phase 1) and the future
//! cron `Push` action (Phase 2).

use serde::{Deserialize, Serialize};

/// What inbound event fires this rule.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// `msg_type=event & event=subscribe` (follow). `trigger_data` unused.
    Subscribe,
    /// `msg_type=text` whose content contains `trigger_data` (substring).
    Keyword,
    /// Custom-menu click: `msg_type=event & event=CLICK` whose `EventKey`
    /// contains `trigger_data` (substring). A click opens the 48h customer
    /// service window, so pushes deliver via the API path.
    MenuClick,
    /// QR scene scan: `event=SCAN` (already-followed re-scan) or a subscribe
    /// carrying `qrscene_*` (new follow via QR). `trigger_data` = scene
    /// substring matched against `EventKey`.
    Scan,
}

impl TriggerKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Subscribe => "subscribe",
            Self::Keyword => "keyword",
            Self::MenuClick => "menu_click",
            Self::Scan => "scan",
        }
    }
}

/// What to do when the rule fires. Both are expressible as a
/// `ContentDescriptor`, so `execute_push` deserializes `task_payload` into one
/// and delivers it (no LLM). Phase 2 may add `PushImage` / `AgentTurn`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// `task_payload = {"text": "..."}`.
    PushText,
    /// `task_payload = {"miniprogram": {appid, pagepath, title, thumb_media_id}}`.
    PushMiniprogram,
    /// Bypass push to admins (does NOT skip the agent -- agent still replies to
    /// the user). `task_payload = {"notify_type": "..."}`; the push content is
    /// the user's message + source, routed via `notify_routes[notify_type]`.
    NotifyAdmin,
    /// Unified push: format inferred from `task_payload` (ContentDescriptor
    /// shape), target from `rule.target`. Subsumes PushText/PushMiniprogram/
    /// NotifyAdmin — those legacy variants remain for backward compat.
    Push,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PushText => "push_text",
            Self::PushMiniprogram => "push_miniprogram",
            Self::NotifyAdmin => "notify_admin",
            Self::Push => "push",
        }
    }
}

/// A single automation rule, scoped to a `(channel, app_id)` pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRule {
    pub id: String,
    /// Bot id for the channel (weixin-oa: the service-account app_id).
    pub app_id: String,
    /// Channel name (`"weixin-oa"`; future: `wecom` / `feishu` / ...).
    pub channel: String,
    pub name: String,
    pub enabled: bool,
    /// Higher = evaluated first. Ties broken by `created_at` ASC.
    pub priority: i64,
    pub trigger_kind: TriggerKind,
    /// Keyword text (empty for `Subscribe`).
    pub trigger_data: String,
    pub task_kind: TaskKind,
    /// `ContentDescriptor`-shaped JSON consumed by `execute_push` /
    /// `push_message`.
    pub task_payload: serde_json::Value,
    /// Push target: `"current"` (default, the triggering user), `"admins"`
    /// (fan-out via admins.json), or a specific user_id. Only meaningful for
    /// `TaskKind::Push` (legacy variants have implicit targets).
    #[serde(default = "default_target")]
    pub target: String,
    pub created_at: String,
    pub updated_at: String,
}

fn default_target() -> String {
    "current".to_string()
}
