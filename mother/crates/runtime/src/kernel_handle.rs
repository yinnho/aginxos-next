//! Trait abstraction for kernel operations needed by the agent runtime.
//!
//! This trait allows `carrier-runtime` to call back into the kernel for
//! inter-agent operations (spawn, send, list, kill) without creating
//! a circular dependency. The kernel implements this trait and passes
//! it into the agent loop.

use async_trait::async_trait;
use carrier_types::error::{CarrierError, CarrierResult};

/// Agent info returned by list and discovery operations.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    /// Human-readable Chinese display name (e.g. "小剪"); falls back to `name` when unset.
    pub display_name: String,
    pub state: String,
    pub modality: String,
    pub model: String,
    pub description: String,
    pub tags: Vec<String>,
    pub tools: Vec<String>,
}

/// Handle to kernel operations, passed into the agent loop so agents
/// can interact with each other via tools.
#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait KernelHandle: Send + Sync {
    /// Spawn a new agent from a TOML manifest string.
    /// `parent_id` is the UUID string of the spawning agent (for lineage tracking).
    /// Returns (agent_id, agent_name) on success.
    async fn spawn_agent(
        &self,
        manifest_toml: &str,
        parent_id: Option<&str>,
    ) -> CarrierResult<(String, String)>;

    /// Send a message to another agent and get the response.
    /// `sender_id` and `sender_name` identify the originating user (e.g. WeChat user).
    /// `caller_agent_id` is the agent invoking this tool, used for tenant isolation.
    /// `owner_id` is the route owner (the person who created the bot). When None,
    /// defaults to sender_id for backward compatibility.
    async fn send_to_agent(
        &self,
        agent_id: &str,
        message: &str,
        sender_id: Option<&str>,
        sender_name: Option<&str>,
        caller_agent_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
    ) -> CarrierResult<String>;

    /// Describe non-text content (image, voice, file, location) for the agent.
    ///
    /// Default return: hardcoded Chinese text description.
    /// Carriers with vision capabilities override this to call a vision model
    /// and return the model's description of the content.
    async fn describe_content(
        &self,
        _content_type: &str,
        _url: &str,
        _metadata: Option<&str>,
    ) -> CarrierResult<String> {
        Ok(format!("[用户发送了非文本内容: {_content_type}]"))
    }

    /// List all running agents visible to the caller.
    fn list_agents(&self) -> Vec<AgentInfo>;

    /// Kill an agent by ID.
    fn kill_agent(&self, agent_id: &str) -> CarrierResult<()>;

    /// Restart an agent by ID (reset state, re-read manifest from workspace).
    fn restart_agent(&self, agent_id: &str) -> CarrierResult<()>;

    /// Find agents by query (matches on name substring, tag, or tool name; case-insensitive).
    fn find_agents(&self, query: &str) -> Vec<AgentInfo>;

    /// Post a task to the shared task queue. Returns the task ID.
    async fn task_post(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
    ) -> CarrierResult<String>;

    /// Claim the next available task.
    async fn task_claim(&self, agent_id: &str) -> CarrierResult<Option<serde_json::Value>>;

    /// Mark a task as completed with a result string.
    async fn task_complete(&self, task_id: &str, result: &str) -> CarrierResult<()>;

    /// List tasks, optionally filtered by status.
    async fn task_list(&self, status: Option<&str>) -> CarrierResult<Vec<serde_json::Value>>;

    /// Publish a custom event that can trigger proactive agents.
    async fn publish_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> CarrierResult<()>;

    /// Create a cron job for the calling agent.
    async fn cron_create(
        &self,
        agent_id: &str,
        owner_id: Option<&str>,
        sender_id: Option<&str>,
        job_json: serde_json::Value,
    ) -> CarrierResult<String> {
        let _ = (agent_id, owner_id, sender_id, job_json);
        Err(CarrierError::Internal(
            "Cron scheduler not available".into(),
        ))
    }

    /// List cron jobs for the calling agent, optionally filtered by owner_id.
    async fn cron_list(
        &self,
        agent_id: &str,
        owner_id: Option<&str>,
    ) -> CarrierResult<Vec<serde_json::Value>> {
        let _ = (agent_id, owner_id);
        Err(CarrierError::Internal(
            "Cron scheduler not available".into(),
        ))
    }

    /// Cancel a cron job by ID.
    async fn cron_cancel(&self, job_id: &str) -> CarrierResult<()> {
        let _ = job_id;
        Err(CarrierError::Internal(
            "Cron scheduler not available".into(),
        ))
    }

    /// List automation rules for (channel, app_id), highest priority first.
    async fn automation_rule_list(
        &self,
        channel: &str,
        app_id: &str,
    ) -> CarrierResult<Vec<carrier_types::automation::AutomationRule>> {
        let _ = (channel, app_id);
        Err(CarrierError::Internal(
            "Automation rule store not available".into(),
        ))
    }

    /// Insert or update an automation rule.
    async fn automation_rule_upsert(
        &self,
        rule: carrier_types::automation::AutomationRule,
    ) -> CarrierResult<()> {
        let _ = rule;
        Err(CarrierError::Internal(
            "Automation rule store not available".into(),
        ))
    }

    /// Delete an automation rule by id.
    async fn automation_rule_delete(&self, id: &str) -> CarrierResult<()> {
        let _ = id;
        Err(CarrierError::Internal(
            "Automation rule store not available".into(),
        ))
    }

    /// Unified push: deliver a `ContentDescriptor` to any target (user_id or
    /// "admins"). Uses `channel_deliver_fn` (rich content on all channels).
    async fn push_message(
        &self,
        target: String,
        content: carrier_types::content::ContentDescriptor,
        source_agent_id: String,
        source_bot_id: String,
    ) -> CarrierResult<()> {
        let _ = (target, content, source_agent_id, source_bot_id);
        Err(CarrierError::Internal("push_message not available".into()))
    }

    /// Look up the `(channel_type, bot_id)` a sender most recently used, from
    /// the `sender_channels` table (written on every inbound). Sync — used by
    /// outbound routing (e.g. `process_notify_markers`) to route admin
    /// fan-out authoritatively instead of by id-prefix guesswork. Returns None
    /// when the sender has no recorded inbound (caller falls back to inference).
    fn resolve_sender_channel(&self, _sender_id: &str) -> Option<(String, String)> {
        None
    }

    /// List discovered external A2A agents as (name, url) pairs.
    fn list_a2a_agents(&self) -> Vec<(String, String)> {
        vec![]
    }

    /// Get the URL of a discovered external A2A agent by name.
    fn get_a2a_agent_url(&self, name: &str) -> Option<String> {
        let _ = name;
        None
    }

    /// Resolve an agent's workspace directory by name.
    /// Returns the absolute path string, or None if the agent is not found.
    fn resolve_agent_workspace(&self, agent_name: &str) -> Option<String> {
        let _ = agent_name;
        None
    }

    /// Rebuild the available tool list for an agent.
    /// Query a toolset from the registry and return its tools.
    /// Stateless — does not modify any session or agent state.
    fn get_toolset_tools(&self, _toolset_name: &str) -> Option<Vec<carrier_types::tool::ToolDefinition>> {
        None
    }

    /// Search the tool catalog for tools matching a query.
    /// Returns (toolset_name, ToolDefinition) pairs ranked by relevance.
    fn search_tools(
        &self,
        query: &str,
        limit: usize,
        max_level: carrier_types::tool::PermissionLevel,
    ) -> Vec<(String, carrier_types::tool::ToolDefinition)> {
        let _ = (query, limit, max_level);
        Vec::new()
    }

    /// Execute a plugin (channel) tool by name via the PluginToolDispatcher.
    ///
    /// Returns `None` if no dispatcher is registered or the tool isn't a plugin
    /// tool (so the caller can fall through to other dispatch paths).
    /// Returns `Ok(Some(content))` on success, `Ok(None)` if no plugin handles
    /// the tool, or `Err(_)` if a plugin handled it but execution failed.
    fn execute_plugin_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        context: &carrier_types::plugin::PluginToolContext,
    ) -> CarrierResult<Option<String>> {
        let _ = (tool_name, args, context);
        Ok(None)
    }

    /// Get the home directory path (~/.aginx/carrier/).
    fn home_dir(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Public base URL for constructing file `view_url`s (e.g. `https://file.yinnho.cn`).
    fn external_url(&self) -> Option<String> {
        None
    }

    /// Deliver rich content by key for an agent, without running an agent loop.
    /// Scripts/cron call this to send `[DELIVER:key]`-equivalent content directly
    /// to a user on a given channel/bot. Default implementation returns an error
    /// - real kernels override it with the wired-up `channel_deliver_fn`.
    fn deliver_content(
        &self,
        _agent: &str,
        _content_key: &str,
        _channel_type: &str,
        _bot_id: &str,
        _user_id: &str,
    ) -> CarrierResult<()> {
        Err(CarrierError::Internal(
            "deliver_content not implemented by this kernel".into(),
        ))
    }

    /// Spawn an agent with capability inheritance enforcement.
    /// `parent_caps` are the parent's granted capabilities. The kernel MUST verify
    /// that every capability in the child manifest is covered by `parent_caps`.
    async fn spawn_agent_checked(
        &self,
        manifest_toml: &str,
        parent_id: Option<&str>,
        parent_caps: &[carrier_types::capability::Capability],
    ) -> CarrierResult<(String, String)> {
        // Default: delegate to spawn_agent (no enforcement)
        // The kernel MUST override this with real enforcement
        let _ = parent_caps;
        self.spawn_agent(manifest_toml, parent_id).await
    }

    /// Install a clone from definition-layer files (`path -> bytes`).
    ///
    /// Writes every file under `workspaces/<name>/`, builds `agent.toml` from the
    /// resulting workspace, and spawns the agent. Returns
    /// `(agent_id, agent_name, display_name)`. Executed kernel-side by the
    /// `[CLONE_INSTALL:<name>]` reply-marker handler (clone_marker.rs) — the
    /// old `clone_install` agent tool was retired (giant-payload turns timed
    /// out; staging + marker replaced it).
    ///
    /// Default: unavailable — real kernels override this to delegate to their
    /// existing `clone_install_files` inherent method.
    async fn clone_install_files(
        &self,
        _name: &str,
        _files: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> CarrierResult<(String, String, String)> {
        Err(CarrierError::Internal(
            "clone_install_files not available on this kernel".into(),
        ))
    }

    /// Read the configured Hub `(url, api_key)` for `clone_publish`.
    ///
    /// Returns `None` when the hub url or api key is unconfigured (so the tool
    /// can surface a clear error instead of a generic network failure).
    fn clone_hub_config(&self) -> Option<(String, String)> {
        None
    }

    /// Unbound-inbound fallback target (system identity「me」). None/empty =
    /// keep the bind-only routing law (drop unbound messages).
    fn inbound_fallback_agent(&self) -> Option<String> {
        None
    }
}
