//! Configuration types for the Carrier kernel.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Controls what usage info appears in response footers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageFooterMode {
    /// Don't show usage info.
    Off,
    /// Show token counts only.
    Tokens,
    /// Show estimated cost only.
    Cost,
    /// Show tokens + cost (default).
    #[default]
    Full,
}

/// Kernel operating mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KernelMode {
    /// Conservative mode — no auto-updates, pinned models, stability-first.
    Stable,
    /// Default balanced mode.
    #[default]
    Default,
    /// Developer mode — experimental features enabled.
    Dev,
}

/// Web tools configuration (search + fetch).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Cache TTL in minutes (0 = disabled).
    pub cache_ttl_minutes: u64,
    /// Web fetch configuration.
    pub fetch: WebFetchConfig,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            cache_ttl_minutes: 15,
            fetch: WebFetchConfig::default(),
        }
    }
}

/// Web fetch configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebFetchConfig {
    /// Maximum characters to return in content.
    pub max_chars: usize,
    /// Maximum response body size in bytes.
    pub max_response_bytes: usize,
    /// HTTP request timeout in seconds.
    pub timeout_secs: u64,
    /// Enable HTML→Markdown readability extraction.
    pub readability: bool,
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            max_chars: 50_000,
            max_response_bytes: 10 * 1024 * 1024, // 10 MB
            timeout_secs: 30,
            readability: true,
        }
    }
}

/// Browser backend preference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserBackend {
    /// Auto-detect: try obscura first, fall back to chromium.
    #[default]
    Auto,
    /// Force use of Obscura.
    Obscura,
    /// Force use of Chromium.
    Chromium,
}

/// Browser automation configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    /// Preferred browser backend. Default: auto (obscura with chromium fallback).
    pub backend: BrowserBackend,
    /// Run browser in headless mode (no visible window).
    pub headless: bool,
    /// Viewport width in pixels.
    pub viewport_width: u32,
    /// Viewport height in pixels.
    pub viewport_height: u32,
    /// Per-action timeout in seconds.
    pub timeout_secs: u64,
    /// Idle timeout — auto-close session after this many seconds of inactivity.
    pub idle_timeout_secs: u64,
    /// Maximum concurrent browser sessions.
    pub max_sessions: usize,
    /// Path to Chromium/Chrome binary. Auto-detected if None.
    pub chromium_path: Option<String>,
    /// External CDP WebSocket endpoint (e.g., "ws://127.0.0.1:9222/devtools/browser").
    /// When set, skips launching a browser process and connects directly.
    /// Use this with Obscura (`obscura serve --port 9222`) or remote CDP servers.
    pub cdp_endpoint: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            backend: BrowserBackend::default(),
            headless: true,
            viewport_width: 1280,
            viewport_height: 720,
            timeout_secs: 30,
            idle_timeout_secs: 300,
            max_sessions: 5,
            chromium_path: None,
            cdp_endpoint: None,
        }
    }
}

/// Config hot-reload mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadMode {
    /// No automatic reloading.
    Off,
    /// Full restart on config change.
    Restart,
    /// Hot-reload safe sections only (channels, flows, heartbeat).
    Hot,
    /// Hot-reload where possible, flag restart-required otherwise.
    #[default]
    Hybrid,
}

/// Configuration for config file watching and hot-reload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReloadConfig {
    /// Reload mode. Default: hybrid.
    pub mode: ReloadMode,
    /// Debounce window in milliseconds. Default: 500.
    pub debounce_ms: u64,
}

impl Default for ReloadConfig {
    fn default() -> Self {
        Self {
            mode: ReloadMode::default(),
            debounce_ms: 500,
        }
    }
}

/// Text-to-speech configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    /// Enable TTS. Default: false.
    pub enabled: bool,
    /// Default provider: "openai" or "elevenlabs".
    pub provider: Option<String>,
    /// OpenAI TTS settings.
    pub openai: TtsOpenAiConfig,
    /// ElevenLabs TTS settings.
    pub elevenlabs: TtsElevenLabsConfig,
    /// Max text length for TTS (chars). Default: 4096.
    pub max_text_length: usize,
    /// Timeout per TTS request in seconds. Default: 30.
    pub timeout_secs: u64,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            openai: TtsOpenAiConfig::default(),
            elevenlabs: TtsElevenLabsConfig::default(),
            max_text_length: 4096,
            timeout_secs: 30,
        }
    }
}

/// OpenAI TTS settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsOpenAiConfig {
    /// Voice: alloy, echo, fable, onyx, nova, shimmer. Default: "alloy".
    pub voice: String,
    /// Model: "tts-1" or "tts-1-hd". Default: "tts-1".
    pub model: String,
    /// Output format: "mp3", "opus", "aac", "flac". Default: "mp3".
    pub format: String,
    /// Speed: 0.25 to 4.0. Default: 1.0.
    pub speed: f32,
}

impl Default for TtsOpenAiConfig {
    fn default() -> Self {
        Self {
            voice: "alloy".to_string(),
            model: "tts-1".to_string(),
            format: "mp3".to_string(),
            speed: 1.0,
        }
    }
}

/// ElevenLabs TTS settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsElevenLabsConfig {
    /// Voice ID. Default: "21m00Tcm4TlvDq8ikWAM" (Rachel).
    pub voice_id: String,
    /// Model ID. Default: "eleven_monolingual_v1".
    pub model_id: String,
    /// Stability (0.0-1.0). Default: 0.5.
    pub stability: f32,
    /// Similarity boost (0.0-1.0). Default: 0.75.
    pub similarity_boost: f32,
}

impl Default for TtsElevenLabsConfig {
    fn default() -> Self {
        Self {
            voice_id: "21m00Tcm4TlvDq8ikWAM".to_string(),
            model_id: "eleven_monolingual_v1".to_string(),
            stability: 0.5,
            similarity_boost: 0.75,
        }
    }
}

/// Credential vault configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    /// Whether the vault is enabled (auto-detected if vault.enc exists).
    pub enabled: bool,
    /// Custom vault file path (default: ~/.aginx/carrier/vault.enc).
    pub path: Option<PathBuf>,
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: None,
        }
    }
}

/// Agent binding — routes specific channel/account/peer patterns to agents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBinding {
    /// Target agent name or ID.
    pub agent: String,
    /// Match criteria (all specified fields must match).
    pub match_rule: BindingMatchRule,
}

/// Match rule for agent bindings. All specified (non-None) fields must match.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BindingMatchRule {
    /// Channel type (e.g., "discord", "telegram", "slack").
    pub channel: Option<String>,
    /// Specific account/bot ID within the channel.
    pub account_id: Option<String>,
    /// Peer/user ID for DM routing.
    pub peer_id: Option<String>,
    /// Guild/server ID (Discord/Slack).
    pub guild_id: Option<String>,
    /// Role-based routing (user must have at least one).
    #[serde(default)]
    pub roles: Vec<String>,
}

impl BindingMatchRule {
    /// Calculate specificity score for binding priority ordering.
    /// Higher = more specific = checked first.
    pub fn specificity(&self) -> u32 {
        let mut score = 0u32;
        if self.peer_id.is_some() {
            score += 8;
        }
        if self.guild_id.is_some() {
            score += 4;
        }
        if !self.roles.is_empty() {
            score += 2;
        }
        if self.account_id.is_some() {
            score += 2;
        }
        if self.channel.is_some() {
            score += 1;
        }
        score
    }
}

/// Broadcast config — send same message to multiple agents.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BroadcastConfig {
    /// Broadcast strategy.
    pub strategy: BroadcastStrategy,
    /// Map of peer_id -> list of agent names to receive the message.
    pub routes: HashMap<String, Vec<String>>,
}

/// Broadcast delivery strategy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BroadcastStrategy {
    /// Send to all agents simultaneously.
    #[default]
    Parallel,
    /// Send to agents one at a time in order.
    Sequential,
}

/// Canvas (Agent-to-UI) configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasConfig {
    /// Enable canvas tool. Default: false.
    pub enabled: bool,
    /// Max HTML size in bytes. Default: 512KB.
    pub max_html_bytes: usize,
    /// Allowed HTML tags (empty = all safe tags allowed).
    #[serde(default)]
    pub allowed_tags: Vec<String>,
}

impl Default for CanvasConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_html_bytes: 512 * 1024,
            allowed_tags: Vec::new(),
        }
    }
}

/// Shell/exec security mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecSecurityMode {
    /// Block all shell execution.
    #[serde(alias = "none", alias = "disabled")]
    Deny,
    /// Only allow commands in safe_bins or allowed_commands.
    #[default]
    #[serde(alias = "restricted")]
    Allowlist,
    /// Allow all commands (unsafe, dev only).
    #[serde(alias = "allow", alias = "all", alias = "unrestricted")]
    Full,
}

/// Shell/exec security policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecPolicy {
    /// Security mode: "deny" blocks all, "allowlist" only allows listed,
    /// "full" allows all (unsafe, dev only).
    pub mode: ExecSecurityMode,
    /// Commands that bypass allowlist (stdin-only utilities).
    pub safe_bins: Vec<String>,
    /// Global command allowlist (when mode = allowlist).
    pub allowed_commands: Vec<String>,
    /// Max execution timeout in seconds. Default: 30.
    pub timeout_secs: u64,
    /// Max output size in bytes. Default: 100KB.
    pub max_output_bytes: usize,
    /// No-output idle timeout in seconds. When > 0, kills processes that
    /// produce no stdout/stderr output for this duration. Default: 30.
    #[serde(default = "default_no_output_timeout")]
    pub no_output_timeout_secs: u64,
}

fn default_no_output_timeout() -> u64 {
    30
}

impl Default for ExecPolicy {
    fn default() -> Self {
        Self {
            mode: ExecSecurityMode::default(),
            safe_bins: vec![
                "sleep", "true", "false", "cat", "sort", "uniq", "cut", "tr", "head", "tail", "wc",
                "date", "echo", "printf", "basename", "dirname", "pwd", "env", "pandoc",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            allowed_commands: Vec::new(),
            timeout_secs: 30,
            max_output_bytes: 100 * 1024,
            no_output_timeout_secs: default_no_output_timeout(),
        }
    }
}

// ---------------------------------------------------------------------------
// Gap 2: No-output idle timeout for subprocess sandbox
// ---------------------------------------------------------------------------

/// A whitelisted CLI command for the `cli_exec` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCommand {
    /// Binary name (e.g. "gh", "todoist", "git").
    pub name: String,
    /// Human-readable description shown to the LLM.
    pub description: String,
    /// Example subcommands/args the LLM can use.
    #[serde(default)]
    pub examples: Vec<String>,
}

/// Configuration for the `cli_exec` tool — a restricted alternative to `shell_exec`.
///
/// Unlike `shell_exec` (PermissionLevel::Dangerous), `cli_exec` only allows
/// commands explicitly listed in `commands`. Arguments are parsed with `shlex`
/// and executed directly (no shell wrapper), making it safe for low-privilege agents.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliExecConfig {
    /// Whitelisted CLI commands. Empty = cli_exec disabled (tool not registered).
    #[serde(default)]
    pub commands: Vec<CliCommand>,
}

// ---------------------------------------------------------------------------
// Gap 5: Docker sandbox maturity
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Gap 6: Typing indicator modes
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Gap 7: Thinking level support
// ---------------------------------------------------------------------------

/// Extended thinking configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThinkingConfig {
    /// Maximum tokens for thinking (budget).
    pub budget_tokens: u32,
    /// Whether to stream thinking tokens to the client.
    pub stream_thinking: bool,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            budget_tokens: 10_000,
            stream_thinking: false,
        }
    }
}

/// Hub (openclone-hub) connection configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HubConfig {
    /// Hub server URL. Default: "https://duphub.com"（hub.aginx.net 已下线）
    pub url: String,
    /// Environment variable name holding the API key (e.g. "OPENCLONE_HUB_KEY").
    /// The API key is read from this env var at runtime.
    pub api_key_env: String,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self {
            url: "https://duphub.com".to_string(),
            api_key_env: "OPENCLONE_HUB_KEY".to_string(),
        }
    }
}

/// Budget configuration for cost/usage alerts.
///
/// Tracks cumulative token usage and fires alerts at configured
/// percentage thresholds via channel messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BudgetConfig {
    /// Monthly token budget (0 = unlimited, no alerts).
    pub monthly_token_limit: u64,
    /// Alert at these percentages of the budget (e.g. [50, 80, 100]).
    pub alert_thresholds: Vec<u8>,
    /// Channel type for alert delivery (e.g. "dingtalk", "feishu", "wecom").
    pub alert_channel: Option<String>,
    /// Recipient user/tenant identifier for alert messages.
    pub alert_recipient: Option<String>,
}

/// Top-level kernel configuration.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KernelConfig {
    /// Carrier home directory (default: ~/.aginx/carrier).
    pub home_dir: PathBuf,
    /// Data directory for databases (default: ~/.aginx/carrier/data).
    pub data_dir: PathBuf,
    /// Log level (trace, debug, info, warn, error).
    pub log_level: String,
    /// API listen address (e.g., "0.0.0.0:4200").
    #[serde(alias = "listen_addr")]
    pub api_listen: String,
    /// Default LLM provider/model — DISPLAY-ONLY relic of the multi-provider
    /// era. In single-layer Brain there is no "default model" to configure:
    /// agents take their model from their own manifest / brain.json, and this
    /// field is never read into an agent manifest. API writes are rejected
    /// (config_set BLOCKED_KEYS, 2026-08-17); the field itself is kept only
    /// because the CLI compiles against it — do not add new readers.
    ///
    /// `brain` (a path to brain.json) holds the authoritative driver/endpoint
    /// config that boots the LLM subsystem.
    #[serde(default)]
    pub default_model: DefaultModelConfig,
    /// Brain configuration — the carrier's independent LLM brain.
    #[serde(default)]
    pub brain: BrainSourceConfig,
    /// Memory substrate configuration.
    pub memory: MemoryConfig,
    /// API authentication key. When set, all API endpoints (except /api/health)
    /// require a `Authorization: Bearer <key>` header.
    /// If empty, the API is unauthenticated (local development only).
    #[serde(skip_serializing)]
    pub api_key: String,
    /// Kernel operating mode (stable, default, dev).
    #[serde(default)]
    pub mode: KernelMode,
    /// Language/locale for CLI and messages (default: "en").
    #[serde(default = "default_language")]
    pub language: String,
    /// MCP server configurations for external tool integration.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfigEntry>,
    /// A2A (Agent-to-Agent) protocol configuration.
    #[serde(default)]
    pub a2a: Option<A2aConfig>,
    /// Usage footer mode (what to show after each response).
    #[serde(default)]
    pub usage_footer: UsageFooterMode,
    /// Web tools configuration (search + fetch).
    #[serde(default)]
    pub web: WebConfig,
    /// Browser automation configuration.
    #[serde(default)]
    pub browser: BrowserConfig,
    /// Credential vault configuration.
    #[serde(default)]
    pub vault: VaultConfig,
    /// Assistant packs (FS.md: `/home/workflows`). Default: `{home}/workflows`
    #[serde(default)]
    pub workflows_dir: Option<PathBuf>,
    /// Hub (openclone-hub) connection settings.
    #[serde(default)]
    pub hub: HubConfig,
    /// Media understanding configuration.
    #[serde(default)]
    pub media: crate::media::MediaConfig,
    /// Link understanding configuration.
    #[serde(default)]
    pub links: crate::media::LinkConfig,
    /// Config hot-reload settings.
    #[serde(default)]
    pub reload: ReloadConfig,
    /// Budget configuration for cost/usage alerts.
    #[serde(default)]
    pub budget: BudgetConfig,
    /// Cron scheduler max total jobs across all agents. Default: 500.
    #[serde(default = "default_max_cron_jobs")]
    pub max_cron_jobs: usize,
    /// Master switch for chained-cron stall auto-resume (断链自动接续).
    /// When true, a broken chained one-shot pipeline (step failed/timed out/
    /// degenerate, completed without scheduling a successor, or stranded by
    /// a mid-fire daemon restart) is silently re-fired from the breakpoint
    /// step up to `MAX_AUTO_RESUMES` times, then escalated to workspace
    /// admins. Default: true.
    #[serde(default = "default_chain_resume_enabled")]
    pub chain_resume_enabled: bool,
    /// Default timeout (seconds) for `user_input` flow steps without an explicit
    /// `timeout_hours`. A suspended flow past this deadline is reaped as
    /// `timed_out`. Default: 86400 (24h).
    #[serde(default = "default_user_input_timeout_secs")]
    pub user_input_timeout_secs: u64,
    /// Outer wall-clock backstop (seconds) for a single agent turn across all
    /// trigger paths (HTTP /send, channel inbound, cron, inter-agent). Applied
    /// at the `send_message_with_handle_and_blocks` chokepoint; cron jobs may
    /// override per-job via `timeout_secs` (tighter wins). This is a daemon-hang
    /// BACKSTOP only - the turn itself is governed by progress/stuck detection
    /// (tool-call repetition + no-progress idle), not a time budget. Default:
    /// 14400 (4h); set to 0 to disable the backstop entirely (rely solely on
    /// stuck/progress detection + per-LLM-call stall timeout).
    #[serde(default = "default_agent_turn_timeout_secs")]
    pub agent_turn_timeout_secs: u64,
    /// Config include files — loaded and deep-merged before the root config.
    /// Paths are relative to the root config file's directory.
    /// Security: absolute paths and `..` components are rejected.
    #[serde(default)]
    pub include: Vec<String>,
    /// Shell/exec security policy.
    #[serde(default)]
    pub exec_policy: ExecPolicy,
    /// CLI exec whitelist — restricted alternative to shell_exec for low-privilege agents.
    #[serde(default)]
    pub cli_exec: CliExecConfig,
    /// Agent bindings for multi-account routing.
    #[serde(default)]
    pub bindings: Vec<AgentBinding>,
    /// Broadcast routing configuration.
    #[serde(default)]
    pub broadcast: BroadcastConfig,
    /// Canvas (A2UI) configuration.
    #[serde(default)]
    pub canvas: CanvasConfig,
    /// Text-to-speech configuration.
    #[serde(default)]
    pub tts: TtsConfig,
    /// Extended thinking configuration.
    #[serde(default)]
    pub thinking: Option<ThinkingConfig>,
    /// OAuth client ID overrides for PKCE flows.
    #[serde(default)]
    pub oauth: OAuthConfig,
    /// Dashboard authentication (username/password login).
    #[serde(default)]
    pub auth: AuthConfig,
    /// Directory for auto-loading workflow JSON files on startup.
    /// Clone lifecycle configuration (evolution, version tracking).
    #[serde(default)]
    pub clone_lifecycle: CloneLifecycleConfig,
    /// Plugin directory for loading channel/tool plugins.
    /// Each subdirectory should contain a plugin.toml and a shared library (.so/.dylib/.dll).
    #[serde(default)]
    pub plugins_dir: Option<PathBuf>,
    /// External base URL for constructing download links (e.g. "https://carrier.yinnho.cn").
    /// When set, the kernel appends output file download URLs to agent responses.
    #[serde(default)]
    pub external_url: Option<String>,
    /// Tool whitelist — tools available to ALL agents without needing explicit declaration.
    /// Non-whitelisted tools must be declared in the agent manifest's capabilities.tools
    /// or via a ToolProfile. Default: empty (no free tools).
    #[serde(default)]
    pub whitelist_tools: Vec<String>,
    /// Max concurrent LLM requests across all agents. Default: 10.
    /// Prevents overwhelming the LLM API when many users send messages simultaneously.
    #[serde(default = "default_llm_concurrency")]
    pub llm_concurrency: usize,
    /// Per-channel permission configuration. Key is channel_type (e.g. "weixin", "feishu").
    /// Tools exceeding the channel's max_permission are filtered out before the LLM sees them.
    #[serde(default)]
    pub channels: HashMap<String, ChannelConfig>,
    /// Trusted Ed25519 public keys for manifest signature verification (hex-encoded).
    /// When empty, `verify()` is used (less secure — trusts embedded key).
    /// When non-empty, `verify_with_trust_store()` is used instead.
    #[serde(default)]
    pub trusted_signing_keys: Vec<String>,
    /// P1-C authority flip: load session history from the append-only event
    /// log (`{data_dir}/session-events/`) instead of the sessions DB table.
    /// The DB row stays as cache and identity index. Canaries per-deploy:
    /// default off; enable in config.toml to flip a deployment, then fleet.
    #[serde(default)]
    pub session_event_source: bool,
    /// 借用准入与配额：谁能借（allowlist）+ 每小时轮次上限。
    #[serde(default)]
    pub borrow: BorrowPolicyConfig,
    /// webhook HTTP 入站通道（机器→agent 事件触达）。
    #[serde(default)]
    pub webhook: WebhookConfig,
    /// 未绑定入站消息的兜底落点（系统身份「我」）。None/空 = 维持
    /// 第十二刀立法（无路由即丢弃）。兜底不写回 SenderRouter——扫码
    /// 绑定才固化路由，这里只是"别让陌生人石沉大海"。
    #[serde(default = "default_inbound_fallback_agent")]
    pub inbound_fallback_agent: Option<String>,
}

fn default_inbound_fallback_agent() -> Option<String> {
    Some("me".to_string())
}

/// webhook HTTP 入站通道（config.toml `[webhook]` 段）——分身被外部事件
/// 触达的第二条入站渠道（第一条 iLink 是人→agent，这条是机器→agent）。
///
/// - `enabled = false`（默认）：不起监听、不注册通道——移动端（uniffi 经
///   carrier lib 编译）安全，服务只在 daemon `start` 形态 spawn。
/// - `hooks`：每条 `{name, agent, token}` 绑一个分身；name 是 URL 路径段
///   兼路由键，token 是调用方凭证（Bearer / X-Webhook-Token / ?token=）。
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookConfig {
    /// 总开关：false 不起 HTTP 监听、不注册通道。
    pub enabled: bool,
    /// 监听地址；默认仅本机回环，公网暴露走 nginx 反代。
    pub listen: String,
    /// `?wait=N` 同步模式的上限秒数（请求值与其取小，clamp 1..=600）。
    pub max_wait_secs: u64,
    /// 单请求 body 上限（字节）。
    pub max_body_bytes: usize,
    /// 命名 hooks；name 限 `[a-z0-9-]`，token ≥16 字符，agent 必须已注册。
    pub hooks: Vec<WebhookHook>,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: "127.0.0.1:8702".to_string(),
            max_wait_secs: 120,
            max_body_bytes: 1024 * 1024,
            hooks: Vec::new(),
        }
    }
}

/// 单个 webhook hook：一个稳定的外部事件源 → 一个分身。
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebhookHook {
    /// hook 名：URL 路径段（`/hook/{name}`）+ 路由键 + 数据目录名。
    pub name: String,
    /// 目标分身名（kernel registry 里的 agent 名）。
    pub agent: String,
    /// 调用方凭证（恒时比较；Bearer / X-Webhook-Token / ?token= 三选一）。
    pub token: String,
}

/// token 不进日志/Debug 输出。
impl std::fmt::Debug for WebhookHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookHook")
            .field("name", &self.name)
            .field("agent", &self.agent)
            .field("token", &if self.token.is_empty() { "<empty>" } else { "<redacted>" })
            .finish()
    }
}

impl std::fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookConfig")
            .field("enabled", &self.enabled)
            .field("listen", &self.listen)
            .field("max_wait_secs", &self.max_wait_secs)
            .field("max_body_bytes", &self.max_body_bytes)
            .field("hooks", &self.hooks)
            .finish()
    }
}

/// 借用准入与配额策略（config.toml `[borrow]` 段）。
///
/// - `enabled = false` 直接拒绝一切借用轮。
/// - `allow_borrowers` 为空 = 不设名单门，任何身份可借（个人安装默认，
///   反正算力是自己的）；非空 = 只有名单内的 borrower 身份可借。
/// - `max_turns_per_hour` 按 borrower+agent 维度计数（台账
///   `{data_dir}/borrow_usage.jsonl`），0 = 不限量。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BorrowPolicyConfig {
    /// 总开关：false 时借用轮全部拒绝。
    pub enabled: bool,
    /// 允许的 borrower 身份列表；空 = 不限身份。
    pub allow_borrowers: Vec<String>,
    /// 单 borrower+agent 每小时最大轮次；0 = 不限量。
    pub max_turns_per_hour: u32,
}

impl Default for BorrowPolicyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allow_borrowers: Vec::new(),
            max_turns_per_hour: 30,
        }
    }
}

/// Per-channel configuration for tool permission filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelConfig {
    /// Maximum permission level allowed for this channel.
    /// Tools with a higher permission level are hidden from the LLM.
    /// Default: Dangerous (all tools allowed — backwards compatible).
    pub max_permission: crate::tool::PermissionLevel,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            max_permission: crate::tool::PermissionLevel::Dangerous,
        }
    }
}

/// Clone lifecycle configuration — controls post-conversation learning and knowledge evolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CloneLifecycleConfig {
    /// Enable post-conversation knowledge evolution for clones.
    /// When true, conversations with clone agents are automatically analyzed
    /// to extract new knowledge files.
    pub evolution_enabled: bool,
    /// Global master switch for autonomous self-growth (idle-time learn/create
    /// cron). When false, no clone runs self-growth regardless of its EVOLUTION.md.
    /// When true, a clone runs self-growth only if its EVOLUTION.md also sets
    /// `self_growth_enabled: true`. Default off.
    pub self_growth_enabled: bool,
}

impl Default for CloneLifecycleConfig {
    fn default() -> Self {
        Self {
            evolution_enabled: true,
            self_growth_enabled: false,
        }
    }
}

/// Dashboard authentication (username/password login).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    /// Enable username/password authentication for the dashboard.
    pub enabled: bool,
    /// Admin username.
    pub username: String,
    /// SHA256 hash of the password (hex-encoded).
    /// Generate with: carrier auth hash-password
    pub password_hash: String,
    /// Session token lifetime in hours (default: 168 = 7 days).
    pub session_ttl_hours: u64,
    /// Trusted reverse proxy IP(s) for rate limiting by real client IP.
    /// When set, x-real-ip / x-forwarded-for headers are only trusted from these IPs.
    /// When empty, those headers are always trusted (legacy behavior — secure only behind a proxy).
    #[serde(default)]
    pub trusted_proxy: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            username: "admin".to_string(),
            password_hash: String::new(),
            session_ttl_hours: 168,
            trusted_proxy: Vec::new(),
        }
    }
}

/// OAuth client ID overrides for PKCE flows.
///
/// Configure in config.toml:
/// ```toml
/// [oauth]
/// google_client_id = "your-google-client-id"
/// github_client_id = "your-github-client-id"
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuthConfig {
    /// Google OAuth2 client ID for PKCE flow.
    pub google_client_id: Option<String>,
    /// GitHub OAuth client ID for PKCE flow.
    pub github_client_id: Option<String>,
    /// Microsoft (Entra ID) OAuth client ID.
    pub microsoft_client_id: Option<String>,
    /// Slack OAuth client ID.
    pub slack_client_id: Option<String>,
}

fn default_max_cron_jobs() -> usize {
    500
}

fn default_chain_resume_enabled() -> bool {
    true
}

fn default_user_input_timeout_secs() -> u64 {
    86_400
}

fn default_agent_turn_timeout_secs() -> u64 {
    14_400
}

fn default_llm_concurrency() -> usize {
    10
}

/// Configuration entry for an MCP server.
///
/// This is the config.toml representation. The runtime `McpServerConfig`
/// struct is constructed from this during kernel boot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfigEntry {
    /// Display name for this server.
    pub name: String,
    /// Brief description of what this MCP server does (shown to LLM in system prompt).
    #[serde(default)]
    pub description: String,
    /// Transport configuration.
    pub transport: McpTransportEntry,
    /// Request timeout in seconds.
    #[serde(default = "default_mcp_timeout")]
    pub timeout_secs: u64,
    /// Environment variables to pass through (e.g., ["GITHUB_PERSONAL_ACCESS_TOKEN"]).
    #[serde(default)]
    pub env: Vec<String>,
}

fn default_mcp_timeout() -> u64 {
    30
}

/// Transport configuration for an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransportEntry {
    /// Subprocess with JSON-RPC over stdin/stdout.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// HTTP Server-Sent Events.
    Sse { url: String },
}

/// A2A (Agent-to-Agent) protocol configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct A2aConfig {
    /// Whether A2A is enabled.
    pub enabled: bool,
    /// Path to serve A2A endpoints (default: "/a2a").
    #[serde(default = "default_a2a_path")]
    pub listen_path: String,
    /// External A2A agents to connect to.
    #[serde(default)]
    pub external_agents: Vec<ExternalAgent>,
}

fn default_a2a_path() -> String {
    "/a2a".to_string()
}

/// An external A2A agent to discover and interact with.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalAgent {
    /// Display name.
    pub name: String,
    /// Agent endpoint URL.
    pub url: String,
}

fn default_language() -> String {
    "en".to_string()
}

impl Default for KernelConfig {
    fn default() -> Self {
        let home = home_dir();
        Self {
            data_dir: home.join("data"),
            home_dir: home,
            log_level: "info".to_string(),
            api_listen: "127.0.0.1:50051".to_string(),
            default_model: DefaultModelConfig::default(),
            brain: BrainSourceConfig::default(),
            memory: MemoryConfig::default(),
            api_key: String::new(),
            mode: KernelMode::default(),
            language: "en".to_string(),
            mcp_servers: Vec::new(),
            a2a: None,
            usage_footer: UsageFooterMode::default(),
            web: WebConfig::default(),
            browser: BrowserConfig::default(),
            vault: VaultConfig::default(),
            workflows_dir: None,
            hub: HubConfig::default(),
            media: crate::media::MediaConfig::default(),
            links: crate::media::LinkConfig::default(),
            reload: ReloadConfig::default(),
            budget: BudgetConfig::default(),
            max_cron_jobs: default_max_cron_jobs(),
            chain_resume_enabled: default_chain_resume_enabled(),
            user_input_timeout_secs: default_user_input_timeout_secs(),
            agent_turn_timeout_secs: default_agent_turn_timeout_secs(),
            include: Vec::new(),
            exec_policy: ExecPolicy::default(),
            cli_exec: CliExecConfig::default(),
            bindings: Vec::new(),
            broadcast: BroadcastConfig::default(),
            canvas: CanvasConfig::default(),
            tts: TtsConfig::default(),
            thinking: None,
            oauth: OAuthConfig::default(),
            auth: AuthConfig::default(),
            clone_lifecycle: CloneLifecycleConfig::default(),
            plugins_dir: None,
            external_url: None,
            whitelist_tools: Vec::new(),
            llm_concurrency: default_llm_concurrency(),
            channels: HashMap::new(),
            trusted_signing_keys: Vec::new(),
            session_event_source: false,
            borrow: BorrowPolicyConfig::default(),
            inbound_fallback_agent: default_inbound_fallback_agent(),
            webhook: WebhookConfig::default(),
        }
    }
}

impl KernelConfig {
    /// Resolved workflows root directory.
    pub fn effective_workflows_dir(&self) -> PathBuf {
        self.workflows_dir
            .clone()
            .unwrap_or_else(|| self.home_dir.join("workflows"))
    }

    /// Workspace directory for one agent by name. The mother ("me") lives at
    /// the home root itself — her persona IS the home tree, not an assistant
    /// folder (docs/FS.md: 不要再做 workflows/me). Every other agent is a
    /// directory under the workflows root.
    pub fn agent_workspace_dir(&self, name: &str) -> PathBuf {
        if name == SYSTEM_AGENT_ME {
            self.home_dir.clone()
        } else {
            self.effective_workflows_dir().join(name)
        }
    }
}

/// System identity agent name — the mother. Kept here (not in carrier-clone)
/// so KernelConfig can special-case her workspace without a reverse dep.
pub const SYSTEM_AGENT_ME: &str = "me";

/// SECURITY: Custom Debug impl redacts sensitive fields (api_key).
impl std::fmt::Debug for KernelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KernelConfig")
            .field("home_dir", &self.home_dir)
            .field("data_dir", &self.data_dir)
            .field("log_level", &self.log_level)
            .field("api_listen", &self.api_listen)
            .field("default_model", &self.default_model)
            .field("memory", &self.memory)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("mode", &self.mode)
            .field("language", &self.language)
            .field(
                "mcp_servers",
                &format!("{} server(s)", self.mcp_servers.len()),
            )
            .field("a2a", &self.a2a.as_ref().map(|a| a.enabled))
            .field("usage_footer", &self.usage_footer)
            .field("web", &self.web)
            .field("browser", &self.browser)
            .field("vault", &format!("enabled={}", self.vault.enabled))
            .field("workflows_dir", &self.workflows_dir)
            .field("hub", &format!("url={}", self.hub.url))
            .field(
                "media",
                &format!(
                    "image={} audio={} video={}",
                    self.media.image_description,
                    self.media.audio_transcription,
                    self.media.video_description
                ),
            )
            .field("links", &format!("enabled={}", self.links.enabled))
            .field("reload", &self.reload.mode)
            .field("max_cron_jobs", &self.max_cron_jobs)
            .field("chain_resume_enabled", &self.chain_resume_enabled)
            .field("include", &format!("{} file(s)", self.include.len()))
            .field("exec_policy", &self.exec_policy.mode)
            .field("bindings", &format!("{} binding(s)", self.bindings.len()))
            .field(
                "broadcast",
                &format!("{} route(s)", self.broadcast.routes.len()),
            )
            .field("canvas", &format!("enabled={}", self.canvas.enabled))
            .field("tts", &format!("enabled={}", self.tts.enabled))
            .field("thinking", &self.thinking.is_some())
            .field("auth", &format!("enabled={}", self.auth.enabled))
            .finish()
    }
}

/// Resolve the mother home directory (AginxOS `AGINX_HOME`).
///
/// Priority: `AGINX_HOME` > `AGINX_CARRIER_HOME` > `/home`.
/// On a phone there is no second user; `/home` is the mother. Host
/// debug must set `AGINX_HOME` (do not hide a `.aginx` folder on device).
pub fn home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("AGINX_HOME") {
        return PathBuf::from(home);
    }
    if let Ok(home) = std::env::var("AGINX_CARRIER_HOME") {
        return PathBuf::from(home);
    }
    PathBuf::from("/home")
}

/// Resolve the per-sender-per-agent data directory under `workflows/`.
///
/// Returns `workflows/{agent_name}/senders/{owner_id}/` or
/// `workflows/{agent_name}/senders/{owner_id}/users/{user_id}/` when user_id differs from owner_id.
///
/// - `owner_id` is the route_key: for WeChat it's the openid, for WeCom/Feishu/DingTalk it's the bot_id/app_id/app_key.
/// - `user_id` is the actual user identity from the platform message. When present and different
///   from `owner_id`, the path becomes `workflows/{agent_name}/senders/{owner_id}/users/{user_id}/`
///   (group users under a bot). When `None` or equal to `owner_id`, the path is
///   `workflows/{agent_name}/senders/{owner_id}/` (the owner's own data).
pub fn sender_data_dir(
    home_dir: &std::path::Path,
    owner_id: &str,
    agent_name: &str,
    user_id: Option<&str>,
) -> PathBuf {
    let safe_owner = sanitize_path_component(owner_id);
    let safe_agent = sanitize_path_component(agent_name);
    let base = home_dir
        .join("workflows")
        .join(safe_agent)
        .join("senders")
        .join(safe_owner);
    match user_id {
        Some(uid) if uid != owner_id => base.join("users").join(sanitize_path_component(uid)),
        _ => base,
    }
}

/// Compute the shell working directory for an agent turn.
///
/// When a sender is present (per-user channel), returns the sender-scoped data
/// directory so `shell_exec` and `file_write` land in the same directory
/// (byte-aligned). Without a sender (CLI/system turns), falls back to the
/// workspace root. This eliminates duplicated cwd resolution in the kernel
/// and runtime layers — both call this single function.
///
/// The returned path is a subdirectory of the workspace:
/// `workflows/{agent}/senders/{owner}/` (sender-driven) or
/// `workspace_root/` (CLI/system).
pub fn resolve_turn_cwd(
    home_dir: &std::path::Path,
    workspace_root: &std::path::Path,
    agent_name: &str,
    sender_id: Option<&str>,
    owner_id: Option<&str>,
) -> std::path::PathBuf {
    match sender_id {
        Some(s) => {
            let owner = owner_id.unwrap_or(s);
            sender_data_dir(home_dir, owner, agent_name, Some(s))
        }
        None => workspace_root.to_path_buf(),
    }
}

/// Compute a home-relative path under `workflows/` for a given subdir (input/output/memory).
///
/// Returns a string like `workflows/{agent_name}/senders/{owner_id}/{subdir}` or
/// `workflows/{agent_name}/senders/{owner_id}/users/{user_id}/{subdir}` when user_id differs from owner_id.
pub fn sender_relative_path(
    owner_id: &str,
    agent_name: &str,
    user_id: Option<&str>,
    subdir: &str,
) -> String {
    let safe_owner = sanitize_path_component(owner_id);
    let safe_agent = sanitize_path_component(agent_name);
    let base = format!("workflows/{}/senders/{}", safe_agent, safe_owner);
    match user_id {
        Some(uid) if uid != owner_id => {
            format!("{}/users/{}/{}", base, sanitize_path_component(uid), subdir)
        }
        _ => format!("{}/{}", base, subdir),
    }
}

/// Sanitize a path component to prevent directory traversal.
/// Returns "_" for empty/unsafe values.
pub fn sanitize_path_component(s: &str) -> &str {
    if s.is_empty() || s.contains('/') || s.contains('\\') || s.contains("..") {
        "_"
    } else {
        s
    }
}

/// Default LLM model configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DefaultModelConfig {
    /// Model identifier.
    pub model: String,
    /// Environment variable name for the API key.
    pub api_key_env: String,
    /// Optional base URL override.
    pub base_url: Option<String>,
}

impl Default for DefaultModelConfig {
    fn default() -> Self {
        Self {
            model: "claude-sonnet-4-20250514".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: None,
        }
    }
}

/// Brain source configuration — tells the carrier where to load brain.json from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrainSourceConfig {
    /// Path to brain.json, relative to home_dir. Default: "brain.json".
    pub config: String,
}

impl Default for BrainSourceConfig {
    fn default() -> Self {
        Self {
            config: "brain.json".to_string(),
        }
    }
}

/// Memory substrate configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Path to SQLite database file.
    pub sqlite_path: Option<PathBuf>,
    /// Maximum memories before consolidation is triggered.
    pub consolidation_threshold: u64,
    /// How often to run memory consolidation (hours). 0 = disabled.
    #[serde(default = "default_consolidation_interval")]
    pub consolidation_interval_hours: u64,
    /// Tree memory configuration (hierarchical memory system).
    #[serde(default)]
    pub tree: TreeMemoryConfig,
}

/// Configuration for the hierarchical tree memory system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeMemoryConfig {
    /// Root directory for tree memory content files (Obsidian-compatible .md).
    /// Default: {data_dir}/memory_tree/content
    #[serde(default)]
    pub content_root: Option<String>,
    /// Number of background worker tasks for tree jobs.
    #[serde(default = "default_tree_worker_count")]
    pub worker_count: usize,
    /// Poll interval in seconds for the job queue.
    #[serde(default = "default_tree_poll_interval")]
    pub poll_interval_secs: u64,
    /// Token budget for L0 buffer before sealing.
    #[serde(default = "default_tree_input_token_budget")]
    pub input_token_budget: u32,
    /// Number of summaries to fan-out at each level.
    #[serde(default = "default_tree_summary_fanout")]
    pub summary_fanout: u32,
    /// Age in seconds after which stale buffers are force-sealed.
    #[serde(default = "default_tree_flush_age")]
    pub flush_age_secs: i64,
    /// Score threshold below which chunks are dropped.
    #[serde(default = "default_tree_drop_threshold")]
    pub drop_threshold: f32,
    /// Maximum tokens per chunk.
    #[serde(default = "default_tree_chunk_max_tokens")]
    pub chunk_max_tokens: u32,
    /// LLM model for summarisation. Empty = inert (concat-only).
    #[serde(default)]
    pub summariser_model: Option<String>,
    /// Hotness threshold for entity topic tree materialisation.
    #[serde(default = "default_tree_topic_hotness")]
    pub topic_hotness_threshold: f32,
}

fn default_tree_worker_count() -> usize {
    4
}
fn default_tree_poll_interval() -> u64 {
    5
}
fn default_tree_input_token_budget() -> u32 {
    50_000
}
fn default_tree_summary_fanout() -> u32 {
    10
}
fn default_tree_flush_age() -> i64 {
    604_800
}
fn default_tree_drop_threshold() -> f32 {
    0.3
}
fn default_tree_chunk_max_tokens() -> u32 {
    3_000
}
fn default_tree_topic_hotness() -> f32 {
    3.0
}

impl Default for TreeMemoryConfig {
    fn default() -> Self {
        Self {
            content_root: None,
            worker_count: default_tree_worker_count(),
            poll_interval_secs: default_tree_poll_interval(),
            input_token_budget: default_tree_input_token_budget(),
            summary_fanout: default_tree_summary_fanout(),
            flush_age_secs: default_tree_flush_age(),
            drop_threshold: default_tree_drop_threshold(),
            chunk_max_tokens: default_tree_chunk_max_tokens(),
            summariser_model: None,
            topic_hotness_threshold: default_tree_topic_hotness(),
        }
    }
}

fn default_consolidation_interval() -> u64 {
    24
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            sqlite_path: None,
            consolidation_threshold: 10_000,
            consolidation_interval_hours: default_consolidation_interval(),
            tree: TreeMemoryConfig::default(),
        }
    }
}

impl KernelConfig {
    /// Validate the configuration, returning a list of warnings.
    pub fn validate(&self) -> Vec<String> {
        let mut warnings = Vec::new();

        // API listen address
        if !self.api_listen.is_empty() && !self.api_listen.contains(':') {
            warnings.push(format!(
                "api_listen '{}' may be missing a port",
                self.api_listen
            ));
        }

        // Numeric bounds
        if self.max_cron_jobs == 0 {
            warnings.push("max_cron_jobs is 0 — cron jobs will be disabled".to_string());
        }
        if self.llm_concurrency == 0 {
            warnings.push("llm_concurrency is 0 — no LLM requests will be allowed".to_string());
        }
        if self.max_cron_jobs > 10000 {
            warnings.push(format!(
                "max_cron_jobs={} is very high — may impact performance",
                self.max_cron_jobs
            ));
        }

        // Hub URL
        if !self.hub.url.is_empty()
            && !self.hub.url.starts_with("https://")
            && !self.hub.url.starts_with("http://")
        {
            warnings.push(format!("hub.url '{}' is not HTTP(S)", self.hub.url));
        }

        warnings
    }

    /// Clamp configuration values to safe production bounds.
    ///
    /// Called after loading config to prevent zero timeouts, unbounded buffers,
    /// or other misconfigurations that cause silent failures at runtime.
    pub fn clamp_bounds(&mut self) {
        // Browser timeout: min 5s, max 300s
        if self.browser.timeout_secs == 0 {
            self.browser.timeout_secs = 30;
        } else if self.browser.timeout_secs > 300 {
            self.browser.timeout_secs = 300;
        }

        // Browser max sessions: min 1, max 100
        if self.browser.max_sessions == 0 {
            self.browser.max_sessions = 3;
        } else if self.browser.max_sessions > 100 {
            self.browser.max_sessions = 100;
        }

        // Web fetch max_response_bytes: min 1KB, max 50MB
        if self.web.fetch.max_response_bytes == 0 {
            self.web.fetch.max_response_bytes = 5_000_000;
        } else if self.web.fetch.max_response_bytes > 50_000_000 {
            self.web.fetch.max_response_bytes = 50_000_000;
        }

        // Web fetch timeout: min 5s, max 120s
        if self.web.fetch.timeout_secs == 0 {
            self.web.fetch.timeout_secs = 30;
        } else if self.web.fetch.timeout_secs > 120 {
            self.web.fetch.timeout_secs = 120;
        }

        // Exec timeout: min 1s
        if self.exec_policy.timeout_secs == 0 {
            self.exec_policy.timeout_secs = 30;
        }
        if self.exec_policy.no_output_timeout_secs == 0 {
            self.exec_policy.no_output_timeout_secs = 30;
        }

        // Browser idle timeout: min 1s
        if self.browser.idle_timeout_secs == 0 {
            self.browser.idle_timeout_secs = 300;
        }

        // TTS timeout: min 1s
        if self.tts.timeout_secs == 0 {
            self.tts.timeout_secs = 30;
        }

        // user_input flow timeout: min 1h (fall back to default), max 30 days
        if self.user_input_timeout_secs == 0 {
            self.user_input_timeout_secs = 86_400;
        } else if self.user_input_timeout_secs > 2_592_000 {
            self.user_input_timeout_secs = 2_592_000;
        }

        // agent turn timeout: 0 means "no backstop" (rely solely on stuck/
        // progress detection + per-LLM-call stall timeout) - leave it as 0.
        // Any positive value is the daemon-hang backstop in seconds.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = KernelConfig::default();
        assert_eq!(config.log_level, "info");
        assert_eq!(config.api_listen, "127.0.0.1:50051");
    }

    #[test]
    fn test_agent_turn_timeout_default_and_zero() {
        let config = KernelConfig::default();
        assert_eq!(config.agent_turn_timeout_secs, 14_400);
        // 0 means "no backstop" - clamp_bounds preserves it (does NOT restore
        // the default), so operators can opt out of the wall-clock backstop.
        let mut config = config;
        config.agent_turn_timeout_secs = 0;
        config.clamp_bounds();
        assert_eq!(config.agent_turn_timeout_secs, 0);
    }

    #[test]
    fn test_config_serialization() {
        let config = KernelConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("log_level"));
    }

    #[test]
    fn test_kernel_mode_default() {
        let mode = KernelMode::default();
        assert_eq!(mode, KernelMode::Default);
    }

    #[test]
    fn test_kernel_mode_serde() {
        let stable = KernelMode::Stable;
        let json = serde_json::to_string(&stable).unwrap();
        assert_eq!(json, "\"stable\"");
        let back: KernelMode = serde_json::from_str(&json).unwrap();
        assert_eq!(back, KernelMode::Stable);
    }

    #[test]
    fn test_config_with_mode_and_language() {
        let config = KernelConfig {
            mode: KernelMode::Stable,
            language: "ar".to_string(),
            ..Default::default()
        };
        assert_eq!(config.mode, KernelMode::Stable);
        assert_eq!(config.language, "ar");
    }

    #[test]
    fn test_clamp_bounds_zero_browser_timeout() {
        let mut config = KernelConfig::default();
        config.browser.timeout_secs = 0;
        config.clamp_bounds();
        assert_eq!(config.browser.timeout_secs, 30);
    }

    #[test]
    fn test_clamp_bounds_excessive_browser_sessions() {
        let mut config = KernelConfig::default();
        config.browser.max_sessions = 999;
        config.clamp_bounds();
        assert_eq!(config.browser.max_sessions, 100);
    }

    #[test]
    fn test_clamp_bounds_zero_fetch_bytes() {
        let mut config = KernelConfig::default();
        config.web.fetch.max_response_bytes = 0;
        config.clamp_bounds();
        assert_eq!(config.web.fetch.max_response_bytes, 5_000_000);
    }

    #[test]
    fn test_clamp_bounds_zero_fetch_timeout() {
        let mut config = KernelConfig::default();
        config.web.fetch.timeout_secs = 0;
        config.clamp_bounds();
        assert_eq!(config.web.fetch.timeout_secs, 30);
    }

    #[test]
    fn test_clamp_bounds_defaults_unchanged() {
        let mut config = KernelConfig::default();
        let browser_timeout = config.browser.timeout_secs;
        let browser_sessions = config.browser.max_sessions;
        let fetch_bytes = config.web.fetch.max_response_bytes;
        let fetch_timeout = config.web.fetch.timeout_secs;
        config.clamp_bounds();
        assert_eq!(config.browser.timeout_secs, browser_timeout);
        assert_eq!(config.browser.max_sessions, browser_sessions);
        assert_eq!(config.web.fetch.max_response_bytes, fetch_bytes);
        assert_eq!(config.web.fetch.timeout_secs, fetch_timeout);
    }

    #[test]
    fn webhook_config_parses_and_defaults() {
        // 显式配置解析。
        let toml_str = r#"
[webhook]
enabled = true
listen = "0.0.0.0:8702"

[[webhook.hooks]]
name = "ci"
agent = "travel-planner"
token = "tok-0123456789abcdef"
"#;
        let cfg: KernelConfig = toml::from_str(toml_str).unwrap();
        assert!(cfg.webhook.enabled);
        assert_eq!(cfg.webhook.listen, "0.0.0.0:8702");
        assert_eq!(cfg.webhook.max_wait_secs, 120); // 默认值落位
        assert_eq!(cfg.webhook.hooks.len(), 1);
        assert_eq!(cfg.webhook.hooks[0].name, "ci");
        assert_eq!(cfg.webhook.hooks[0].agent, "travel-planner");
        // 空配置：disabled 默认（移动端安全）。
        let empty: KernelConfig = toml::from_str("").unwrap();
        assert!(!empty.webhook.enabled);
        assert_eq!(empty.webhook.listen, "127.0.0.1:8702");
        assert!(empty.webhook.hooks.is_empty());
        // token 不进 Debug。
        assert!(!format!("{:?}", cfg.webhook.hooks[0]).contains("tok-0123456789abcdef"));
    }
}
