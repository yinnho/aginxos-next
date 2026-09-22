//! Cron/scheduled job types for the Carrier scheduler.
//!
//! Defines the core types for recurring and one-shot scheduled jobs that can
//! trigger agent turns, system events, or webhook deliveries.

use crate::agent::AgentId;
use crate::error::{CarrierError, CarrierResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum number of scheduled jobs per agent.
pub const MAX_JOBS_PER_AGENT: usize = 50;

/// Maximum name length in characters.
const MAX_NAME_LEN: usize = 128;

/// Minimum interval for recurring jobs (seconds).
const MIN_EVERY_SECS: u64 = 60;

/// Maximum interval for recurring jobs (seconds) = 24 hours.
const MAX_EVERY_SECS: u64 = 86_400;

/// Maximum future horizon for one-shot `At` jobs (seconds) = 1 year.
const MAX_AT_HORIZON_SECS: i64 = 365 * 24 * 3600;

/// Maximum length of SystemEvent text.
const MAX_EVENT_TEXT_LEN: usize = 4096;

/// Maximum length of AgentTurn message.
const MAX_TURN_MESSAGE_LEN: usize = 16_384;

/// Minimum timeout for AgentTurn (seconds).
const MIN_TIMEOUT_SECS: u64 = 10;

/// Maximum timeout for AgentTurn (seconds). Raised from 600 to 86400 (24h) so
/// long research crons are not capped at the old 600s. The turn itself is
/// governed by progress/stuck detection, not this backstop.
const MAX_TIMEOUT_SECS: u64 = 86_400;

/// Maximum webhook URL length.
const MAX_WEBHOOK_URL_LEN: usize = 2048;

// ---------------------------------------------------------------------------
// CronJobId
// ---------------------------------------------------------------------------

/// Unique identifier for a scheduled job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CronJobId(pub Uuid);

impl CronJobId {
    /// Generate a new random CronJobId.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CronJobId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CronJobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for CronJobId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

// ---------------------------------------------------------------------------
// CronSchedule
// ---------------------------------------------------------------------------

/// When a scheduled job fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CronSchedule {
    /// Fire once at a specific time.
    At {
        /// The exact UTC time to fire.
        at: DateTime<Utc>,
    },
    /// Fire on a fixed interval.
    Every {
        /// Interval in seconds (60..=86400).
        every_secs: u64,
    },
    /// Fire on a cron expression (5-field standard cron).
    Cron {
        /// Cron expression, e.g. `"0 9 * * 1-5"`.
        expr: String,
        /// Optional IANA timezone (e.g. `"America/New_York"`). Defaults to UTC.
        tz: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// CronAction
// ---------------------------------------------------------------------------

/// What a scheduled job does when it fires.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CronAction {
    /// Publish a system event.
    SystemEvent {
        /// Event text/payload (max 4096 chars).
        text: String,
    },
    /// Trigger an agent conversation turn.
    AgentTurn {
        /// Message to send to the agent.
        message: String,
        /// Optional model override for this turn.
        model_override: Option<String>,
        /// Timeout in seconds (10..=600).
        timeout_secs: Option<u64>,
        /// Explicitly pin the flow to run for this turn, bypassing the LLM
        /// flow classifier (loaded by name, like a resume). None = classify.
        #[serde(default)]
        active_flow: Option<String>,
        /// Session isolation override for chained pipelines: when set, the
        /// turn runs in its OWN session (label used verbatim) instead of the
        /// sender's — user chat interleaving mid-chain cannot pollute
        /// pipeline steps. sender_id still routes file paths and delivery.
        #[serde(default)]
        session_label: Option<String>,
    },
    /// Push fixed content on a schedule WITHOUT an agent turn (automation
    /// Phase 2). Payload is the same `ContentDescriptor`-shaped JSON as
    /// automation-rule `task_payload`, executed through the same
    /// `do_push_message` path — no LLM, no session.
    Push {
        /// Channel the bot lives on (`"weixin-oa"`; informational for routing
        /// diagnostics — delivery resolves per-recipient via sender_channels).
        channel: String,
        /// Channel bot id (weixin-oa: the service-account app_id). Also the
        /// `source_bot_id` handed to `do_push_message`.
        bot_id: String,
        /// `ContentDescriptor`-shaped JSON: `{"text": "..."}` or
        /// `{"miniprogram": {appid, pagepath, title, thumb_media_id}}`.
        payload: serde_json::Value,
        /// Audience: `"admins"` (fan-out via admins.json), `"followers"`
        /// (pushable followers of bot_id), or a raw user id (openid) — the
        /// same vocabulary as automation-rule `target`.
        target: String,
    },
    /// Follower-growth digest pushed to admins (no LLM). Computes new/unfollow
    /// counts since this job's previous fire from the followers ledger and
    /// delivers a fixed-format text to the agent's admins.
    FollowerReport {
        /// Channel of the ledger rows to summarize (`"weixin-oa"`).
        channel: String,
        /// Bot id (app_id) whose followers are summarized.
        bot_id: String,
    },
    /// Poll pending freepublish ids for an account and report terminal states
    /// to admins (no LLM). The pending list lives in the wechat-oa core
    /// publish tracker (`~/.aginx/carrier/data/wechat_publish_pending.json`);
    /// the publish tool records each submitted publish_id there. The arm
    /// self-deletes this job after consecutive empty rounds — the daemon
    /// reconcile loop re-creates it when new publishes appear.
    PublishPoll {
        /// Channel of the account (`"weixin-oa"`).
        channel: String,
        /// Bot id (app_id) whose publishes are polled.
        bot_id: String,
    },
    /// Pull NEW reader comments from every published article of an account
    /// and append them into the bound clone's knowledge (no LLM). The dedup
    /// ledger (`~/.aginx/carrier/data/wechat_comment_seen.json`) keeps each run
    /// incremental; new comments land in `knowledge/读者留言-YYYY-MM.md` of
    /// the account's `bind_agent` workspace, where the lifecycle knowledge
    /// compiler picks them up. A one-line digest goes to admins when anything
    /// new was ingested. Standing job (never self-deletes).
    CommentPull {
        /// Channel of the account (`"weixin-oa"`).
        channel: String,
        /// Bot id (app_id) whose articles' comments are pulled.
        bot_id: String,
    },
}

// ---------------------------------------------------------------------------
// CronDelivery
// ---------------------------------------------------------------------------

/// Where the job's output is delivered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CronDelivery {
    /// No delivery — fire and forget.
    None,
    /// Deliver via the user's last communication channel (degrades to None if no channel).
    LastChannel,
    /// Deliver to all admins of the agent's workspace (fans out via admins.json
    /// → `do_push_message("admins", …)`). Unlike an agent calling `message_push`,
    /// this is a privileged delivery path with no ephemeral admin-identity gate,
    /// so it reliably reaches admins from a scheduled (async) cron turn.
    Admins,
    /// Deliver via HTTP webhook.
    Webhook {
        /// Webhook URL (must start with `http://` or `https://`).
        url: String,
    },
}

// ---------------------------------------------------------------------------
// CronJob
// ---------------------------------------------------------------------------

/// Chained-pipeline identity for a cron job (Plan A of broken-chain
/// monitoring). Chained pipelines (writing chains: research → outline →
/// article → format → publish) run each step as a one-shot cron that ends by
/// `cron_create`-ing the next step. A step that completes WITHOUT scheduling
/// its successor breaks the chain silently — this metadata makes the
/// expectation explicit so the daemon can detect and alert on it.
///
/// `chain_id` should match the pipeline id (and typically the `session_label`
/// / `output/<pipeline_id>/` paths). `step` is 1-based; `step == total_steps`
/// marks the tail step, which legitimately creates no successor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainMeta {
    pub chain_id: String,
    pub step: u32,
    pub total_steps: u32,
}

impl ChainMeta {
    /// True when this job is the tail of its chain (no successor expected).
    pub fn is_tail(&self) -> bool {
        self.step >= self.total_steps
    }
}

/// A scheduled job belonging to a specific agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    /// Unique job identifier.
    pub id: CronJobId,
    /// Owning agent.
    pub agent_id: AgentId,
    /// Route owner (bot/app owner) who created this job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
    /// Actual sender/user on whose behalf this job runs (for per-sender file paths).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_id: Option<String>,
    /// Human-readable name (max 128 chars, alphanumeric + spaces/hyphens/underscores).
    pub name: String,
    /// Whether the job is active.
    pub enabled: bool,
    /// When to fire.
    pub schedule: CronSchedule,
    /// What to do when fired.
    pub action: CronAction,
    /// Where to deliver the result.
    pub delivery: CronDelivery,
    /// Chained-pipeline identity; when set on a non-tail step, the daemon
    /// alerts if no same-chain job is pending after the turn completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainMeta>,
    /// When the job was created.
    pub created_at: DateTime<Utc>,
    /// When the job last fired (if ever).
    pub last_run: Option<DateTime<Utc>>,
    /// When the job is next expected to fire.
    pub next_run: Option<DateTime<Utc>>,
}

impl CronJob {
    /// Validate this job's fields.
    ///
    /// `existing_count` is the number of jobs the owning agent already has
    /// (excluding this job if it already exists). Returns `Ok(())` or an
    /// error message describing the first validation failure.
    pub fn validate(&self, existing_count: usize) -> CarrierResult<()> {
        // -- job count cap --
        if existing_count >= MAX_JOBS_PER_AGENT {
            return Err(CarrierError::InvalidInput(format!(
                "agent already has {existing_count} jobs (max {MAX_JOBS_PER_AGENT})"
            )));
        }

        // -- name --
        if self.name.is_empty() {
            return Err(CarrierError::InvalidInput("name must not be empty".into()));
        }
        if self.name.len() > MAX_NAME_LEN {
            return Err(CarrierError::InvalidInput(format!(
                "name too long ({} chars, max {MAX_NAME_LEN})",
                self.name.len()
            )));
        }
        // Names are free-form labels (stored parameterized in SQLite, never used
        // as a filename/path/shell arg). Reject only control characters (which
        // would corrupt logs/listings); allow all letters, digits, punctuation,
        // and emoji — including CJK and Chinese punctuation — so agents can name
        // jobs naturally (e.g. "发布第二篇：OpenAI 硬件").
        if self.name.chars().any(|c| c.is_control()) {
            return Err(CarrierError::InvalidInput(
                "name may not contain control characters".into(),
            ));
        }

        // -- schedule --
        self.validate_schedule()?;

        // -- action --
        self.validate_action()?;

        // -- delivery --
        self.validate_delivery()?;

        Ok(())
    }

    fn validate_schedule(&self) -> CarrierResult<()> {
        match &self.schedule {
            CronSchedule::Every { every_secs } => {
                if *every_secs < MIN_EVERY_SECS {
                    return Err(CarrierError::InvalidInput(format!(
                        "every_secs too small ({every_secs}, min {MIN_EVERY_SECS})"
                    )));
                }
                if *every_secs > MAX_EVERY_SECS {
                    return Err(CarrierError::InvalidInput(format!(
                        "every_secs too large ({every_secs}, max {MAX_EVERY_SECS})"
                    )));
                }
            }
            CronSchedule::At { at } => {
                let now = Utc::now();
                if *at <= now {
                    return Err(CarrierError::InvalidInput(
                        "scheduled time must be in the future".into(),
                    ));
                }
                let delta = (*at - now).num_seconds();
                if delta > MAX_AT_HORIZON_SECS {
                    return Err(CarrierError::InvalidInput(format!(
                        "scheduled time too far in the future (max {MAX_AT_HORIZON_SECS}s / ~1 year)"
                    )));
                }
            }
            CronSchedule::Cron { expr, .. } => {
                validate_cron_expr(expr)?;
            }
        }
        Ok(())
    }

    fn validate_action(&self) -> CarrierResult<()> {
        match &self.action {
            CronAction::SystemEvent { text } => {
                if text.is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "system event text must not be empty".into(),
                    ));
                }
                if text.len() > MAX_EVENT_TEXT_LEN {
                    return Err(CarrierError::InvalidInput(format!(
                        "system event text too long ({} chars, max {MAX_EVENT_TEXT_LEN})",
                        text.len()
                    )));
                }
            }
            CronAction::AgentTurn {
                message,
                timeout_secs,
                ..
            } => {
                if message.is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "agent turn message must not be empty".into(),
                    ));
                }
                if message.len() > MAX_TURN_MESSAGE_LEN {
                    return Err(CarrierError::InvalidInput(format!(
                        "agent turn message too long ({} chars, max {MAX_TURN_MESSAGE_LEN})",
                        message.len()
                    )));
                }
                if let Some(t) = timeout_secs {
                    if *t < MIN_TIMEOUT_SECS {
                        return Err(CarrierError::InvalidInput(format!(
                            "timeout_secs too small ({t}, min {MIN_TIMEOUT_SECS})"
                        )));
                    }
                    if *t > MAX_TIMEOUT_SECS {
                        return Err(CarrierError::InvalidInput(format!(
                            "timeout_secs too large ({t}, max {MAX_TIMEOUT_SECS})"
                        )));
                    }
                }
            }
            CronAction::Push {
                channel,
                bot_id,
                payload,
                target,
            } => {
                if channel.trim().is_empty() || bot_id.trim().is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "push action requires non-empty channel and bot_id".into(),
                    ));
                }
                if target.trim().is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "push action requires a target (\"admins\" | \"followers\" | user id)"
                            .into(),
                    ));
                }
                // Payload must be a valid NON-EMPTY ContentDescriptor — every
                // field is optional, so a bare `{"unexpected": true}` would
                // otherwise parse into an empty descriptor and deliver nothing.
                match serde_json::from_value::<crate::content::ContentDescriptor>(payload.clone()) {
                    Ok(c)
                        if c.text.is_some()
                            || c.link.is_some()
                            || c.image.is_some()
                            || c.video.is_some()
                            || c.file.is_some()
                            || c.voice.is_some()
                            || c.miniprogram.is_some()
                            || c.template.is_some() => {}
                    _ => {
                        return Err(CarrierError::InvalidInput(
                            "push payload must be ContentDescriptor-shaped and non-empty: {\"text\": \"...\"} or {\"miniprogram\": {appid, pagepath, title, thumb_media_id}} or {\"template\": {template_id, data}}".into(),
                        ));
                    }
                }
            }
            CronAction::FollowerReport { channel, bot_id } => {
                if channel.trim().is_empty() || bot_id.trim().is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "follower_report action requires non-empty channel and bot_id".into(),
                    ));
                }
            }
            CronAction::PublishPoll { channel, bot_id } => {
                if channel.trim().is_empty() || bot_id.trim().is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "publish_poll action requires non-empty channel and bot_id".into(),
                    ));
                }
            }
            CronAction::CommentPull { channel, bot_id } => {
                if channel.trim().is_empty() || bot_id.trim().is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "comment_pull action requires non-empty channel and bot_id".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_delivery(&self) -> CarrierResult<()> {
        match &self.delivery {
            CronDelivery::Webhook { url } => {
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(CarrierError::InvalidInput(
                        "webhook URL must start with http:// or https://".into(),
                    ));
                }
                if url.len() > MAX_WEBHOOK_URL_LEN {
                    return Err(CarrierError::InvalidInput(format!(
                        "webhook URL too long ({} chars, max {MAX_WEBHOOK_URL_LEN})",
                        url.len()
                    )));
                }
            }
            CronDelivery::None => {}
            CronDelivery::LastChannel => {}
            CronDelivery::Admins => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Cron expression basic format validation
// ---------------------------------------------------------------------------

/// Basic cron expression format validation: must have exactly 5 whitespace-separated fields.
/// Actual parsing and scheduling is done in the kernel crate.
fn validate_cron_expr(expr: &str) -> CarrierResult<()> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err(CarrierError::InvalidInput(
            "cron expression must not be empty".into(),
        ));
    }
    let fields: Vec<&str> = trimmed.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(CarrierError::InvalidInput(format!(
            "cron expression must have exactly 5 fields (got {}): \"{}\"",
            fields.len(),
            trimmed
        )));
    }
    // Basic character validation per field — allow digits, *, /, -, and ,.
    for (i, field) in fields.iter().enumerate() {
        if field.is_empty() {
            return Err(CarrierError::InvalidInput(format!(
                "cron field {i} is empty"
            )));
        }
        if !field
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '*' | '/' | '-' | ',' | '?'))
        {
            return Err(CarrierError::InvalidInput(format!(
                "cron field {i} contains invalid characters: \"{field}\""
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// Helper: build a minimal valid CronJob.
    fn valid_job() -> CronJob {
        CronJob {
            id: CronJobId::new(),
            agent_id: AgentId::new(),
            owner_id: None,
            sender_id: None,
            name: "daily-report".into(),
            enabled: true,
            schedule: CronSchedule::Every { every_secs: 3600 },
            action: CronAction::SystemEvent {
                text: "ping".into(),
            },
            delivery: CronDelivery::None,
            chain: None,
            created_at: Utc::now(),
            last_run: None,
            next_run: None,
        }
    }

    #[test]
    fn validate_push_action_roundtrip() {
        // Valid: ContentDescriptor-shaped payload + explicit target.
        let mut job = valid_job();
        job.action = CronAction::Push {
            channel: "weixin-oa".into(),
            bot_id: "wx123".into(),
            payload: serde_json::json!({"text": "早上好"}),
            target: "followers".into(),
        };
        assert!(job.validate(0).is_ok());

        // Bad payload shape -> rejected at creation, not at fire time.
        job.action = CronAction::Push {
            channel: "weixin-oa".into(),
            bot_id: "wx123".into(),
            payload: serde_json::json!({"unexpected": true}),
            target: "admins".into(),
        };
        assert!(job.validate(0).is_err());

        // Empty target rejected.
        job.action = CronAction::Push {
            channel: "weixin-oa".into(),
            bot_id: "wx123".into(),
            payload: serde_json::json!({"text": "hi"}),
            target: "".into(),
        };
        assert!(job.validate(0).is_err());

        // Serde roundtrip keeps the new variant's tag shape (cron_create sends
        // {"kind":"push",...}).
        let json = serde_json::to_value(&job.action).unwrap();
        assert_eq!(json["kind"], "push");
        let back: CronAction = serde_json::from_value(json).unwrap();
        assert!(matches!(back, CronAction::Push { ref target, .. } if target.is_empty()));
    }

    #[test]
    fn validate_follower_report_action() {
        let mut job = valid_job();
        job.action = CronAction::FollowerReport {
            channel: "weixin-oa".into(),
            bot_id: "wx123".into(),
        };
        assert!(job.validate(0).is_ok());

        let json = serde_json::to_value(&job.action).unwrap();
        assert_eq!(json["kind"], "follower_report");
        let back: CronAction = serde_json::from_value(json).unwrap();
        assert!(matches!(back, CronAction::FollowerReport { .. }));
    }

    /// A template-only Push payload (zero-LLM scheduled push beyond the 48h
    /// customer-service window) must pass creation validation, and the
    /// publish_poll action kind round-trips its snake_case tag (old cron DB
    /// rows with other kinds are unaffected — additive enum variant).
    #[test]
    fn validate_template_push_and_publish_poll() {
        let mut job = valid_job();
        job.action = CronAction::Push {
            channel: "weixin-oa".into(),
            bot_id: "wx123".into(),
            payload: serde_json::json!({
                "template": {
                    "template_id": "tpl-1",
                    "data": {"thing1": {"value": "月卡到期提醒"}}
                }
            }),
            target: "followers".into(),
        };
        assert!(job.validate(0).is_ok());

        job.action = CronAction::PublishPoll {
            channel: "weixin-oa".into(),
            bot_id: "wx123".into(),
        };
        assert!(job.validate(0).is_ok());
        let json = serde_json::to_value(&job.action).unwrap();
        assert_eq!(json["kind"], "publish_poll");
        let back: CronAction = serde_json::from_value(json).unwrap();
        assert!(matches!(back, CronAction::PublishPoll { ref bot_id, .. } if bot_id == "wx123"));

        // Empty bot_id rejected like the other zero-LLM actions.
        job.action = CronAction::PublishPoll {
            channel: "weixin-oa".into(),
            bot_id: "  ".into(),
        };
        assert!(job.validate(0).is_err());
    }

    /// comment_pull: standing comment-ingestion job — snake_case tag
    /// round-trips and empty bot_id is rejected like its siblings.
    #[test]
    fn validate_comment_pull_action() {
        let mut job = valid_job();
        job.action = CronAction::CommentPull {
            channel: "weixin-oa".into(),
            bot_id: "wx123".into(),
        };
        assert!(job.validate(0).is_ok());
        let json = serde_json::to_value(&job.action).unwrap();
        assert_eq!(json["kind"], "comment_pull");
        let back: CronAction = serde_json::from_value(json).unwrap();
        assert!(matches!(back, CronAction::CommentPull { ref bot_id, .. } if bot_id == "wx123"));

        job.action = CronAction::CommentPull {
            channel: "weixin-oa".into(),
            bot_id: "".into(),
        };
        assert!(job.validate(0).is_err());
    }

    // -- CronJobId --

    /// ChainMeta tail semantics: only `step == total_steps` is the tail (no
    /// successor expected, no broken-chain alert). Anything less expects one.
    #[test]
    fn chain_meta_is_tail() {
        let mid = ChainMeta {
            chain_id: "p".into(),
            step: 2,
            total_steps: 5,
        };
        assert!(
            !mid.is_tail(),
            "step 2/5 is a mid step — successor expected"
        );
        let tail = ChainMeta {
            chain_id: "p".into(),
            step: 5,
            total_steps: 5,
        };
        assert!(tail.is_tail(), "step 5/5 is the tail — no successor");
        // step beyond total is treated as tail (defensive: validation rejects
        // it at cron_create, but is_tail must never panic or flip the other way)
        let over = ChainMeta {
            chain_id: "p".into(),
            step: 6,
            total_steps: 5,
        };
        assert!(over.is_tail());
    }

    /// Chained-pipeline session isolation: `session_label` rides the agent_turn
    /// wire format, absent on old payloads (serde default).
    #[test]
    fn agent_turn_session_label_wire_roundtrip() {
        let action = CronAction::AgentTurn {
            message: "写正文".into(),
            model_override: None,
            timeout_secs: None,
            active_flow: Some("article-writer".into()),
            session_label: Some("pipeline:20260815-glm53".into()),
        };
        let v = serde_json::to_value(&action).unwrap();
        assert_eq!(v["kind"], "agent_turn");
        assert_eq!(v["session_label"], "pipeline:20260815-glm53");
        let back: CronAction = serde_json::from_value(v).unwrap();
        match back {
            CronAction::AgentTurn { session_label, .. } => {
                assert_eq!(session_label.as_deref(), Some("pipeline:20260815-glm53"));
            }
            other => panic!("expected AgentTurn, got {other:?}"),
        }

        // Old payload without the field still parses (None).
        let legacy = serde_json::json!({
            "kind": "agent_turn",
            "message": "hi",
        });
        let back: CronAction = serde_json::from_value(legacy).unwrap();
        match back {
            CronAction::AgentTurn { session_label, .. } => assert!(session_label.is_none()),
            other => panic!("expected AgentTurn, got {other:?}"),
        }
    }

    #[test]
    fn cron_job_id_display_roundtrip() {
        let id = CronJobId::new();
        let s = id.to_string();
        let parsed: CronJobId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn cron_job_id_default() {
        let a = CronJobId::default();
        let b = CronJobId::default();
        assert_ne!(a, b);
    }

    // -- Valid job --

    #[test]
    fn valid_job_passes() {
        assert!(valid_job().validate(0).is_ok());
    }

    // -- Name validation --

    #[test]
    fn empty_name_rejected() {
        let mut job = valid_job();
        job.name = String::new();
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn long_name_rejected() {
        let mut job = valid_job();
        job.name = "a".repeat(129);
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn name_128_chars_ok() {
        let mut job = valid_job();
        job.name = "a".repeat(128);
        assert!(job.validate(0).is_ok());
    }

    #[test]
    fn name_special_chars_ok() {
        // Punctuation, CJK, and Chinese punctuation are all valid now — names are
        // free-form labels, not identifiers.
        let mut job = valid_job();
        job.name = "my job!".into();
        assert!(job.validate(0).is_ok());
        job.name = "发布第二篇：OpenAI 硬件（2026）".into();
        assert!(job.validate(0).is_ok());
        job.name = "article-2 / WAIC".into();
        assert!(job.validate(0).is_ok());
    }

    #[test]
    fn name_control_chars_rejected() {
        let mut job = valid_job();
        job.name = "job\nwith newline".into();
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("control characters"), "{err}");
        job.name = "bad\0null".into();
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("control characters"), "{err}");
    }

    #[test]
    fn name_with_spaces_hyphens_underscores_ok() {
        let mut job = valid_job();
        job.name = "My Daily-Report_v2".into();
        assert!(job.validate(0).is_ok());
    }

    // -- Job count cap --

    #[test]
    fn max_jobs_rejected() {
        let job = valid_job();
        let err = job.validate(50).unwrap_err().to_string();
        assert!(err.contains("50"), "{err}");
    }

    #[test]
    fn under_max_jobs_ok() {
        let job = valid_job();
        assert!(job.validate(49).is_ok());
    }

    // -- Schedule: Every --

    #[test]
    fn every_too_small() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Every { every_secs: 59 };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too small"), "{err}");
    }

    #[test]
    fn every_too_large() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Every { every_secs: 86_401 };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn every_min_boundary_ok() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Every { every_secs: 60 };
        assert!(job.validate(0).is_ok());
    }

    #[test]
    fn every_max_boundary_ok() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Every { every_secs: 86_400 };
        assert!(job.validate(0).is_ok());
    }

    // -- Schedule: At --

    #[test]
    fn at_in_past_rejected() {
        let mut job = valid_job();
        job.schedule = CronSchedule::At {
            at: Utc::now() - Duration::seconds(10),
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("future"), "{err}");
    }

    #[test]
    fn at_too_far_future_rejected() {
        let mut job = valid_job();
        job.schedule = CronSchedule::At {
            at: Utc::now() + Duration::days(366),
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too far"), "{err}");
    }

    #[test]
    fn at_near_future_ok() {
        let mut job = valid_job();
        job.schedule = CronSchedule::At {
            at: Utc::now() + Duration::hours(1),
        };
        assert!(job.validate(0).is_ok());
    }

    // -- Schedule: Cron --

    #[test]
    fn cron_valid_expr() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Cron {
            expr: "0 9 * * 1-5".into(),
            tz: Some("America/New_York".into()),
        };
        assert!(job.validate(0).is_ok());
    }

    #[test]
    fn cron_empty_expr() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Cron {
            expr: String::new(),
            tz: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn cron_wrong_field_count() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Cron {
            expr: "0 9 * *".into(),
            tz: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("5 fields"), "{err}");
    }

    #[test]
    fn cron_invalid_chars() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Cron {
            expr: "0 9 * * MON".into(),
            tz: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("invalid characters"), "{err}");
    }

    // -- Action: SystemEvent --

    #[test]
    fn system_event_empty_text() {
        let mut job = valid_job();
        job.action = CronAction::SystemEvent {
            text: String::new(),
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn system_event_text_too_long() {
        let mut job = valid_job();
        job.action = CronAction::SystemEvent {
            text: "x".repeat(4097),
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn system_event_max_text_ok() {
        let mut job = valid_job();
        job.action = CronAction::SystemEvent {
            text: "x".repeat(4096),
        };
        assert!(job.validate(0).is_ok());
    }

    // -- Action: AgentTurn --

    #[test]
    fn agent_turn_empty_message() {
        let mut job = valid_job();
        job.action = CronAction::AgentTurn {
            message: String::new(),
            model_override: None,
            timeout_secs: None,
            active_flow: None,
            session_label: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn agent_turn_message_too_long() {
        let mut job = valid_job();
        job.action = CronAction::AgentTurn {
            message: "x".repeat(16_385),
            model_override: None,
            timeout_secs: None,
            active_flow: None,
            session_label: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn agent_turn_timeout_too_small() {
        let mut job = valid_job();
        job.action = CronAction::AgentTurn {
            message: "hello".into(),
            model_override: None,
            timeout_secs: Some(9),
            active_flow: None,
            session_label: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too small"), "{err}");
    }

    #[test]
    fn agent_turn_timeout_too_large() {
        let mut job = valid_job();
        job.action = CronAction::AgentTurn {
            message: "hello".into(),
            model_override: None,
            timeout_secs: Some(86_401),
            active_flow: None,
            session_label: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too large"), "{err}");
    }

    #[test]
    fn agent_turn_timeout_boundaries_ok() {
        let mut job = valid_job();
        job.action = CronAction::AgentTurn {
            message: "hello".into(),
            model_override: Some("claude-haiku-4-5-20251001".into()),
            timeout_secs: Some(10),
            active_flow: None,
            session_label: None,
        };
        assert!(job.validate(0).is_ok());

        job.action = CronAction::AgentTurn {
            message: "hello".into(),
            model_override: None,
            timeout_secs: Some(86_400),
            active_flow: None,
            session_label: None,
        };
        assert!(job.validate(0).is_ok());
    }

    #[test]
    fn agent_turn_no_timeout_ok() {
        let mut job = valid_job();
        job.action = CronAction::AgentTurn {
            message: "hello".into(),
            model_override: None,
            timeout_secs: None,
            active_flow: None,
            session_label: None,
        };
        assert!(job.validate(0).is_ok());
    }

    // -- Delivery: Webhook --

    #[test]
    fn webhook_bad_scheme() {
        let mut job = valid_job();
        job.delivery = CronDelivery::Webhook {
            url: "ftp://example.com/hook".into(),
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("http://"), "{err}");
    }

    #[test]
    fn webhook_too_long() {
        let mut job = valid_job();
        job.delivery = CronDelivery::Webhook {
            url: format!("https://example.com/{}", "a".repeat(2048)),
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn webhook_http_ok() {
        let mut job = valid_job();
        job.delivery = CronDelivery::Webhook {
            url: "http://localhost:8080/hook".into(),
        };
        assert!(job.validate(0).is_ok());
    }

    #[test]
    fn webhook_https_ok() {
        let mut job = valid_job();
        job.delivery = CronDelivery::Webhook {
            url: "https://example.com/hook".into(),
        };
        assert!(job.validate(0).is_ok());
    }

    // -- Delivery: None --

    #[test]
    fn delivery_none_ok() {
        let mut job = valid_job();
        job.delivery = CronDelivery::None;
        assert!(job.validate(0).is_ok());
    }

    // -- Serde roundtrip --

    #[test]
    fn serde_roundtrip_every() {
        let job = valid_job();
        let json = serde_json::to_string(&job).unwrap();
        let back: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, job.name);
        assert_eq!(back.id, job.id);
    }

    #[test]
    fn serde_roundtrip_cron_schedule() {
        let schedule = CronSchedule::Cron {
            expr: "*/5 * * * *".into(),
            tz: Some("UTC".into()),
        };
        let json = serde_json::to_string(&schedule).unwrap();
        assert!(json.contains("\"kind\":\"cron\""));
        let back: CronSchedule = serde_json::from_str(&json).unwrap();
        if let CronSchedule::Cron { expr, tz } = back {
            assert_eq!(expr, "*/5 * * * *");
            assert_eq!(tz, Some("UTC".into()));
        } else {
            panic!("expected Cron variant");
        }
    }

    #[test]
    fn serde_action_tags() {
        let action = CronAction::AgentTurn {
            message: "hi".into(),
            model_override: None,
            timeout_secs: Some(30),
            active_flow: None,
            session_label: None,
        };
        let json = serde_json::to_string(&action).unwrap();
        assert!(json.contains("\"kind\":\"agent_turn\""));
    }

    #[test]
    fn serde_delivery_tags() {
        let d = CronDelivery::None;
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"kind\":\"none\""));

        let d2 = CronDelivery::Webhook {
            url: "https://x.com".into(),
        };
        let json2 = serde_json::to_string(&d2).unwrap();
        assert!(json2.contains("\"kind\":\"webhook\""));

        // Admins delivery: round-trip through {"kind":"admins"}.
        let d3 = CronDelivery::Admins;
        let json3 = serde_json::to_string(&d3).unwrap();
        assert_eq!(json3, "{\"kind\":\"admins\"}");
        let back: CronDelivery = serde_json::from_str(&json3).unwrap();
        assert!(matches!(back, CronDelivery::Admins));
    }

    // -- Cron expression edge cases --

    #[test]
    fn cron_extra_whitespace_ok() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Cron {
            expr: "  0  9  *  *  *  ".into(),
            tz: None,
        };
        assert!(job.validate(0).is_ok());
    }

    #[test]
    fn cron_six_fields_rejected() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Cron {
            expr: "0 0 9 * * 1".into(),
            tz: None,
        };
        let err = job.validate(0).unwrap_err().to_string();
        assert!(err.contains("5 fields"), "{err}");
    }

    #[test]
    fn cron_slash_and_comma_ok() {
        let mut job = valid_job();
        job.schedule = CronSchedule::Cron {
            expr: "*/15 0,12 1-15 * 1,3,5".into(),
            tz: None,
        };
        assert!(job.validate(0).is_ok());
    }
}
