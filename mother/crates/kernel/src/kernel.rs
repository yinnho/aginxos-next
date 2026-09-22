//! CarrierKernel — assembles all subsystems and provides the main API.

use crate::background::BackgroundExecutor;
use crate::brain::Brain;
use crate::capabilities::{manifest_to_capabilities, CapabilityManager};
use crate::config::load_config;
use crate::error::{KernelError, KernelResult};
use crate::event_bus::EventBus;
use crate::metering::MeteringEngine;
use crate::prompt_sources::read_identity_file;
use crate::registry::AgentRegistry;
use crate::scheduler::AgentScheduler;
use crate::supervisor::Supervisor;
use crate::workspace::{ensure_workspace, generate_identity_files};
use carrier_memory::MemorySubstrate;
use carrier_runtime::audit::AuditLog;
use carrier_runtime::sandbox::WasmSandbox;
use carrier_types::agent::*;
use carrier_types::config::KernelConfig;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::ToolDefinition;

use std::path::Path;
use std::sync::{Arc, OnceLock, Weak};
use tracing::{info, warn};

/// LLM brain subsystem.
pub struct KernelBrain {
    /// The carrier's independent LLM brain. Always loaded — boot fails without a valid brain.json.
    /// Wrapped in RwLock to allow hot-reload of brain.json at runtime.
    pub(crate) brain: Arc<std::sync::RwLock<Arc<Brain>>>,
    /// Path to brain.json (saved at boot for hot-reload writes).
    pub(crate) brain_path: std::path::PathBuf,
}

/// A2A (Agent-to-Agent) communication subsystem.
pub struct KernelA2a {
    /// A2A task store for tracking task lifecycle.
    pub a2a_task_store: carrier_runtime::a2a::A2aTaskStore,
    /// Discovered external A2A agent cards with discovery timestamp.
    pub a2a_external_agents:
        std::sync::Mutex<Vec<(String, carrier_runtime::a2a::AgentCard, std::time::Instant)>>,
}

impl KernelA2a {
    /// Remove external agent entries that have been stale for longer than the
    /// given TTL. This prevents stale / unreachable agents from accumulating
    /// in the discovery store.
    pub fn cleanup_stale_agents(&self) {
        const STALE_TTL_SECS: u64 = 600; // 10 minutes
        if let Ok(mut agents) = self.a2a_external_agents.lock() {
            let now = std::time::Instant::now();
            agents.retain(|(_, _, discovered_at)| {
                now.duration_since(*discovered_at).as_secs() < STALE_TTL_SECS
            });
        }
    }
}

/// External service integrations (media, TTS, embeddings).
/// （M31 D3 批1：web fetch 引擎已随 aginx-web CLI 外置成包，不再常驻 kernel。）
pub struct KernelServices {
    /// Media understanding engine (image description, audio transcription).
    pub media_engine: carrier_runtime::media_understanding::MediaEngine,
}

/// Plugin and MCP tooling subsystem.
pub struct KernelPlugins {
    /// MCP server connections keyed by normalized server name.
    /// DashMap allows concurrent tool calls to different servers without blocking each other.
    pub mcp_connections: Arc<dashmap::DashMap<String, carrier_runtime::mcp::McpConnection>>,
    /// MCP tool definitions cache (populated after connections are established).
    pub mcp_tools: std::sync::Mutex<Vec<ToolDefinition>>,
    /// Toolset registry: name -> tool definitions for that toolset.
    pub toolset_registry: std::sync::RwLock<std::collections::HashMap<String, Vec<ToolDefinition>>>,
    /// Configured MCP server list (from config, used for MCP connections).
    pub effective_mcp_servers: std::sync::RwLock<Vec<carrier_types::config::McpServerConfigEntry>>,
    /// Plugin tool dispatcher — routes plugin tool calls to loaded shared libraries.
    pub plugin_tool_dispatcher:
        std::sync::Mutex<Option<Arc<carrier_runtime::plugin::tool_dispatch::PluginToolDispatcher>>>,
    /// Per-server consecutive reconnection failure count for exponential backoff.
    /// Key: normalized server name, Value: failure count.
    pub mcp_reconnect_failures: dashmap::DashMap<String, u32>,
}

/// Agent scheduling, supervision, and runtime execution subsystem.
pub struct KernelRuntime {
    /// Agent scheduler.
    pub scheduler: AgentScheduler,
    /// Process supervisor.
    pub supervisor: Supervisor,
    /// Background agent executor.
    pub background: BackgroundExecutor,
    /// Tracks running agent tasks for cancellation support.
    pub running_tasks: dashmap::DashMap<AgentId, tokio::task::AbortHandle>,
    /// WASM sandbox engine (shared across all WASM agent executions).
    pub(crate) wasm_sandbox: WasmSandbox,
    /// Per-(agent, owner) message locks — serializes LLM calls for the same agent+owner
    /// Concurrency limit for LLM requests — prevents overwhelming the API.
    pub(crate) llm_concurrency_limit: Arc<tokio::sync::Semaphore>,
    /// File watcher handles for clone agents (stopped when dropped).
    pub(crate) watcher_handles: std::sync::Mutex<Vec<carrier_lifecycle::watcher::WatcherHandle>>,
}

/// Cross-cutting coordination: capabilities, events, bindings, hooks, and process management.
pub struct KernelCoordination {
    /// Capability manager.
    pub capabilities: CapabilityManager,
    /// Event bus.
    pub event_bus: EventBus,
    /// Agent bindings for multi-account routing (Mutex for runtime add/remove).
    pub bindings: std::sync::Mutex<Vec<carrier_types::config::AgentBinding>>,
    /// Broadcast configuration.
    pub broadcast: carrier_types::config::BroadcastConfig,
    /// Plugin lifecycle hook registry.
    pub hooks: carrier_runtime::hooks::HookRegistry,
    /// Persistent process manager for interactive sessions (REPLs, servers).
    pub process_manager: Arc<carrier_runtime::process_manager::ProcessManager>,
    /// Boot timestamp for uptime calculation.
    pub booted_at: std::time::Instant,
    /// Weak self-reference for trigger dispatch (set after Arc wrapping).
    pub(crate) self_handle: OnceLock<Weak<CarrierKernel>>,
}

/// A probe that returns whether a given channel type supports proactive push.
pub type ChannelProactivePushFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The main Carrier kernel — coordinates all subsystems.
pub struct CarrierKernel {
    /// Kernel configuration.
    pub config: KernelConfig,
    /// Agent registry.
    pub registry: AgentRegistry,
    /// Memory substrate.
    pub memory: Arc<MemorySubstrate>,
    /// Merkle hash chain audit trail.
    pub audit_log: Arc<AuditLog>,
    /// Per-(agent, sender-label) turn gate: serialize same-sender turns and
    /// coalesce rapid-fire messages into one claim (dsh inbox+claim at turn
    /// granularity). See `sender_gate`.
    pub sender_gate: crate::sender_gate::SenderGate,
    /// Cost metering engine.
    pub metering: Arc<MeteringEngine>,
    /// Cron job scheduler.
    pub cron_scheduler: crate::cron::CronScheduler,

    /// Channel send function: (channel_type, bot_id, user_id, text) → Result.
    /// Wired up by the API server after the ChannelManager starts. Used by
    /// cron delivery to send notifications back to users.
    pub channel_send_fn: std::sync::RwLock<Option<carrier_runtime::plugin::bridge::ChannelSendFn>>,
    /// Channel deliver function: (channel_type, bot_id, user_id, content) -> Result.
    /// Wired up alongside channel_send_fn. Backs `[DELIVER:key]` marker handling
    /// and script/no-agent rich-content delivery.
    pub channel_deliver_fn:
        std::sync::RwLock<Option<carrier_runtime::plugin::bridge::ChannelDeliverFn>>,
    /// Channel proactive-push capability probe: channel_type → bool.
    /// Wired up alongside channel_send_fn.
    pub channel_supports_proactive_fn: std::sync::RwLock<Option<ChannelProactivePushFn>>,

    /// LLM brain.
    pub brain: KernelBrain,
    /// A2A communication subsystem.
    pub a2a: KernelA2a,
    /// External service integrations.
    pub services: KernelServices,
    /// Plugin and MCP tooling.
    pub plugins: KernelPlugins,
    /// Scheduling, supervision, and runtime.
    pub runtime: KernelRuntime,
    /// Coordination: capabilities, events, bindings, hooks, processes.
    pub coordination: KernelCoordination,
}

// ── Internal boot helpers ──────────────────────────────────

impl CarrierKernel {
    /// Fetch brain configuration from Hub (blocking wrapper).
    fn fetch_brain_from_hub(
        hub: &carrier_types::config::HubConfig,
        brain_path: &std::path::Path,
    ) -> CarrierResult<carrier_types::brain::BrainConfig> {
        let api_key = std::env::var(&hub.api_key_env).map_err(|_| {
            CarrierError::Config(format!("Environment variable {} not set", hub.api_key_env))
        })?;

        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| CarrierError::Internal(format!("Failed to create tokio runtime: {e}")))?;
        let json_value = rt
            .block_on(carrier_clone::hub::fetch_brain_config(&hub.url, &api_key))
            .map_err(|e| CarrierError::Internal(format!("Hub brain config fetch failed: {e}")))?;

        let json_str = serde_json::to_string(&json_value).map_err(|e| {
            CarrierError::Internal(format!("Failed to serialize brain config: {e}"))
        })?;

        let config: carrier_types::brain::BrainConfig = serde_json::from_str(&json_str)
            .map_err(|e| CarrierError::Internal(format!("Invalid brain config from Hub: {e}")))?;

        if let Some(parent) = brain_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(brain_path, &json_str)
            .map_err(|e| CarrierError::Internal(format!("Failed to write brain.json: {e}")))?;

        Ok(config)
    }

    /// Run post-conversation evolution for clone agents (background, non-blocking).
    ///
    /// Checks if evolution is enabled, the agent is a clone (empty system_prompt),
    /// and the conversation is non-trivial. If so, spawns a background task that:
    /// 1. Calls `should_skip()` for local filtering
    /// 2. Sends the conversation to LLM for analysis
    /// 3. Parses the response and writes knowledge files
    pub fn maybe_run_evolution(
        &self,
        manifest: &carrier_types::agent::AgentManifest,
        user_msg: &str,
        response: &str,
        owner_id: Option<&str>,
        sender_id: Option<&str>,
    ) {
        // Check config + clone mode
        if !self.config.clone_lifecycle.evolution_enabled {
            return;
        }
        let Some(ref workspace) = manifest.workspace else {
            return;
        };
        // Clone mode: empty system_prompt signals dynamic assembly
        if !manifest.model.system_prompt.is_empty() {
            return;
        }
        // Check per-clone evolution config (EVOLUTION.md)
        let evo_config = carrier_lifecycle::evolution_config::read_evolution_config(workspace);
        let knowledge_count = std::fs::read_dir(workspace.join("knowledge"))
            .map(|d| d.count())
            .unwrap_or(0);
        if !carrier_lifecycle::evolution_config::should_evolve(&evo_config, knowledge_count) {
            return;
        }
        // Local pre-filter
        if carrier_lifecycle::evolution::should_skip(user_msg, response) {
            return;
        }

        let workspace = workspace.clone();
        let user_msg = user_msg.to_string();
        let response = response.to_string();
        let clone_name = manifest.name.clone();
        let owner_id_owned = owner_id.map(|s| s.to_string());
        let sender_id_owned = sender_id.map(|s| s.to_string());
        let home_dir = self.config.home_dir.clone();
        let feedback_to_hub = evo_config.feedback_to_hub;
        let hub_url = self.config.hub.url.clone();
        let hub_api_key =
            carrier_clone::hub::read_api_key(&self.config.hub.api_key_env).unwrap_or_default();
        let driver = match self.resolve_driver(manifest) {
            Ok(d) => d,
            Err(_) => return,
        };
        let memory_md = read_identity_file(&workspace, "MEMORY.md");

        tokio::spawn(async move {
            let prompt = carrier_lifecycle::evolution::build_analysis_prompt();
            let memory_index = memory_md.unwrap_or_default();
            let mem_preview = if memory_index.len() > 2000 {
                format!("{}...(省略)", &memory_index[..2000])
            } else {
                memory_index
            };
            let resp_preview = if response.len() > 4000 {
                format!("{}...(截断)", &response[..4000])
            } else {
                response.clone()
            };
            let user_prompt = format!(
                "已知知识索引：\n{}\n\n---\n\n对话：\n用户: {}\n\n助手: {}",
                mem_preview, user_msg, resp_preview
            );

            let request = carrier_runtime::llm_driver::CompletionRequest {
                model: String::new(), // driver uses its default
                messages: vec![carrier_types::message::Message {
                    role: carrier_types::message::Role::User,
                    content: carrier_types::message::MessageContent::Text(user_prompt),
                }],
                tools: vec![],
                max_tokens: 2048,
                temperature: 0.3,
                system: Some(prompt),
                thinking: None,
                extra: Default::default(),
            };

            match tokio::time::timeout(std::time::Duration::from_secs(60), driver.complete(request))
                .await
            {
                Ok(Ok(completion)) => {
                    let text = completion.text();
                    match carrier_lifecycle::evolution::parse_analysis_response(&text) {
                        Ok(analysis) => {
                            let saved = carrier_lifecycle::evolution::apply_evolution(
                                &workspace,
                                &analysis,
                                owner_id_owned.as_deref(),
                                sender_id_owned.as_deref(),
                                Some(&home_dir),
                            );
                            if !saved.is_empty() {
                                tracing::info!(
                                    count = saved.len(),
                                    "Evolution: new knowledge extracted"
                                );
                            }

                            // Feedback pipeline — anonymize and push to Hub
                            if feedback_to_hub && !analysis.knowledge.is_empty() {
                                for candidate in &analysis.knowledge {
                                    let (sys, user) =
                                        carrier_lifecycle::feedback::build_anonymize_prompt(
                                            &candidate.title,
                                            &candidate.content,
                                        );
                                    let anon_req = carrier_runtime::llm_driver::CompletionRequest {
                                        model: String::new(),
                                        messages: vec![carrier_types::message::Message {
                                            role: carrier_types::message::Role::User,
                                            content: carrier_types::message::MessageContent::Text(
                                                user,
                                            ),
                                        }],
                                        tools: vec![],
                                        max_tokens: 1024,
                                        temperature: 0.1,
                                        system: Some(sys),
                                        thinking: None,
                                        extra: Default::default(),
                                    };
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(30),
                                        driver.complete(anon_req),
                                    )
                                    .await
                                    {
                                        Ok(Ok(anon_resp)) => {
                                            let anon_text = anon_resp.text();
                                            let (title, content) =
                                                carrier_lifecycle::feedback::parse_anonymize_response(
                                                    &anon_text,
                                                )
                                                .unwrap_or_else(|_| {
                                                    (
                                                        candidate.title.clone(),
                                                        candidate.content.clone(),
                                                    )
                                                });
                                            if let Err(e) =
                                                carrier_lifecycle::feedback::save_feedback(
                                                    &workspace,
                                                    &clone_name,
                                                    &title,
                                                    &content,
                                                )
                                            {
                                                tracing::warn!(error = %e, "Feedback: failed to save");
                                            }
                                        }
                                        Ok(Err(e)) => {
                                            tracing::warn!(error = %e, "Feedback: anonymize LLM failed");
                                        }
                                        Err(_) => {
                                            tracing::warn!(
                                                "Feedback: anonymize LLM timed out after 30s"
                                            );
                                        }
                                    }
                                }

                                // Push collected feedback to Hub
                                if let Ok(entries) =
                                    carrier_lifecycle::feedback::collect_feedback(&workspace)
                                {
                                    if !entries.is_empty() {
                                        match carrier_lifecycle::feedback::push_feedback_to_hub(
                                            &hub_url,
                                            &hub_api_key,
                                            &entries,
                                        )
                                        .await
                                        {
                                            Ok(results) => {
                                                tracing::info!(
                                                    count = results.len(),
                                                    "Feedback: pushed to Hub"
                                                );
                                            }
                                            Err(e) => {
                                                tracing::warn!(error = %e, "Feedback: push failed");
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "Evolution: failed to parse analysis")
                        }
                    }
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "Evolution: LLM call failed");
                }
                Err(_) => {
                    tracing::warn!("Evolution: LLM call timed out after 60s");
                }
            }
        });
    }
}

// ── Boot / lifecycle ───────────────────────────────────────

impl CarrierKernel {
    /// Boot the kernel with configuration from the given path.
    pub fn boot(config_path: Option<&Path>) -> KernelResult<Self> {
        let config = load_config(config_path);
        Self::boot_with_config(config)
    }

    /// Boot the kernel with an explicit configuration.
    pub fn boot_with_config(mut config: KernelConfig) -> KernelResult<Self> {
        use carrier_types::config::KernelMode;

        // `~/.aginx/carrier/.env`（+ secrets.env）进进程内覆盖表——所有形态
        // （start 守护 / acp 桥 / 桌面）都经这里 boot，LLM key 等一次到位。
        // 系统环境变量优先，不覆盖。
        crate::dotenv::load_dotenv();

        // Env var overrides — useful for Docker where config.toml is baked in.
        if let Ok(listen) = std::env::var("AGINX_CARRIER_LISTEN") {
            config.api_listen = listen;
        }

        // AGINX_CARRIER_API_KEY: env var sets the API authentication key when
        // config.toml doesn't already have one.  Config file takes precedence.
        if config.api_key.trim().is_empty() {
            if let Ok(key) = std::env::var("AGINX_CARRIER_API_KEY") {
                let key = key.trim().to_string();
                if !key.is_empty() {
                    info!("Using API key from AGINX_CARRIER_API_KEY environment variable");
                    config.api_key = key;
                }
            }
        }

        // Clamp configuration bounds to prevent zero-value or unbounded misconfigs
        config.clamp_bounds();

        match config.mode {
            KernelMode::Stable => {
                info!("Booting Carrier kernel in STABLE mode — conservative defaults enforced");
            }
            KernelMode::Dev => {
                warn!("Booting Carrier kernel in DEV mode — experimental features enabled");
            }
            KernelMode::Default => {
                info!("Booting Carrier kernel...");
            }
        }

        // Validate configuration and log warnings
        let warnings = config.validate();
        for w in &warnings {
            warn!("Config: {}", w);
        }

        // Ensure data directory exists
        std::fs::create_dir_all(&config.data_dir)
            .map_err(|e| KernelError::BootFailed(format!("Failed to create data dir: {e}")))?;

        // Initialize memory substrate
        let db_path = config
            .memory
            .sqlite_path
            .clone()
            .unwrap_or_else(|| config.data_dir.join("carrier.db"));
        let memory = Arc::new(
            MemorySubstrate::open(&db_path)
                .map_err(|e| KernelError::BootFailed(format!("Memory init failed: {e}")))?,
        );

        // ── Auto-migrate admin tenant from config.toml ──────────────
        // ── Load Brain (carrier's independent LLM brain) ──────────────
        // Brain is required — boot fails without a valid brain.json.
        let brain_path = config.home_dir.join(&config.brain.config);
        let brain = if brain_path.exists() {
            let json_str = std::fs::read_to_string(&brain_path).map_err(|e| {
                KernelError::BootFailed(format!("Cannot read {}: {e}", brain_path.display()))
            })?;
            let brain_config: carrier_types::brain::BrainConfig =
                serde_json::from_str(&json_str)
                    .map_err(|e| KernelError::BootFailed(format!("Invalid brain.json: {e}")))?;
            let brain = Brain::new(brain_config)
                .map_err(|e| KernelError::BootFailed(format!("Brain init failed: {e}")))?;
            info!("Brain loaded from {}", brain_path.display());
            brain
        } else {
            // No local brain.json — try fetching from Hub.
            info!("Brain config not found locally; attempting to fetch from Hub...");
            match Self::fetch_brain_from_hub(&config.hub, &brain_path) {
                Ok(brain_config) => {
                    let brain = Brain::new(brain_config)
                        .map_err(|e| KernelError::BootFailed(format!("Brain init failed: {e}")))?;
                    info!(
                        "Brain fetched from Hub and saved to {}",
                        brain_path.display()
                    );
                    brain
                }
                Err(e) => {
                    return Err(KernelError::BootFailed(format!(
                        "Brain config not found at {} and could not be fetched from Hub: {}. \
                         Please set {} or create brain.json manually.",
                        brain_path.display(),
                        e,
                        config.hub.api_key_env
                    )));
                }
            }
        };

        // Initialize metering engine (shares the same SQLite connection as the memory substrate)
        let metering = Arc::new(MeteringEngine::new(
            Arc::new(carrier_memory::usage::UsageStore::new(memory.usage_conn())),
            config.budget.clone(),
        ));

        let supervisor = Supervisor::new();
        let background = BackgroundExecutor::new(supervisor.subscribe());

        // Initialize WASM sandbox engine (shared across all WASM agents)
        let wasm_sandbox = WasmSandbox::new()
            .map_err(|e| KernelError::BootFailed(format!("WASM sandbox init failed: {e}")))?;

        // MCP server list: use config directly (no extension merging)
        let all_mcp_servers = config.mcp_servers.clone();

        let brain_arc: Arc<Brain> = Arc::new(brain);

        // Initialize media understanding engine
        let media_engine =
            carrier_runtime::media_understanding::MediaEngine::new(config.media.clone());

        // Initialize cron scheduler with DB-backed persistence
        let mut cron_scheduler =
            crate::cron::CronScheduler::new(&config.home_dir, config.max_cron_jobs);
        cron_scheduler.set_db_store(Arc::new(memory.cron_store().clone()));
        match cron_scheduler.load() {
            Ok(count) => {
                if count > 0 {
                    info!("Loaded {count} cron job(s) from database");
                }
            }
            Err(e) => {
                warn!("Failed to load cron jobs: {e}");
            }
        }

        // Initialize binding/broadcast from config
        let initial_bindings = config.bindings.clone();
        let initial_broadcast = config.broadcast.clone();
        let llm_concurrency = config.llm_concurrency;

        let kernel = Self {
            config,
            registry: AgentRegistry::new(),
            memory: memory.clone(),
            audit_log: Arc::new(AuditLog::with_db(memory.usage_conn())),
            sender_gate: crate::sender_gate::SenderGate::default(),
            metering,
            cron_scheduler,
            channel_send_fn: std::sync::RwLock::new(None),
            channel_deliver_fn: std::sync::RwLock::new(None),
            channel_supports_proactive_fn: std::sync::RwLock::new(None),
            brain: KernelBrain {
                brain: Arc::new(std::sync::RwLock::new(brain_arc)),
                brain_path: brain_path.clone(),
            },
            a2a: KernelA2a {
                a2a_task_store: carrier_runtime::a2a::A2aTaskStore::default(),
                a2a_external_agents: std::sync::Mutex::new(Vec::new()),
            },
            services: KernelServices { media_engine },
            plugins: KernelPlugins {
                mcp_connections: Arc::new(dashmap::DashMap::new()),
                mcp_tools: std::sync::Mutex::new(Vec::new()),
                toolset_registry: std::sync::RwLock::new(std::collections::HashMap::new()),
                effective_mcp_servers: std::sync::RwLock::new(all_mcp_servers),
                plugin_tool_dispatcher: std::sync::Mutex::new(None),
                mcp_reconnect_failures: dashmap::DashMap::new(),
            },
            runtime: KernelRuntime {
                scheduler: AgentScheduler::new(),
                supervisor,
                background,
                running_tasks: dashmap::DashMap::new(),
                wasm_sandbox,
                llm_concurrency_limit: Arc::new(tokio::sync::Semaphore::new(llm_concurrency)),
                watcher_handles: std::sync::Mutex::new(Vec::new()),
            },
            coordination: KernelCoordination {
                capabilities: CapabilityManager::new(),
                event_bus: EventBus::new(),
                bindings: std::sync::Mutex::new(initial_bindings),
                broadcast: initial_broadcast,
                hooks: carrier_runtime::hooks::HookRegistry::new(),
                process_manager: Arc::new(carrier_runtime::process_manager::ProcessManager::new(5)),
                booted_at: std::time::Instant::now(),
                self_handle: OnceLock::new(),
            },
        };

        // Restore persisted agents from SQLite
        match kernel.memory.load_all_agents() {
            Ok(agents) => {
                let count = agents.len();
                for entry in agents {
                    let agent_id = entry.id;
                    let name = entry.name.clone();

                    let mut entry = entry;

                    let ws = kernel.config.agent_workspace_dir(&name);
                    entry.manifest.workspace = Some(ws.clone());

                    // Hot-reload agent.toml if it exists — picks up tool/capability changes
                    // made to the workspace without needing an explicit restart.
                    let toml_path = ws.join("agent.toml");
                    if toml_path.exists() {
                        if let Ok(toml_str) = std::fs::read_to_string(&toml_path) {
                            if let Ok(disk_manifest) = toml::from_str::<AgentManifest>(&toml_str) {
                                // Surface schema/type drift in agent.toml that the
                                // lenient deserializers would otherwise silently
                                // empty (notably tool_blocklist/tool_allowlist,
                                // where empty means "no exclusions"). See
                                // carrier_types::serde_compat::take_lenient_diagnostics.
                                let drift = carrier_types::serde_compat::take_lenient_diagnostics();
                                if !drift.is_empty() {
                                    tracing::warn!(
                                        agent = %name,
                                        count = drift.len(),
                                        details = ?drift,
                                        "agent.toml fields fell back to empty defaults due to type drift — check tool_blocklist/tool_allowlist/etc."
                                    );
                                }
                                let mut disk_manifest = disk_manifest;
                                disk_manifest.workspace = Some(ws.clone());
                                // Definition-layer overlay (same rationale as
                                // reload_manifest_from_workspace): agent.toml is
                                // install-time generated and dup pushes never
                                // touch the runtime layer, so without this fill
                                // every daemon restart would clobber the DB
                                // manifest's presentation fields (display_name/
                                // description) with the stale agent.toml copy.
                                crate::sessions::fill_presentation_from_template_json(
                                    &mut disk_manifest,
                                    &ws,
                                );
                                if disk_manifest.exec_policy.is_none() {
                                    disk_manifest.exec_policy =
                                        Some(kernel.config.exec_policy.clone());
                                }
                                if disk_manifest.model.modality.is_empty() {
                                    disk_manifest.model.modality = "chat".to_string();
                                }
                                entry.manifest = disk_manifest;
                                tracing::info!(agent = %name, "Hot-reloaded manifest from agent.toml on boot");
                            }
                        }
                    }

                    // Re-grant capabilities
                    let caps = manifest_to_capabilities(&entry.manifest);
                    kernel.coordination.capabilities.grant(agent_id, caps);

                    // Re-register with scheduler
                    kernel
                        .runtime
                        .scheduler
                        .register(agent_id, entry.manifest.resources.clone());

                    // Re-register in the in-memory registry.
                    // Restore Running agents as-is; promote Created/Suspended → Running
                    // so agents resume after service restarts without manual intervention.
                    let mut restored_entry = entry;
                    if restored_entry.state == AgentState::Created
                        || restored_entry.state == AgentState::Suspended
                    {
                        restored_entry.state = AgentState::Running;
                    }

                    // Inherit kernel exec_policy for agents that lack one
                    if restored_entry.manifest.exec_policy.is_none() {
                        restored_entry.manifest.exec_policy =
                            Some(kernel.config.exec_policy.clone());
                    }

                    // Apply default modality to restored agents if empty.
                    {
                        if restored_entry.manifest.model.modality.is_empty() {
                            restored_entry.manifest.model.modality = "chat".to_string();
                        }
                    }

                    if let Err(e) = kernel.registry.register(restored_entry) {
                        tracing::warn!(agent = %name, "Failed to restore agent: {e}");
                    } else {
                        tracing::debug!(agent = %name, id = %agent_id, "Restored agent");
                    }
                }
                if count > 0 {
                    info!("Restored {count} agent(s) from persistent storage");
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load persisted agents: {e}");
            }
        }

        // Boot validation complete

        info!("Carrier kernel booted successfully");
        Ok(kernel)
    }

    /// Spawn a new agent from a manifest, optionally linking to a parent agent.
    pub fn spawn_agent(&self, manifest: AgentManifest) -> KernelResult<AgentId> {
        self.spawn_agent_with_parent(manifest, None, None)
    }

    /// Spawn a new agent with an optional parent for lineage tracking.
    /// If fixed_id is provided, use it instead of generating a new UUID.
    /// If tenant_id is provided, the agent and its workspace are scoped to that tenant.
    pub fn spawn_agent_with_parent(
        &self,
        manifest: AgentManifest,
        parent: Option<AgentId>,
        fixed_id: Option<AgentId>,
    ) -> KernelResult<AgentId> {
        let agent_id = fixed_id.unwrap_or_default();
        let session_id = SessionId::new();
        let name = manifest.name.clone();

        // SECURITY: Validate agent name doesn't contain path traversal characters
        if name.contains('/') || name.contains('\\') || name.contains("..") || name.is_empty() {
            return Err(KernelError::Carrier(
                carrier_types::error::CarrierError::InvalidInput(format!(
                    "Invalid agent name {:?}: must not contain path separators or '..'",
                    name
                )),
            ));
        }

        info!(agent = %name, id = %agent_id, parent = ?parent, "Spawning agent");

        // Create session
        self.memory
            .create_session(name.clone())
            .map_err(KernelError::Carrier)?;

        // Inherit kernel exec_policy as fallback if agent manifest doesn't have one
        let mut manifest = manifest;
        if manifest.exec_policy.is_none() {
            manifest.exec_policy = Some(self.config.exec_policy.clone());
        }
        // Inherit kernel cli_exec config as fallback if agent manifest doesn't have one
        if manifest.cli_exec.is_none() && !self.config.cli_exec.commands.is_empty() {
            manifest.cli_exec = Some(self.config.cli_exec.clone());
        }
        info!(agent = %name, id = %agent_id, exec_mode = ?manifest.exec_policy.as_ref().map(|p| &p.mode), "Agent exec_policy resolved");

        // NOTE: an earlier comment here claimed default_model is "overlaid onto
        // agents that don't explicitly choose", but no such overlay is wired —
        // config.default_model is only surfaced by the CLI and watched by
        // hot-reload. Agents get their model from their own manifest / brain.
        // Bundled agents defer by leaving model unset; that deferral resolves
        // via brain.json, not via this field. Kept honest to avoid chasing a
        // non-existent code path.
        // Create workspace directory for the agent (name-based, so SOUL.md survives recreation)
        let workspace_dir = manifest
            .workspace
            .clone()
            .unwrap_or_else(|| self.config.agent_workspace_dir(&name));
        if workspace_dir == self.config.home_dir {
            // 母体（"me"）：家目录即工作区。只补会话账目录——助理脚手架
            // （knowledge/logs/history/AGENT.json）不落在家根上（docs/FS.md）。
            std::fs::create_dir_all(workspace_dir.join("sessions")).map_err(|e| {
                KernelError::Carrier(carrier_types::error::CarrierError::Internal(format!(
                    "Failed to create mother sessions dir: {e}"
                )))
            })?;
        } else {
            ensure_workspace(&workspace_dir)?;
        }
        if manifest.generate_identity_files {
            generate_identity_files(&workspace_dir, &manifest);
        }
        manifest.workspace = Some(workspace_dir);

        // Register capabilities
        let caps = manifest_to_capabilities(&manifest);
        self.coordination.capabilities.grant(agent_id, caps);

        // Register with scheduler
        self.runtime
            .scheduler
            .register(agent_id, manifest.resources.clone());

        // Create registry entry
        let tags = manifest.tags.clone();
        let entry = AgentEntry {
            id: agent_id,
            name: manifest.name.clone(),
            manifest,
            state: AgentState::Running,
            mode: AgentMode::default(),
            created_at: chrono::Utc::now(),
            last_active: chrono::Utc::now(),
            parent,
            children: vec![],
            session_id,
            tags,
            identity: Default::default(),
            onboarding_completed: false,
            onboarding_completed_at: None,
        };
        self.registry
            .register(entry.clone())
            .map_err(KernelError::Carrier)?;

        // Update parent's children list
        if let Some(parent_id) = parent {
            self.registry.add_child(parent_id, agent_id);
        }

        // Persist agent to SQLite so it survives restarts
        self.memory
            .save_agent(&entry)
            .map_err(KernelError::Carrier)?;

        info!(agent = %name, id = %agent_id, "Agent spawned");

        // SECURITY: Record agent spawn in audit trail
        self.audit_log.record(
            agent_id.to_string(),
            carrier_runtime::audit::AuditAction::AgentSpawn,
            format!("name={name}, parent={parent:?}"),
            "ok",
        );

        Ok(agent_id)
    }

    /// Verify a signed manifest envelope (Ed25519 + SHA-256).
    ///
    /// Call this before `spawn_agent` when a `SignedManifest` JSON is provided
    /// alongside the TOML. Returns the verified manifest TOML string on success.
    pub fn verify_signed_manifest(&self, signed_json: &str) -> KernelResult<String> {
        let signed: carrier_types::manifest_signing::SignedManifest =
            serde_json::from_str(signed_json).map_err(|e| {
                KernelError::Carrier(carrier_types::error::CarrierError::Config(format!(
                    "Invalid signed manifest JSON: {e}"
                )))
            })?;

        // Verify using trust store if trusted keys are configured
        let trusted_keys: Vec<ed25519_dalek::VerifyingKey> = self
            .config
            .trusted_signing_keys
            .iter()
            .filter_map(|hex_str| {
                let bytes = hex::decode(hex_str).ok()?;
                let arr: [u8; 32] = bytes.try_into().ok()?;
                ed25519_dalek::VerifyingKey::from_bytes(&arr).ok()
            })
            .collect();
        if !trusted_keys.is_empty() {
            signed
                .verify_with_trust_store(&trusted_keys)
                .map_err(KernelError::Carrier)?;
        } else {
            // Fallback: verify with embedded key + warn
            warn!("No trusted_signing_keys configured — verifying with embedded key (less secure)");
            signed.verify().map_err(KernelError::Carrier)?;
        }

        info!(signer = %signed.signer_id, hash = %signed.content_hash, "Signed manifest verified");
        Ok(signed.manifest)
    }

    /// Build the toolset registry from builtin modules only.
    /// MCP tools are stored separately in mcp_tools and loaded by agent config.
    /// Must be called after MCP connections are established (for logging purposes).
    pub(crate) fn build_toolset_registry(&self) {
        let mut registry: std::collections::HashMap<String, Vec<ToolDefinition>> =
            std::collections::HashMap::new();

        // Group builtin tools by toolset
        let all_builtins =
            carrier_runtime::tool_runner::builtin_tool_definitions(self.config.cli_exec.clone());
        for tool in &all_builtins {
            if let Some(ts_name) = Self::tool_to_toolset(&tool.name) {
                registry
                    .entry(ts_name.to_string())
                    .or_default()
                    .push(tool.clone());
            }
        }

        let mcp_count = self.plugins.mcp_tools.lock().map(|t| t.len()).unwrap_or(0);
        tracing::info!(
            builtin_toolsets = registry.len(),
            mcp_tools = mcp_count,
            toolsets = ?registry.keys().collect::<Vec<_>>(),
            "Built toolset registry (builtins only, MCP tools separate)"
        );

        if let Ok(mut reg) = self.plugins.toolset_registry.write() {
            *reg = registry;
        }
    }

    /// Map a builtin tool name to its toolset. Returns None for core tools.
    fn tool_to_toolset(name: &str) -> Option<&'static str> {
        match name {
            "session_summarize" | "flow_load" | "flow_create" | "flow_update"
            | "knowledge_read" | "knowledge_list" | "file_read" | "file_list" | "cron_create"
            | "cron_list" | "cron_cancel" | "memory_tree" | "task_plan" => None,
            n if n.starts_with("file_") => Some("filesystem"),
            "shell_exec" => Some("shell"),
            n if n.starts_with("knowledge_") || n.starts_with("flow_") || n == "clone_evaluate" => {
                Some("knowledge")
            }
            n if n.starts_with("memory_") => Some("memory"),
            n if n.starts_with("media_")
                || n.starts_with("image_")
                || n == "text_to_speech"
                || n == "speech_to_text" =>
            {
                Some("media")
            }
            n if n.starts_with("web_") => Some("web"),
            n if n.starts_with("browser_") => Some("browser"),
            n if n.starts_with("agent_") || n.starts_with("train_") => Some("agent"),
            n if n.starts_with("location_") || n.starts_with("system_") || n == "user_profile" => {
                Some("misc")
            }
            n if n.starts_with("process_") => Some("process"),
            "apply_patch" => Some("filesystem"),
            _ => Some("misc"),
        }
    }

    /// Build a compact toolset summary for the system prompt.
    /// All tools are active (always visible), so no ACTIVE/available distinction.
    fn build_toolset_summary(&self) -> String {
        let mut summary = String::new();

        // --- Built-in toolsets ---
        let registry = match self.plugins.toolset_registry.read() {
            Ok(r) => r.clone(),
            Err(_) => return String::new(),
        };

        if !registry.is_empty() {
            summary
                .push_str("\n\n--- Built-in Toolsets ---\nAll tools are available directly.\n\n");

            let mut entries: Vec<_> = registry.iter().collect();
            entries.sort_by_key(|(name, _)| name.as_str());

            for (name, tools) in &entries {
                let examples: Vec<&str> = tools.iter().take(3).map(|t| t.name.as_str()).collect();
                let example_str = if tools.len() > 3 {
                    format!("{}, ... ({} total)", examples.join(", "), tools.len())
                } else {
                    examples.join(", ")
                };

                summary.push_str(&format!(
                    "- [{}] {} tools: {}\n",
                    name,
                    tools.len(),
                    example_str
                ));
            }
        }

        // --- MCP Servers ---
        let mcp_entries: Vec<_> = self.plugins.mcp_connections.iter().collect();
        if !mcp_entries.is_empty() {
            summary.push_str("\n--- MCP Servers ---\nThese servers are configured and their tools are available directly.\n");
            for entry in &mcp_entries {
                let conn = entry.value();
                let config = conn.config();
                let desc = if config.description.is_empty() {
                    String::new()
                } else {
                    format!(": {}", config.description)
                };
                let tool_names: Vec<&str> = conn
                    .tools()
                    .iter()
                    .take(3)
                    .map(|t| t.name.as_str())
                    .collect();
                let tool_str = if conn.tools().len() > 3 {
                    format!(
                        "{}, ... ({} total)",
                        tool_names.join(", "),
                        conn.tools().len()
                    )
                } else {
                    tool_names.join(", ")
                };
                summary.push_str(&format!("- {}{} — {}\n", config.name, desc, tool_str));
            }
        }

        // Filesystem MCP guidance
        if registry.keys().any(|s| s.contains("filesystem")) {
            summary.push_str(
                "\nIMPORTANT: For accessing files OUTSIDE your workspace directory, you MUST use \
                 the MCP filesystem tools (e.g. mcp_filesystem_read_file, mcp_filesystem_list_directory) \
                 instead of the built-in file_read/file_list/file_write tools, which are restricted to \
                 the workspace. The MCP filesystem server has been granted access to specific directories \
                 by the user.\n",
            );
        }

        summary
    }

    /// Format a millisecond timestamp for display in memory hits.
    fn format_time_ms(ms: i64) -> String {
        chrono::DateTime::from_timestamp_millis(ms)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| ms.to_string())
    }

    /// Prefetch tree memories for prompt injection (检索回指).
    ///
    /// Reads back what `tree_ingest` writes every turn. When the turn has a
    /// sender, query THEIR source trees (per-user isolation — the cross-user
    /// global digest would leak other users' summaries into this prompt);
    /// fall back to the global digest for sender-less turns (cron/background)
    /// or when the sender has no history yet.
    fn prefetch_tree_memories(
        &self,
        owner_id: &str,
        sender_id: Option<&str>,
    ) -> Vec<carrier_runtime::prompt_builder::TreeMemoryHit> {
        use carrier_types::memory_tree::{GlobalQuery, SourceQuery};

        // Tree ingest partition: owner_id.unwrap_or("default"), user = sender.
        let tree_owner = if owner_id.is_empty() {
            "default"
        } else {
            owner_id
        };

        let handle = crate::handle::make_memory_handle(std::sync::Arc::clone(&self.memory));
        // Through the shared memory handle — tree queries take the same
        // path the tools do (memory is local, M35).
        let per_user = sender_id.filter(|s| !s.is_empty()).map(|sender| {
            let req = SourceQuery {
                owner_id: tree_owner,
                source_id: None,
                source_kind: None,
                time_window_days: Some(7),
                query: None,
                limit: 3,
                user_id: Some(sender),
            };
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(handle.tree_query_source(req))
            })
        });

        let result = match per_user {
            // Sender has history → per-user hits only (no cross-user leak).
            Some(Ok(resp)) if !resp.hits.is_empty() => Ok(resp),
            // Sender has nothing yet → fall through to the global digest.
            Some(Ok(_)) | None => {
                let owner = tree_owner.to_string();
                let req = GlobalQuery {
                    owner_id: &owner,
                    time_window_days: Some(7),
                    query: None,
                    limit: 3,
                    user_id: None,
                };
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(handle.tree_query_global(req))
                })
            }
            Some(Err(_)) => {
                let owner = tree_owner.to_string();
                let req = GlobalQuery {
                    owner_id: &owner,
                    time_window_days: Some(7),
                    query: None,
                    limit: 3,
                    user_id: None,
                };
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(handle.tree_query_global(req))
                })
            }
        };

        match result {
            Ok(resp) => resp
                .hits
                .iter()
                .take(3)
                .map(|h| carrier_runtime::prompt_builder::TreeMemoryHit {
                    scope: h.tree_scope.clone(),
                    kind: h.tree_kind.to_string(),
                    content: h.content.chars().take(500).collect(),
                    time_range: format!(
                        "{} — {}",
                        Self::format_time_ms(h.time_range_start_ms),
                        Self::format_time_ms(h.time_range_end_ms)
                    ),
                })
                .collect(),
            Err(e) => {
                tracing::debug!("Tree memory prefetch failed (non-fatal): {e}");
                Vec::new()
            }
        }
    }

    /// Prefetch kv memories for prompt injection (检索回指).
    ///
    /// ONE kv_list on the canonical kv partition — `(agent_name, owner_id or
    /// "", sender_id or "")` — which is where every writer puts data (kv_set
    /// tool, per-turn key_facts, compaction write-back). The old code read
    /// `(name, owner, owner)` — using the owner as the user — so every
    /// sender-partitioned entry was invisible to the prompt (and multi-user
    /// clones would have shown the owner's drawer to everyone).
    ///
    /// Returns (drawer entries, recalled memories):
    /// - drawer: profile./preference./entity./fact./event.* keys
    /// - recalled: the two most recent `session_compaction.*` summaries —
    ///   the compaction write-back bridge's cross-session 回指 (old sessions
    ///   stay recallable after their turn_summaries were compacted away).
    pub(crate) fn prefetch_kv_memories(
        &self,
        agent_name: &str,
        owner_id: &str,
        sender_id: &str,
    ) -> (
        Vec<carrier_runtime::prompt_builder::DrawerEntry>,
        Vec<(String, String)>,
    ) {
        // Through the shared memory handle — single write path (memory is
        // local, M35).
        let handle = crate::handle::make_memory_handle(std::sync::Arc::clone(&self.memory));
        let all_pairs = match handle.kv_list(agent_name, owner_id, sender_id) {
            Ok(pairs) => pairs,
            Err(e) => {
                tracing::debug!("kv memory prefetch failed (non-fatal): {e}");
                return (Vec::new(), Vec::new());
            }
        };

        const DRAWER_PREFIXES: &[&str] = &["profile.", "preference.", "entity.", "fact.", "event."];
        const COMPACTION_PREFIX: &str = "session_compaction.";
        const MAX_COMPACTION_RECALLED: usize = 2;

        let mut drawer = Vec::new();
        let mut compactions: Vec<String> = Vec::new();
        for (key, value) in all_pairs {
            if key.starts_with(COMPACTION_PREFIX) {
                if let serde_json::Value::String(s) = value {
                    compactions.push(format!("{key}\u{1}{s}"));
                }
                continue;
            }
            if !DRAWER_PREFIXES.iter().any(|p| key.starts_with(p)) {
                continue;
            }
            let values = match value {
                serde_json::Value::Array(arr) => arr
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect(),
                serde_json::Value::String(s) => vec![s],
                other => vec![other.to_string()],
            };
            if !values.is_empty() {
                drawer.push(carrier_runtime::prompt_builder::DrawerEntry { key, value: values });
            }
        }

        // session_compaction.{YYYY-MM-DD} sorts lexicographically by date.
        compactions.sort();
        compactions.reverse();
        let recalled = compactions
            .into_iter()
            .take(MAX_COMPACTION_RECALLED)
            .map(|joined| {
                let mut parts = joined.splitn(2, '\u{1}');
                let key = parts.next().unwrap_or_default().to_string();
                let val = parts.next().unwrap_or_default().to_string();
                (key, val)
            })
            .collect();

        (drawer, recalled)
    }

    /// Build PromptContext and apply it to the manifest's system prompt.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_apply_prompt(
        &self,
        manifest: &mut AgentManifest,
        tools: &[carrier_types::tool::ToolDefinition],
        sender_id: &Option<String>,
        sender_name: Option<String>,
        owner_id: &Option<String>,
        auto_matched_flow: Option<String>,
        turn_summaries: Vec<carrier_types::message::TurnSummary>,
        drawer_entries: Vec<carrier_runtime::prompt_builder::DrawerEntry>,
        recalled_memories: Vec<(String, String)>,
        task_id: Option<String>,
        chain_id: Option<String>,
    ) {
        let sid = sender_id.as_deref().unwrap_or("");
        let oid = owner_id.as_deref().unwrap_or(sid);
        // Canonical kv partition owner: every kv writer uses
        // owner_id.unwrap_or("") — NOT the sender fallback (that mismatch is
        // what used to hide sender-partitioned memories from the prompt).
        let kv_owner = owner_id.as_deref().unwrap_or("");
        // Through the shared memory handle — single write path (memory is
        // local, M35).
        let mem_handle = crate::handle::make_memory_handle(std::sync::Arc::clone(&self.memory));
        let user_name = mem_handle
            .kv_get(&manifest.name, kv_owner, sid, "user_name")
            .ok()
            .flatten()
            .and_then(|v| v.as_str().map(String::from))
            .or_else(|| sender_name.clone());

        let peer_agents: Vec<(String, String, String)> = self
            .registry
            .list()
            .iter()
            .map(|a| {
                (
                    a.name.clone(),
                    format!("{:?}", a.state),
                    a.manifest.model.modality.clone(),
                )
            })
            .collect();

        let prompt_ctx = carrier_runtime::prompt_builder::PromptContext {
            agent_name: manifest.name.clone(),
            agent_description: manifest.description.clone(),
            base_system_prompt: manifest.model.system_prompt.clone(),
            granted_tools: tools.iter().map(|t| t.name.clone()).collect(),
            recalled_memories,
            tree_memories: self.prefetch_tree_memories(kv_owner, sender_id.as_deref()),
            flow_summary: String::new(),
            flow_prompt_context: String::new(),
            mcp_summary: self.build_toolset_summary(),
            workspace_path: manifest.workspace.as_ref().map(|p| p.display().to_string()),
            soul_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "SOUL.md")),
            user_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "USER.md")),
            memory_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "MEMORY.md")),
            user_name,
            channel_type: None,
            is_subagent: manifest
                .metadata
                .get("is_subagent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            is_autonomous: manifest.autonomous.is_some(),
            agents_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "AGENTS.md")),
            bootstrap_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "BOOTSTRAP.md")),
            workspace_context: manifest.workspace.as_ref().map(|w| {
                let mut ws_ctx = carrier_runtime::workspace_context::WorkspaceContext::detect(w);
                ws_ctx.build_context_section()
            }),
            identity_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "IDENTITY.md")),
            heartbeat_md: if manifest.autonomous.is_some() {
                manifest
                    .workspace
                    .as_ref()
                    .and_then(|w| crate::prompt_sources::read_identity_file(w, "HEARTBEAT.md"))
            } else {
                None
            },
            peer_agents,
            current_date: Some(
                chrono::Local::now()
                    .format("%A, %B %d, %Y (%Y-%m-%d %H:%M %Z)")
                    .to_string(),
            ),
            sender_id: sender_id.clone(),
            sender_name,
            user_profile_summary: sender_id.as_ref().and_then(|sid| {
                crate::prompt_sources::read_user_profile_summary(
                    &self.config.home_dir,
                    oid,
                    &manifest.name,
                    Some(sid),
                )
            }),
            // Admin session signal — creator/approved admin per admins.json.
            // Drives the [管理员会话] prompt section + admin-gated tools.
            is_admin: manifest
                .workspace
                .as_ref()
                .map(|w| carrier_runtime::plugin::admin_store::is_admin(w, sid))
                .unwrap_or(false),
            clone_system_prompt_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "system_prompt.md")),
            clone_flows_catalog: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_flows_catalog(w)),
            clone_style_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_style_samples(w)),
            clone_flows_prompts: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_workspace_flows_prompts(w)),
            knowledge_content: manifest.workspace.as_ref().and_then(|w| {
                crate::prompt_sources::read_knowledge_content(
                    w,
                    Some(oid),
                    sender_id.as_deref(),
                    Some(&self.config.home_dir),
                    Some(&manifest.name),
                )
            }),
            clone_agents_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_agents_directory(w)),
            evolution_rules_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_evolution_rules(w)),
            mental_models_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "MENTAL-MODELS.md")),
            decision_heuristics_md: manifest.workspace.as_ref().and_then(|w| {
                crate::prompt_sources::read_identity_file(w, "DECISION-HEURISTICS.md")
            }),
            expression_dna_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "EXPRESSION-DNA.md")),
            timeline_md: manifest
                .workspace
                .as_ref()
                .and_then(|w| crate::prompt_sources::read_identity_file(w, "TIMELINE.md")),
            auto_matched_flow,
            turn_summaries,
            drawer_entries,
            task_id,
            chain_id,
        };
        manifest.model.system_prompt =
            carrier_runtime::prompt_builder::build_system_prompt(&prompt_ctx);
    }

    /// Push a notification to admins (automation `notify_admin` rule bypass).
    /// Looks up `notify_type` in `notify_routes`, resolves recipients (admins
    /// fan-out via the agent's `admins.json`, or the route's explicit user_id),
    /// and pushes via `channel_send_fn`. Does NOT touch the agent turn -- the
    /// caller still runs the agent to reply to the user.
    pub async fn notify_admins(
        &self,
        agent_id: &str,
        notify_type: &str,
        content: &str,
        source_sender: &str,
        source_bot: &str,
    ) -> carrier_types::error::CarrierResult<()> {
        use carrier_types::error::CarrierError;

        // 1. Find the route for this notify type.
        let route = {
            let routes = self.memory.notify_store().load_all()?;
            routes
                .iter()
                .find(|r| r.name == notify_type)
                .cloned()
                .ok_or_else(|| CarrierError::Config(format!("no notify route '{notify_type}'")))?
        };

        // 2. Build the push message (prefix + content + source).
        let msg = match route.prefix.as_ref().filter(|p| !p.is_empty()) {
            Some(p) => format!("{p}\n{content}\n来源用户: {source_sender}"),
            None => format!("{content}\n来源用户: {source_sender}"),
        };

        // 3. Resolve recipient (channel, bot_id, user_id) tuples.
        //    recipients="admins" -> fan out via admins.json, routing each admin
        //    through sender_channels (authoritative) with prefix fallback — this
        //    also handles wecom (`wm…`) admins, which the old inline logic missed.
        let cron_store = self.memory.cron_delivery();
        let recipient_ids: Vec<(String, String, String)> =
            if route.recipients.as_deref() == Some("admins") {
                // Inline resolve_agent_workspace (it's a KernelHandle trait
                // method; inlining avoids importing the trait here).
                let ws = self
                    .registry
                    .resolve(agent_id)
                    .ok()
                    .and_then(|(_, entry)| entry.manifest.workspace.clone())
                    .map(|p| p.to_string_lossy().to_string());
                match ws {
                    Some(ws) => {
                        let admins = carrier_runtime::plugin::admin_store::read_admins(
                            std::path::Path::new(&ws),
                        );
                        admins
                            .admins
                            .into_iter()
                            .map(|a| {
                                carrier_memory::cron_delivery::route_recipient(
                                    &a.sender_id,
                                    cron_store,
                                    source_bot,
                                )
                            })
                            .collect()
                    }
                    None => {
                        tracing::warn!(
                            agent_id = %agent_id, notify_type = %notify_type,
                            "notify_admins: recipients=admins but workspace unresolved"
                        );
                        Vec::new()
                    }
                }
            } else if route.user_id.is_empty() {
                tracing::warn!(
                    notify_type = %notify_type,
                    "notify_admins: route has empty user_id and recipients != admins"
                );
                Vec::new()
            } else {
                vec![(
                    route.channel.clone(),
                    route.bot_id.clone(),
                    route.user_id.clone(),
                )]
            };

        // 4. Push via channel_send_fn (sync fn -> spawn_blocking, fire-and-forget + log).
        let send_fn = self
            .channel_send_fn
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        let send_fn = match send_fn {
            Some(f) => f,
            None => {
                return Err(CarrierError::Config(
                    "channel_send_fn not configured".into(),
                ))
            }
        };

        for (channel, bot_id, user_id) in recipient_ids {
            let send_fn = std::sync::Arc::clone(&send_fn);
            let msg = msg.clone();
            let (ch, bot, user) = (channel.clone(), bot_id.clone(), user_id.clone());
            let nt = notify_type.to_string();
            match tokio::task::spawn_blocking(move || send_fn(&ch, &bot, &user, &msg)).await {
                Ok(Ok(())) => tracing::info!(
                    notify_type = %nt, target_channel = %channel, target_user = %user_id,
                    "automation notify_admin pushed"
                ),
                Ok(Err(e)) => tracing::warn!(
                    notify_type = %notify_type, target_user = %user_id, error = %e,
                    "automation notify_admin push failed"
                ),
                Err(e) => tracing::warn!(
                    notify_type = %notify_type, target_user = %user_id, error = %e,
                    "automation notify_admin join failed"
                ),
            }
        }
        Ok(())
    }

    /// Followers ledger (automation Phase 2). Thin substrate passthroughs —
    /// callers: the weixin-oa webhook (follow/touch/unfollow) and cron
    /// `Push`/`FollowerReport` actions (audience + growth stats).
    pub async fn follower_record_follow(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
        unionid: Option<&str>,
        scene: Option<&str>,
    ) -> CarrierResult<()> {
        self.memory
            .follower_record_follow(channel, app_id, openid, unionid, scene)
            .await
    }

    pub async fn follower_touch(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
    ) -> CarrierResult<()> {
        self.memory.follower_touch(channel, app_id, openid).await
    }

    pub async fn follower_mark_unfollowed(
        &self,
        channel: &str,
        app_id: &str,
        openid: &str,
    ) -> CarrierResult<()> {
        self.memory
            .follower_mark_unfollowed(channel, app_id, openid)
            .await
    }

    /// Active followers seen since `since_rfc3339` — the deliverable audience
    /// for a scheduled push (OA customer-service 48h window).
    pub async fn follower_list_pushable(
        &self,
        channel: &str,
        app_id: &str,
        since_rfc3339: &str,
    ) -> CarrierResult<Vec<String>> {
        self.memory
            .follower_list_pushable(channel, app_id, since_rfc3339)
            .await
    }

    pub async fn follower_stats(
        &self,
        channel: &str,
        app_id: &str,
        since_rfc3339: &str,
        push_window_since_rfc3339: &str,
    ) -> CarrierResult<carrier_memory::follower_store::FollowerStats> {
        self.memory
            .follower_stats(channel, app_id, since_rfc3339, push_window_since_rfc3339)
            .await
    }

    /// Unified push: deliver a `ContentDescriptor` (text/miniprogram/image/link)
    /// to any target — a specific user_id or `"admins"` (fan-out). Uses
    /// `channel_deliver_fn` (supports rich content on all channels). The agent
    /// turn is NOT affected (caller decides whether to skip agent).
    pub async fn do_push_message(
        &self,
        target: &str,
        content: &carrier_types::content::ContentDescriptor,
        source_agent_id: &str,
        source_bot_id: &str,
    ) -> carrier_types::error::CarrierResult<()> {
        use carrier_types::error::CarrierError;

        let deliver_fn = self
            .channel_deliver_fn
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        let deliver_fn = match deliver_fn {
            Some(f) => f,
            None => {
                return Err(CarrierError::Config(
                    "channel_deliver_fn not configured".into(),
                ))
            }
        };

        // Resolve recipients: (channel, bot_id, user_id) via sender_channels
        // (authoritative, multi-tenant safe) with prefix fallback. `"admins"`
        // fans out via admins.json.
        let cron_store = self.memory.cron_delivery();
        let recipients: Vec<(String, String, String)> = if target == "admins" {
            let ws = self
                .registry
                .resolve(source_agent_id)
                .ok()
                .and_then(|(_, entry)| entry.manifest.workspace.clone())
                .map(|p| p.to_string_lossy().to_string());
            match ws {
                Some(w) => {
                    let admins =
                        carrier_runtime::plugin::admin_store::read_admins(std::path::Path::new(&w));
                    admins
                        .admins
                        .into_iter()
                        .map(|a| {
                            carrier_memory::cron_delivery::route_recipient(
                                &a.sender_id,
                                cron_store,
                                source_bot_id,
                            )
                        })
                        .collect()
                }
                None => {
                    tracing::warn!(
                        agent_id = %source_agent_id, target = %target,
                        "push_message: target=admins but workspace unresolved"
                    );
                    Vec::new()
                }
            }
        } else {
            // Security gate: only push to recipients the bot has actually
            // interacted with (recorded in sender_channels). This blocks an
            // automation rule from pushing to an arbitrary陌生 openid via a
            // public keyword trigger, and also fails fast for doomed deliveries
            // — OA/wecom can only reach users who have followed / entered the
            // kf session, which is exactly when sender_channels gets a row.
            if cron_store.get_last_channel(target).ok().flatten().is_none() {
                return Err(CarrierError::InvalidInput(format!(
                    "cannot push to '{target}': no recorded interaction with this \
                     recipient (not in sender_channels). Have them send the bot a \
                     message first, or target 'admins'."
                )));
            }
            vec![carrier_memory::cron_delivery::route_recipient(
                target,
                cron_store,
                source_bot_id,
            )]
        };

        // Fail loudly on zero recipients or total delivery failure so the caller
        // (tool_message_push) surfaces an error instead of a false success.
        let total = recipients.len();
        if total == 0 {
            return Err(CarrierError::Config(format!(
                "push to '{target}': no recipients resolved"
            )));
        }

        let mut failed = 0usize;
        for (channel, bot_id, user_id) in recipients {
            let deliver_fn = std::sync::Arc::clone(&deliver_fn);
            let content = content.clone();
            let (ch, bot, user) = (channel.clone(), bot_id.clone(), user_id.clone());
            match tokio::task::spawn_blocking(move || deliver_fn(&ch, &bot, &user, &content)).await
            {
                Ok(Ok(())) => tracing::info!(
                    target_channel = %channel, target_user = %user_id,
                    "push_message delivered"
                ),
                Ok(Err(e)) => {
                    failed += 1;
                    tracing::warn!(
                        target_user = %user_id, error = %e,
                        "push_message failed"
                    );
                }
                Err(e) => {
                    failed += 1;
                    tracing::warn!(
                        target_user = %user_id, error = %e,
                        "push_message join failed"
                    );
                }
            }
        }

        if failed == total {
            return Err(CarrierError::Internal(format!(
                "push to '{target}': all {total} deliveries failed"
            )));
        }
        if failed > 0 {
            tracing::warn!(
                target = %target, failed, total,
                "push_message partial failure"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::manifest_to_capabilities;
    use carrier_types::capability::Capability;
    use std::collections::HashMap;

    /// 检索回指 regression: kv prefetch must read the SAME partition the
    /// writers use — `(agent_name, owner_id or "", sender_id or "")`. The old
    /// code read `(name, owner, owner)` (owner as user), so sender-partitioned
    /// drawer entries never reached the prompt and users would have seen the
    /// owner's drawer instead of their own. Also covers the session_compaction
    /// recall (two most recent, newest first) that the compaction write-back
    /// bridge persists.
    #[test]
    fn kv_memory_recall_uses_sender_partition() {
        let tmp = tempfile::tempdir().unwrap();
        let brain = serde_json::json!({
            "base_url": "http://127.0.0.1:1/v1/chat/completions",
            "api_key_env": "",
            "default_modality": "chat",
            "modalities": { "chat": { "description": "test" } }
        });
        std::fs::write(tmp.path().join("brain.json"), brain.to_string()).unwrap();
        let config = KernelConfig {
            home_dir: tmp.path().to_path_buf(),
            data_dir: tmp.path().join("data"),
            ..KernelConfig::default()
        };
        let kernel = CarrierKernel::boot_with_config(config).expect("kernel should boot");

        // Seed at the canonical writer partition (owner "" — every production
        // entry point passes owner_id=None).
        let handle = crate::handle::make_memory_handle(std::sync::Arc::clone(&kernel.memory));
        handle
            .kv_set(
                "mem-agent",
                "",
                "user-A",
                "preference.tone",
                serde_json::json!("casual"),
            )
            .unwrap();
        handle
            .kv_set(
                "mem-agent",
                "",
                "user-A",
                "session_compaction.2026-08-01",
                serde_json::json!("old summary"),
            )
            .unwrap();
        handle
            .kv_set(
                "mem-agent",
                "",
                "user-A",
                "session_compaction.2026-08-17",
                serde_json::json!("new summary"),
            )
            .unwrap();
        // Another user's data — must NOT leak into user-A's prefetch.
        handle
            .kv_set(
                "mem-agent",
                "",
                "user-B",
                "preference.secret",
                serde_json::json!("B-only"),
            )
            .unwrap();

        let (drawer, recalled) = kernel.prefetch_kv_memories("mem-agent", "", "user-A");

        // Drawer: own entries visible, other users' hidden.
        assert!(
            drawer.iter().any(|d| d.key == "preference.tone"),
            "sender-partitioned drawer entry must reach the prompt"
        );
        assert!(
            !drawer.iter().any(|d| d.key == "preference.secret"),
            "other users' drawer entries must not leak"
        );

        // Compaction recall: two most recent, newest first (date sort).
        assert_eq!(recalled.len(), 2);
        assert_eq!(recalled[0].0, "session_compaction.2026-08-17");
        assert_eq!(recalled[0].1, "new summary");
        assert_eq!(recalled[1].0, "session_compaction.2026-08-01");

        // The OLD read partition (owner as user) must not be what we read:
        // seeding there and asserting invisibility proves the coordinates.
        handle
            .kv_set(
                "mem-agent",
                "user-A",
                "user-A",
                "preference.legacy",
                serde_json::json!("stale partition"),
            )
            .unwrap();
        let (drawer2, _) = kernel.prefetch_kv_memories("mem-agent", "", "user-A");
        assert!(
            !drawer2.iter().any(|d| d.key == "preference.legacy"),
            "prefetch must read (owner, sender), not (owner-as-user, owner-as-user)"
        );

        kernel.shutdown();
    }

    #[test]
    fn test_manifest_to_capabilities() {
        let mut manifest = AgentManifest {
            name: "test".to_string(),
            display_name: String::new(),
            version: "0.1.0".to_string(),
            description: "test".to_string(),
            author: "test".to_string(),
            module: "test".to_string(),
            schedule: ScheduleMode::default(),
            model: ModelConfig::default(),
            resources: ResourceQuota::default(),
            priority: Priority::default(),
            capabilities: ManifestCapabilities::default(),
            profile: None,
            tools: HashMap::new(),
            flows: vec![],
            mcp_servers: vec![],
            max_tool_level: carrier_types::tool::PermissionLevel::Write,
            intent_classifier_enabled: None,
            default_flow: None,
            metadata: HashMap::new(),
            tags: vec![],
            autonomous: None,
            workspace: None,
            generate_identity_files: true,
            clone_source: None,
            exec_policy: None,
            cli_exec: None,
            tool_allowlist: vec![],
            tool_blocklist: vec![],
            knowledge_files: vec![],
            plugins: vec![],
            subagents: vec![],
        };
        manifest.capabilities.tools = vec!["file_read".to_string(), "web_fetch".to_string()];
        let caps = manifest_to_capabilities(&manifest);
        assert!(caps
            .iter()
            .any(|c| matches!(c, Capability::ToolInvoke(t) if t == "file_read")));
        assert!(caps
            .iter()
            .any(|c| matches!(c, Capability::ToolInvoke(t) if t == "web_fetch")));
    }

    fn test_manifest(name: &str, description: &str, tags: Vec<String>) -> AgentManifest {
        AgentManifest {
            name: name.to_string(),
            display_name: String::new(),
            version: "0.1.0".to_string(),
            description: description.to_string(),
            author: "test".to_string(),
            module: "test".to_string(),
            schedule: ScheduleMode::default(),
            model: ModelConfig::default(),
            resources: ResourceQuota::default(),
            priority: Priority::default(),
            capabilities: ManifestCapabilities::default(),
            profile: None,
            tools: HashMap::new(),
            flows: vec![],
            mcp_servers: vec![],
            max_tool_level: carrier_types::tool::PermissionLevel::Write,
            intent_classifier_enabled: None,
            default_flow: None,
            metadata: HashMap::new(),
            tags,
            autonomous: None,
            workspace: None,
            generate_identity_files: true,
            clone_source: None,
            exec_policy: None,
            cli_exec: None,
            tool_allowlist: vec![],
            tool_blocklist: vec![],
            knowledge_files: vec![],
            plugins: vec![],
            subagents: vec![],
        }
    }

    fn register_test_agent(
        registry: &AgentRegistry,
        name: &str,
        desc: &str,
        tags: Vec<String>,
    ) -> AgentId {
        use carrier_types::agent::{AgentEntry, AgentIdentity, AgentMode, AgentState, SessionId};
        let id = AgentId::new();
        let entry = AgentEntry {
            id,
            name: name.to_string(),
            manifest: test_manifest(name, desc, tags),
            state: AgentState::Running,
            mode: AgentMode::default(),
            created_at: chrono::Utc::now(),
            last_active: chrono::Utc::now(),
            parent: None,
            children: vec![],
            session_id: SessionId::new(),
            tags: vec![],
            identity: AgentIdentity::default(),
            onboarding_completed: false,
            onboarding_completed_at: None,
        };
        registry.register(entry).unwrap();
        id
    }

    #[test]
    fn test_send_to_agent_by_name_resolution() {
        let registry = AgentRegistry::new();
        let id = register_test_agent(
            &registry,
            "alice",
            "Alice agent",
            vec!["helper".to_string()],
        );
        assert!(registry.get(id).is_some());
        let found = registry.find_by_name("alice");
        assert!(found.is_some());
        assert_eq!(found.unwrap().id, id);
    }

    #[test]
    fn test_find_agents_by_tag() {
        let registry = AgentRegistry::new();
        register_test_agent(&registry, "bob", "Bob agent", vec!["coding".to_string()]);
        register_test_agent(
            &registry,
            "carol",
            "Carol agent",
            vec!["writing".to_string()],
        );
        let all = registry.list();
        let coding: Vec<_> = all
            .iter()
            .filter(|a| a.manifest.tags.contains(&"coding".to_string()))
            .collect();
        assert_eq!(coding.len(), 1);
        assert_eq!(coding[0].name, "bob");
    }

    #[test]
    fn test_manifest_to_capabilities_with_profile() {
        let mut manifest = test_manifest("profiled", "test", vec![]);
        manifest.profile = Some(carrier_types::agent::ToolProfile::Coding);
        let caps = manifest_to_capabilities(&manifest);
        assert!(!caps.is_empty());
    }

    #[test]
    fn test_manifest_to_capabilities_profile_overridden_by_explicit_tools() {
        let mut manifest = test_manifest("override", "test", vec![]);
        manifest.profile = Some(carrier_types::agent::ToolProfile::Coding);
        manifest.capabilities.tools = vec!["file_read".to_string()];
        let caps = manifest_to_capabilities(&manifest);
        assert_eq!(caps.len(), 1);
        assert!(caps
            .iter()
            .any(|c| matches!(c, Capability::ToolInvoke(t) if t == "file_read")));
    }
}
