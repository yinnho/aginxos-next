//! KernelHandle trait implementation — the runtime-to-kernel interface.
//!
//! Implements the `KernelHandle` trait for `CarrierKernel`, providing agent
//! spawning, messaging, memory, task, cron, A2A, clone, and plugin operations.

use async_trait::async_trait;
use carrier_runtime::kernel_handle::{self, KernelHandle};
use carrier_runtime::llm_driver::CompletionRequest;
use carrier_runtime::memory_handle::MemoryHandle;
use carrier_types::agent::{AgentId, AgentManifest};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::event::*;
use carrier_types::message::{ContentBlock, Message, MessageContent, Role};
use std::sync::Arc;

/// Well-known agent ID for system/kernel-originated events.
pub const SYSTEM_AGENT_ID: AgentId = AgentId(uuid::Uuid::nil());

use crate::capabilities::manifest_to_capabilities;
use crate::kernel::CarrierKernel;
use carrier_memory::MemorySubstrate;

// ── Export helper ──────────────────────────────────────────

// ── KernelHandle trait implementation ─────────────────────

#[async_trait]
impl KernelHandle for CarrierKernel {
    async fn spawn_agent(
        &self,
        manifest_toml: &str,
        parent_id: Option<&str>,
    ) -> CarrierResult<(String, String)> {
        let content_hash = carrier_types::manifest_signing::hash_manifest(manifest_toml);
        tracing::debug!(hash = %content_hash, "Manifest SHA-256 computed for integrity tracking");

        let manifest: AgentManifest = toml::from_str(manifest_toml)
            .map_err(|e| CarrierError::ManifestParse(format!("Invalid manifest: {e}")))?;
        let name = manifest.name.clone();
        let parent = parent_id.and_then(|pid| pid.parse::<AgentId>().ok());
        let id = self.spawn_agent_with_parent(manifest, parent, None)?;
        Ok((id.to_string(), name))
    }

    async fn send_to_agent(
        &self,
        agent_id: &str,
        message: &str,
        sender_id: Option<&str>,
        sender_name: Option<&str>,
        _caller_agent_id: Option<&str>,
        owner_id: Option<&str>,
        channel_type: Option<&str>,
    ) -> CarrierResult<String> {
        let (id, _target_entry) = self.registry.resolve(agent_id)?;

        let handle: Option<Arc<dyn KernelHandle>> = self
            .coordination
            .self_handle
            .get()
            .and_then(|w| w.upgrade())
            .map(|arc| arc as Arc<dyn KernelHandle>);

        let result = self
            .send_message_with_handle(
                id,
                message,
                handle,
                sender_id.map(|s| s.to_string()),
                sender_name.map(|s| s.to_string()),
                owner_id.map(|s| s.to_string()),
                channel_type.map(|s| s.to_string()),
                None,
                None,
            )
            .await?;

        Ok(result.response)
    }

    async fn describe_content(
        &self,
        content_type: &str,
        url: &str,
        _metadata: Option<&str>,
    ) -> CarrierResult<String> {
        if content_type != "image" {
            return Ok(format!("[用户发送了非文本内容: {content_type}]"));
        }

        // Prefer HTTP(S) URL → vision provider fetches the image itself.
        // Avoids embedding large base64 payloads (token bloat / timeouts).
        let image_block = if url.starts_with("https://") || url.starts_with("http://") {
            // Soft SSRF guard: block obvious private-network targets even when
            // the provider does the fetch (we still don't want to pass them).
            carrier_types::ssrf::check_ssrf(url)?;
            let mime = mime_from_image_url(url);
            tracing::info!(%url, %mime, "Vision describe via public URL (no base64)");
            ContentBlock::Image {
                media_type: mime,
                data: String::new(),
                url: Some(url.to_string()),
            }
        } else if let Some(rest) = url.strip_prefix("data:") {
            // Legacy data-URI path (fallback only).
            let sep = rest
                .find(";base64,")
                .ok_or_else(|| CarrierError::InvalidInput("Invalid data URI format".into()))?;
            let mime = rest[..sep].to_string();
            let b64 = rest[sep + ";base64,".len()..].to_string();
            let max_b64 = 5 * 1024 * 1024 * 2;
            if b64.len() > max_b64 {
                return Err(CarrierError::InvalidInput(format!(
                    "Image too large (data URI): {} chars",
                    b64.len()
                )));
            }
            tracing::warn!(
                b64_len = b64.len(),
                "Vision describe falling back to data URI base64"
            );
            ContentBlock::Image {
                media_type: mime,
                data: b64,
                url: None,
            }
        } else {
            let preview: String = url.chars().take(80).collect();
            return Err(CarrierError::InvalidInput(format!(
                "Unsupported image reference (need https:// URL or data URI): {preview}"
            )));
        };

        let request = CompletionRequest {
            model: String::new(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![
                    image_block,
                    ContentBlock::Text {
                        text: "请详细描述这张图片的内容。".to_string(),
                        provider_metadata: None,
                    },
                ]),
            }],
            tools: vec![],
            max_tokens: 1024,
            temperature: 0.3,
            system: None,
            thinking: None,
            extra: Default::default(),
        };

        let brain: Arc<dyn carrier_runtime::llm_driver::Brain> = Arc::clone(
            &*self
                .brain
                .brain
                .read()
                .map_err(|e| CarrierError::Internal(format!("Brain lock: {e}")))?,
        )
            as Arc<dyn carrier_runtime::llm_driver::Brain>;

        let result = brain
            .complete("vision", request)
            .await
            .map_err(|e| CarrierError::LlmDriver(format!("Vision call failed: {e}")))?;

        let description = result.text();
        if description.is_empty() {
            return Err(CarrierError::LlmDriver(
                "Vision model returned empty description".into(),
            ));
        }

        tracing::info!(
            content_type,
            desc_len = description.len(),
            via_url = url.starts_with("http"),
            "Content described by vision model"
        );
        Ok(description)
    }

    fn list_agents(&self) -> Vec<kernel_handle::AgentInfo> {
        let agents = self.registry.list();
        agents
            .into_iter()
            .map(|e| {
                let (modality, model) = self.resolve_model_label(&e.manifest.model.modality);
                kernel_handle::AgentInfo {
                    id: e.id.to_string(),
                    name: e.name.clone(),
                    display_name: e.manifest.display_name.clone(),
                    state: format!("{:?}", e.state),
                    modality,
                    model,
                    description: e.manifest.description.clone(),
                    tags: e.tags.clone(),
                    tools: e.manifest.capabilities.tools.clone(),
                }
            })
            .collect()
    }

    fn kill_agent(&self, agent_id: &str) -> CarrierResult<()> {
        let (id, _) = self.registry.resolve(agent_id)?;
        CarrierKernel::kill_agent(self, id).map_err(CarrierError::from)
    }

    fn restart_agent(&self, agent_id: &str) -> CarrierResult<()> {
        let (id, _) = self.registry.resolve(agent_id)?;
        self.stop_agent_run(id)?;

        // Re-read agent.toml from workspace to pick up tool/capability changes
        // (shared path with the API restart route; also fills empty
        // display_name/description from template.json).
        self.reload_manifest_from_workspace(id);

        self.registry
            .set_state(id, carrier_types::agent::AgentState::Running)?;
        Ok(())
    }

    fn find_agents(&self, query: &str) -> Vec<kernel_handle::AgentInfo> {
        let q = query.to_lowercase();
        let agents = self.registry.list();
        agents
            .into_iter()
            .filter(|e| {
                let name_match = e.name.to_lowercase().contains(&q);
                let tag_match = e.tags.iter().any(|t| t.to_lowercase().contains(&q));
                let tool_match = e
                    .manifest
                    .capabilities
                    .tools
                    .iter()
                    .any(|t| t.to_lowercase().contains(&q));
                let desc_match = e.manifest.description.to_lowercase().contains(&q);
                name_match || tag_match || tool_match || desc_match
            })
            .map(|e| {
                let (modality, model) = self.resolve_model_label(&e.manifest.model.modality);
                kernel_handle::AgentInfo {
                    id: e.id.to_string(),
                    name: e.name.clone(),
                    display_name: e.manifest.display_name.clone(),
                    state: format!("{:?}", e.state),
                    modality,
                    model,
                    description: e.manifest.description.clone(),
                    tags: e.tags.clone(),
                    tools: e.manifest.capabilities.tools.clone(),
                }
            })
            .collect()
    }

    async fn task_post(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
    ) -> CarrierResult<String> {
        self.memory
            .task_post(title, description, assigned_to, created_by)
            .await
    }

    async fn task_claim(&self, agent_id: &str) -> CarrierResult<Option<serde_json::Value>> {
        self.memory.task_claim(agent_id).await
    }

    async fn task_complete(&self, task_id: &str, result: &str) -> CarrierResult<()> {
        self.memory.task_complete(task_id, result).await
    }

    async fn task_list(&self, status: Option<&str>) -> CarrierResult<Vec<serde_json::Value>> {
        self.memory.task_list(status).await
    }

    async fn automation_rule_list(
        &self,
        channel: &str,
        app_id: &str,
    ) -> CarrierResult<Vec<carrier_types::automation::AutomationRule>> {
        self.memory.automation_rule_list(channel, app_id).await
    }

    async fn automation_rule_upsert(
        &self,
        rule: carrier_types::automation::AutomationRule,
    ) -> CarrierResult<()> {
        self.memory.automation_rule_upsert(rule).await
    }

    async fn automation_rule_delete(&self, id: &str) -> CarrierResult<()> {
        self.memory.automation_rule_delete(id).await
    }

    async fn push_message(
        &self,
        target: String,
        content: carrier_types::content::ContentDescriptor,
        source_agent_id: String,
        source_bot_id: String,
    ) -> CarrierResult<()> {
        self.do_push_message(&target, &content, &source_agent_id, &source_bot_id)
            .await
    }

    fn resolve_sender_channel(&self, sender_id: &str) -> Option<(String, String)> {
        self.memory
            .cron_delivery()
            .get_last_channel(sender_id)
            .ok()
            .flatten()
            .map(|lc| (lc.channel_type, lc.bot_id))
    }

    async fn publish_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> CarrierResult<()> {
        let system_agent = SYSTEM_AGENT_ID;
        let payload_bytes =
            serde_json::to_vec(&serde_json::json!({"type": event_type, "data": payload}))
                .map_err(|e| CarrierError::Serialization(e.to_string()))?;
        let event = Event::new(
            system_agent,
            EventTarget::Broadcast,
            EventPayload::Custom(payload_bytes),
        );
        CarrierKernel::publish_event(self, event).await;
        Ok(())
    }

    async fn cron_create(
        &self,
        agent_id: &str,
        owner_id: Option<&str>,
        sender_id: Option<&str>,
        job_json: serde_json::Value,
    ) -> CarrierResult<String> {
        use carrier_types::scheduler::{
            CronAction, CronDelivery, CronJob, CronJobId, CronSchedule,
        };

        let name = job_json["name"]
            .as_str()
            .ok_or_else(|| CarrierError::InvalidInput("'name' must be a string".into()))?
            .to_string();
        let schedule: CronSchedule = {
            let schedule_val = job_json
                .get("schedule")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            // LLMs sometimes wrap the schedule in a string; unwrap it.
            let resolved = match &schedule_val {
                serde_json::Value::String(s) => {
                    serde_json::from_str::<serde_json::Value>(s).unwrap_or(schedule_val)
                }
                other => other.clone(),
            };
            // Relative one-shot time ({"kind":"at","in_secs":N}): resolve the
            // absolute fire time SERVER-side so agents never do timezone math
            // (08-19 白云调图事故: agent-computed `at` landed in the past
            // twice with "scheduled time must be in the future" rejections).
            let resolved = resolve_relative_at(resolved)?;
            serde_json::from_value(resolved)
                .map_err(|e| CarrierError::Serialization(format!("Invalid schedule: {e}")))?
        };
        let action: CronAction = {
            let action_val = job_json
                .get("action")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let resolved = match &action_val {
                serde_json::Value::String(s) => {
                    serde_json::from_str::<serde_json::Value>(s).unwrap_or(action_val)
                }
                other => other.clone(),
            };
            serde_json::from_value(resolved)
                .map_err(|e| CarrierError::Serialization(format!("Invalid action: {e}")))?
        };
        let delivery: CronDelivery = {
            let val = job_json
                .get("delivery")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            if val.is_null() {
                // Default to LastChannel when owner_id is set so cron results
                // are pushed to the user automatically.
                if owner_id.is_some() {
                    CronDelivery::LastChannel
                } else {
                    CronDelivery::None
                }
            } else {
                let resolved = match &val {
                    serde_json::Value::String(s) => {
                        serde_json::from_str::<serde_json::Value>(s).unwrap_or_else(|_| val.clone())
                    }
                    other => other.clone(),
                };
                if resolved.is_object() {
                    serde_json::from_value(resolved).map_err(|e| {
                        CarrierError::Serialization(format!("Invalid delivery: {e}"))
                    })?
                } else {
                    tracing::warn!("delivery is not an object, defaulting to None: {val}");
                    CronDelivery::None
                }
            }
        };
        let one_shot = match job_json.get("one_shot") {
            Some(v) => match v {
                serde_json::Value::Bool(b) => *b,
                serde_json::Value::String(s) => {
                    matches!(s.to_lowercase().as_str(), "true" | "1" | "yes")
                }
                _ => false,
            },
            None => false,
        };

        tracing::debug!(agent_id, "cron_create resolving agent_id");
        let (aid, _) = self.registry.resolve(agent_id)?;

        // Chained-pipeline identity (optional; Plan A of broken-chain
        // monitoring). Validated structurally so a malformed chain from the
        // LLM is rejected with a message it can self-heal from, not silently
        // stored as a broken expectation.
        let chain: Option<carrier_types::scheduler::ChainMeta> = match job_json.get("chain") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => {
                let c: carrier_types::scheduler::ChainMeta = serde_json::from_value(v.clone())
                    .map_err(|e| CarrierError::InvalidInput(format!(
                        "Invalid chain metadata: {e}. Expected {{chain_id, step, total_steps}} — \
                         chain_id=pipeline id, step=1-based current step, total_steps=chain length; \
                         step==total_steps marks the tail (creates no successor)."
                    )))?;
                if c.chain_id.trim().is_empty() {
                    return Err(CarrierError::InvalidInput(
                        "chain.chain_id must be a non-empty pipeline id".into(),
                    ));
                }
                // Identity/path-safety checks (see validate_chain_identity).
                validate_chain_identity(&c, &action)?;
                if c.step < 1 || c.total_steps < 1 || c.step > c.total_steps {
                    return Err(CarrierError::InvalidInput(format!(
                        "chain step/total_steps out of range: step={} total_steps={} \
                         (need 1 <= step <= total_steps)",
                        c.step, c.total_steps
                    )));
                }
                Some(c)
            }
        };

        // Captured before `job` consumes it: the chain progress hook below
        // needs (chain_id, step) after add_job succeeds.
        let chain_meta = chain.clone();
        let job = CronJob {
            id: CronJobId::new(),
            agent_id: aid,
            owner_id: owner_id.map(|s| s.to_string()),
            sender_id: sender_id.map(|s| s.to_string()),
            name,
            schedule,
            action,
            delivery,
            chain,
            enabled: true,
            created_at: chrono::Utc::now(),
            next_run: None,
            last_run: None,
        };

        let id = self.cron_scheduler.add_job(job, one_shot)?;

        // Chain progress hook (断链自动接续): an agent/human scheduling a
        // chained step is ground-truth chain progress — zero that
        // (chain_id, step)'s auto-resume budget so retries start fresh.
        // Daemon-issued resume jobs call `add_job` directly (not this trait
        // method), so they bump instead of reset — disjoint by construction.
        if let Some(c) = &chain_meta {
            if let Err(e) = self.memory.chain_resume().reset(&c.chain_id, c.step) {
                tracing::warn!(
                    chain_id = %c.chain_id,
                    step = c.step,
                    "Chain resume-budget reset failed: {e}"
                );
            }
        }

        if let Err(e) = self.cron_scheduler.persist() {
            tracing::warn!("Failed to persist cron jobs: {e}");
        }

        Ok(serde_json::json!({
            "job_id": id.to_string(),
            "status": "created"
        })
        .to_string())
    }

    async fn cron_list(
        &self,
        agent_id: &str,
        owner_id: Option<&str>,
    ) -> CarrierResult<Vec<serde_json::Value>> {
        let (aid, _) = self.registry.resolve(agent_id)?;
        let mut jobs = self.cron_scheduler.list_jobs(aid);
        if let Some(oid) = owner_id {
            jobs.retain(|j| j.owner_id.as_deref() == Some(oid));
        }
        let json_jobs: Vec<serde_json::Value> = jobs
            .into_iter()
            .map(|j| serde_json::to_value(&j).unwrap_or_default())
            .collect();
        Ok(json_jobs)
    }

    async fn cron_cancel(&self, job_id: &str) -> CarrierResult<()> {
        let id = carrier_types::scheduler::CronJobId(
            uuid::Uuid::parse_str(job_id)
                .map_err(|e| CarrierError::InvalidInput(format!("Invalid job ID: {e}")))?,
        );
        self.cron_scheduler.remove_job(id)?;

        if let Err(e) = self.cron_scheduler.persist() {
            tracing::warn!("Failed to persist cron jobs: {e}");
        }

        Ok(())
    }

    fn list_a2a_agents(&self) -> Vec<(String, String)> {
        self.a2a.cleanup_stale_agents();
        let agents = self
            .a2a
            .a2a_external_agents
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        agents
            .iter()
            .map(|(_, card, _)| (card.name.clone(), card.url.clone()))
            .collect()
    }

    fn get_a2a_agent_url(&self, name: &str) -> Option<String> {
        self.a2a.cleanup_stale_agents();
        let agents = self
            .a2a
            .a2a_external_agents
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let name_lower = name.to_lowercase();
        agents
            .iter()
            .find(|(_, card, _)| card.name.to_lowercase() == name_lower)
            .map(|(_, card, _)| card.url.clone())
    }

    async fn spawn_agent_checked(
        &self,
        manifest_toml: &str,
        parent_id: Option<&str>,
        parent_caps: &[carrier_types::capability::Capability],
    ) -> CarrierResult<(String, String)> {
        let child_manifest: AgentManifest = toml::from_str(manifest_toml)
            .map_err(|e| CarrierError::ManifestParse(format!("Invalid manifest: {e}")))?;
        let child_caps = manifest_to_capabilities(&child_manifest);

        carrier_types::capability::validate_capability_inheritance(parent_caps, &child_caps)?;

        tracing::info!(
            parent = parent_id.unwrap_or("kernel"),
            child = %child_manifest.name,
            child_caps = child_caps.len(),
            "Capability inheritance validated — spawning child agent"
        );

        KernelHandle::spawn_agent(self, manifest_toml, parent_id).await
    }

    fn home_dir(&self) -> Option<std::path::PathBuf> {
        Some(self.config.home_dir.clone())
    }

    fn external_url(&self) -> Option<String> {
        self.config.external_url.clone()
    }

    fn resolve_agent_workspace(&self, agent_name: &str) -> Option<String> {
        // Accept either agent name or UUID string — callers (esp. cron) may pass
        // either form. Workspace path still comes from the manifest (name-based dir).
        self.registry
            .resolve(agent_name)
            .ok()
            .and_then(|(_, entry)| entry.manifest.workspace.clone())
            .map(|p| p.to_string_lossy().to_string())
    }

    fn deliver_content(
        &self,
        agent: &str,
        content_key: &str,
        channel_type: &str,
        bot_id: &str,
        user_id: &str,
    ) -> CarrierResult<()> {
        let ws = self.resolve_agent_workspace(agent).ok_or_else(|| {
            CarrierError::AgentNotFound(format!(
                "deliver_content: agent {agent} not found or has no workspace"
            ))
        })?;
        let ws_path = std::path::Path::new(&ws);
        let config = carrier_runtime::outbound::ContentRegistry::global()
            .load(agent, ws_path)
            .ok_or_else(|| {
                CarrierError::Internal(format!(
                    "deliver_content: failed to load content.toml for agent {agent} under {}",
                    ws_path.display()
                ))
            })?;
        let desc = config.get(content_key).cloned().ok_or_else(|| {
            CarrierError::Internal(format!(
                "deliver_content: key '{content_key}' not found in {}/content.toml",
                ws_path.display()
            ))
        })?;

        let guard = self
            .channel_deliver_fn
            .read()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let deliver_fn = guard.as_ref().ok_or_else(|| {
            CarrierError::Config("deliver_content: channel_deliver_fn not wired".into())
        })?;
        deliver_fn(channel_type, bot_id, user_id, &desc)
            .map_err(|e| CarrierError::Network(format!("deliver_content: {e}")))
    }

    fn get_toolset_tools(
        &self,
        toolset_name: &str,
    ) -> Option<Vec<carrier_types::tool::ToolDefinition>> {
        let registry = self.plugins.toolset_registry.read().ok()?;

        // Resolve the registry key — try direct match first, then normalize-matching
        let resolved_key = if registry.contains_key(toolset_name) {
            toolset_name.to_string()
        } else {
            let normalized = carrier_runtime::mcp::normalize_name(toolset_name);
            registry
                .keys()
                .find(|k| carrier_runtime::mcp::normalize_name(k) == normalized)
                .cloned()?
        };

        let tools = registry.get(&resolved_key).cloned()?;
        if tools.is_empty() {
            None
        } else {
            Some(tools)
        }
    }

    fn search_tools(
        &self,
        query: &str,
        limit: usize,
        max_level: carrier_types::tool::PermissionLevel,
    ) -> Vec<(String, carrier_types::tool::ToolDefinition)> {
        let registry = match self.plugins.toolset_registry.read() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("toolset_registry read poisoned: {e}");
                return Vec::new();
            }
        };
        let query_lower = query.to_lowercase();
        let keywords: Vec<&str> = query_lower
            .split_whitespace()
            .filter(|w| w.len() >= 2)
            .collect();
        let mut scored: Vec<(usize, String, carrier_types::tool::ToolDefinition)> = Vec::new();

        // Search builtin toolsets
        for (ts_name, tools) in registry.iter() {
            let ts_lower = ts_name.to_lowercase();
            for tool in tools {
                let name_lower = tool.name.to_lowercase();
                let desc_lower = tool.description.to_lowercase();
                let score = CarrierKernel::score_tool(
                    &query_lower,
                    &keywords,
                    &name_lower,
                    &desc_lower,
                    &ts_lower,
                );
                if score > 0 {
                    scored.push((score, ts_name.clone(), tool.clone()));
                }
            }
        }

        // Search MCP servers — return individual tools so the agent can call them directly.
        for entry in self.plugins.mcp_connections.iter() {
            let conn = entry.value();
            let config = conn.config();
            let server_name = config.name.to_lowercase();
            let server_desc = config.description.to_lowercase();
            let server_score = CarrierKernel::score_tool(
                &query_lower,
                &keywords,
                &server_name,
                &server_desc,
                &server_name,
            );
            let ts = format!("mcp_{}", carrier_runtime::mcp::normalize_name(&config.name));
            for tool in conn.tools() {
                let name_lower = tool.name.to_lowercase();
                let desc_lower = tool.description.to_lowercase();
                let tool_score = CarrierKernel::score_tool(
                    &query_lower,
                    &keywords,
                    &name_lower,
                    &desc_lower,
                    &server_name,
                );
                let score = if tool_score > 0 {
                    tool_score
                } else {
                    server_score
                };
                if score > 0 {
                    // conn.tools() already returns namespaced names (e.g. mcp_wechat_oa_create_draft)
                    scored.push((
                        score + 50,
                        ts.clone(),
                        carrier_types::tool::ToolDefinition {
                            name: tool.name.clone(),
                            description: tool.description.clone(),
                            input_schema: tool.input_schema.clone(),
                        },
                    ));
                }
            }
        }

        // Search plugin tool dispatcher — remaining channel tools (e.g.
        // charter_create_order, weixin_oa_publish_article) registered as
        // ToolProvider instances. Rich content delivery now uses the unified
        // Channel::deliver path and [DELIVER:key] markers instead of channel-
        // specific send tools. These are exact-match candidates: flow-declared
        // tool names must resolve here. Flow tool resolution passes the exact
        // tool name as the query, so prefer a high exact-match score.
        if let Some(dispatcher) = self
            .plugins
            .plugin_tool_dispatcher
            .lock()
            .ok()
            .and_then(|g| g.clone())
        {
            for tool in dispatcher.definitions() {
                let name_lower = tool.name.to_lowercase();
                let exact = name_lower == query_lower;
                let score = if exact {
                    1000 // flow-declared exact match — always wins
                } else {
                    CarrierKernel::score_tool(
                        &query_lower,
                        &keywords,
                        &name_lower,
                        &tool.description.to_lowercase(),
                        "plugin",
                    )
                };
                if score > 0 {
                    scored.push((score, "plugin".to_string(), tool));
                }
            }
        }

        scored.sort_by_key(|s| std::cmp::Reverse(s.0));

        // Filter by max_level. Dangerous tools (e.g. shell_exec) are only
        // visible when max_level is Dangerous — typically via system-flow
        // turn elevation, not a permanent agent grant.
        scored.retain(|(_, _, def)| {
            let level = carrier_types::tool::PermissionLevel::for_tool(&def.name);
            level <= max_level
        });

        let count = scored.len();
        scored.truncate(limit);
        tracing::info!(
            query = query,
            results = scored.len(),
            total_candidates = count,
            "tool catalog search executed"
        );
        scored.into_iter().map(|(_, ts, def)| (ts, def)).collect()
    }

    fn execute_plugin_tool(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        context: &carrier_types::plugin::PluginToolContext,
    ) -> CarrierResult<Option<String>> {
        let dispatcher = self
            .plugins
            .plugin_tool_dispatcher
            .lock()
            .ok()
            .and_then(|g| g.clone());
        let Some(dispatcher) = dispatcher else {
            return Ok(None);
        };
        if !dispatcher.has_tool(tool_name) {
            return Ok(None);
        }
        Ok(Some(dispatcher.execute(tool_name, args, context)?))
    }

    async fn clone_install_files(
        &self,
        name: &str,
        files: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> CarrierResult<(String, String, String)> {
        CarrierKernel::clone_install_files(self, name, files).await
    }

    /// Read the configured Hub `(url, api_key)` for `clone_publish`.
    /// Mirrors the config access used by kernel.rs / api::routes::hub.rs.
    fn clone_hub_config(&self) -> Option<(String, String)> {
        let url = self.config.hub.url.clone();
        let key = carrier_clone::hub::read_api_key(&self.config.hub.api_key_env).ok()?;
        if url.is_empty() || key.is_empty() {
            return None;
        }
        Some((url, key))
    }

    fn inbound_fallback_agent(&self) -> Option<String> {
        self.config
            .inbound_fallback_agent
            .clone()
            .filter(|s| !s.is_empty())
    }
}

type ToolsetAlias = (fn(&str) -> bool, &'static str);

// Non-trait methods on CarrierKernel (called directly, not via KernelHandle)
impl CarrierKernel {
    /// Score a tool against a search query using multi-signal matching.
    fn score_tool(
        query: &str,
        keywords: &[&str],
        tool_name: &str,
        tool_desc: &str,
        toolset_name: &str,
    ) -> usize {
        let mut score: usize = 0;

        if tool_name == query {
            return 20;
        }
        if tool_name.contains(query) {
            score += 10;
        }
        for kw in keywords {
            if tool_name.contains(kw) {
                score += 5;
            }
        }
        if tool_desc.contains(query) {
            score += 5;
        }
        for kw in keywords {
            if tool_desc.contains(kw) {
                score += 2;
            }
        }
        if toolset_name.contains(query) {
            score += 3;
        }
        for kw in keywords {
            if toolset_name.contains(kw) {
                score += 2;
            }
        }

        let aliases: &[ToolsetAlias] = &[
            (
                |q: &str| {
                    q.contains("file")
                        || q.contains("save")
                        || q.contains("read")
                        || q.contains("write")
                },
                "filesystem",
            ),
            (
                |q: &str| {
                    q.contains("browser")
                        || q.contains("browse")
                        || q.contains("网页")
                        || q.contains("打开")
                },
                "browser",
            ),
            (
                |q: &str| {
                    q.contains("wechat")
                        || q.contains("微信")
                        || q.contains("公众号")
                        || q.contains("draft")
                },
                "wechat-oa",
            ),
            (
                |q: &str| q.contains("feishu") || q.contains("飞书") || q.contains("lark"),
                "feishu",
            ),
            (
                |q: &str| q.contains("wecom") || q.contains("企微") || q.contains("企业微信"),
                "wecom",
            ),
            (
                |q: &str| {
                    q.contains("shell")
                        || q.contains("command")
                        || q.contains("exec")
                        || q.contains("终端")
                },
                "shell",
            ),
            (
                |q: &str| {
                    q.contains("image")
                        || q.contains("图片")
                        || q.contains("media")
                        || q.contains("photo")
                },
                "media",
            ),
            (
                |q: &str| q.contains("search") || q.contains("fetch") || q.contains("web"),
                "web",
            ),
        ];
        for (matches, ts) in aliases {
            if matches(query) && toolset_name == *ts {
                score += 4;
            }
        }

        score
    }

    /// Install a clone from a file-level manifest + fetched files (dup file-level
    /// path). Writes files via `carrier_clone::write_files_to_workspace`, then
    /// build_manifest_from_workspace -> agent.toml -> spawn -> plugins.
    pub async fn clone_install_files(
        &self,
        name: &str,
        files: std::collections::BTreeMap<String, Vec<u8>>,
    ) -> CarrierResult<(String, String, String)> {
        use carrier_clone::{build_manifest_from_workspace, write_files_to_workspace};

        if name.is_empty()
            || name.len() > 64
            || name.starts_with('-')
            || name.ends_with('-')
            || !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(CarrierError::Internal(format!(
                "Invalid clone name '{}': must be 1-64 lowercase alphanumeric/hyphen characters",
                name
            )));
        }

        let workspace_dir = self.config.effective_workspaces_dir().join(name);
        if !workspace_dir.starts_with(self.config.effective_workspaces_dir()) {
            return Err(CarrierError::Internal("Path traversal denied".into()));
        }

        let clone_name = name.to_string();

        // Re-install (overwrite) support: if an agent is already registered or a
        // workspace already exists for this name, tear down the old one first so
        // regeneration doesn't hit a wall. The `.dup/` version-history dir is
        // preserved (keeps the clone's dup-push history); everything else is
        // cleared for a clean reinstall.
        if let Some(entry) = self.registry.find_by_name(&clone_name) {
            tracing::info!(
                name = %clone_name,
                old_id = %entry.id,
                "Clone already registered - killing existing agent for reinstall"
            );
            self.kill_agent(entry.id).map_err(CarrierError::from)?;
        }
        if workspace_dir.exists() {
            for dir_entry in std::fs::read_dir(&workspace_dir).map_err(|e| {
                CarrierError::Internal(format!("Failed to read workspace for reinstall: {e}"))
            })? {
                let dir_entry = dir_entry
                    .map_err(|e| CarrierError::Internal(format!("Dir entry error: {e}")))?;
                let path = dir_entry.path();
                // Preserve .dup/ (clone version history for the dup sync workflow).
                if dir_entry.file_name().to_string_lossy() == ".dup" {
                    continue;
                }
                let _ = if path.is_dir() {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
            }
            tracing::info!(name = %clone_name, "Existing workspace cleared for reinstall (.dup/ preserved)");
        }

        // Ensure template.json has a `version` field - DupHub requires it to
        // extract listing metadata (description/display_name/category). Agents
        // sometimes omit it; add "1" if missing (never overwrite - defeats dup
        // debounce). Mutates the in-memory files map before writing to disk.
        let mut files = files;
        if carrier_clone::manifest::ensure_template_version(&mut files) {
            tracing::info!(name = %clone_name, "Added missing `version` field to template.json (DupHub listing metadata requires it)");
        }

        // Install-time hard format gate (docs/CLONE-FORMAT.md enforcement):
        // reject the two layouts the runtime silently mis-parses — top-level
        // `skills/` (invisible to scan_flows) and flow files without a
        // non-empty description (flow never injected, tools dead). The
        // structured error tells the caller (usually the clone-creator agent)
        // exactly what to fix, so it self-repairs instead of retrying blind.
        let format_errors = carrier_clone::validate_install_format(&files)
            .map_err(|e| CarrierError::Internal(format!("format validation: {e}")))?;
        if !format_errors.is_empty() {
            return Err(CarrierError::Internal(format!(
                "分身格式校验未通过（共 {} 项，修复后重新提交）：\n- {}",
                format_errors.len(),
                format_errors.join("\n- ")
            )));
        }

        // File-level write of the fetched definition files.
        let security_warnings = write_files_to_workspace(&files, &workspace_dir).map_err(|e| {
            let _ = std::fs::remove_dir_all(&workspace_dir);
            CarrierError::Internal(format!("Failed to write files: {e}"))
        })?;

        // Seed the default `self-growth` flow (factory-baked autonomous
        // learn/create capability) unless the clone ships its own. Done before
        // building the manifest so the flow is auto-registered in the flows
        // list. Whether it actually runs is controlled per-clone by
        // EVOLUTION.md `self_growth_enabled` (reconciled by the daemon).
        let self_growth_flow = workspace_dir.join("flows/self-growth/flow.md");
        if !self_growth_flow.exists() {
            if let Some(parent) = self_growth_flow.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) =
                std::fs::write(&self_growth_flow, carrier_clone::DEFAULT_SELF_GROWTH_FLOW)
            {
                tracing::warn!(name = %clone_name, error = %e, "failed to seed default self-growth flow");
            }
        }

        // Seed the clone format spec (`knowledge/format-spec.md`) so the
        // clone-creator (and any agent) reads the CURRENT format rules rather
        // than a possibly-stale copy baked into its own definition layer.
        // Unlike self-growth, this file is system-owned: the daemon's reseeding
        // reconciler overwrites it when the binary's spec version changes.
        let spec_path = workspace_dir.join("knowledge/format-spec.md");
        if let Some(parent) = spec_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let stamped = format!(
            "<!-- clone-format-spec {} (system-seeded; do not edit) -->\n{}",
            carrier_clone::CLONE_FORMAT_SPEC_VERSION,
            carrier_clone::CLONE_FORMAT_SPEC
        );
        if let Err(e) = std::fs::write(&spec_path, stamped) {
            tracing::warn!(name = %clone_name, error = %e, "failed to seed format spec");
        }

        let mut manifest =
            build_manifest_from_workspace(&workspace_dir, &clone_name, Some(clone_name.clone()))
                .map_err(|e| {
                    let _ = std::fs::remove_dir_all(&workspace_dir);
                    CarrierError::Internal(format!("Failed to build manifest: {e}"))
                })?;
        manifest.workspace = Some(workspace_dir.clone());

        let toml_str = toml::to_string_pretty(&manifest)
            .map_err(|e| CarrierError::Internal(format!("Failed to serialize agent.toml: {e}")))?;
        std::fs::write(workspace_dir.join("agent.toml"), toml_str)
            .map_err(|e| CarrierError::Internal(format!("Failed to write agent.toml: {e}")))?;

        let agent_name = manifest.name.clone();
        let display_name = manifest.display_name.clone();
        let id = self
            .spawn_agent(manifest)
            .map_err(|e| CarrierError::Internal(format!("Spawn failed: {e}")))?;

        let template = std::fs::read_to_string(workspace_dir.join("template.json"))
            .ok()
            .and_then(|s| carrier_clone::parse_template_manifest_lenient(&s));
        let plugins = template
            .as_ref()
            .map(|t| t.plugins.clone())
            .unwrap_or_default();

        if !plugins.is_empty() {
            self.resolve_plugin_dependencies(&plugins).await;
        }

        // ── aginx 入网钩子 ──
        // 分身装好即入网：写 ~/.aginx/agents/<name>/aginx.toml，网关扫描即
        // 可见。失败不挡安装（aginx 网关可以不存在）。
        let (desc, ver) = match template.as_ref() {
            Some(t) => (t.description.clone(), t.version.clone()),
            None => (String::new(), String::new()),
        };
        if let Err(e) =
            crate::aginx_net::register_clone_default(&agent_name, &display_name, &desc, &ver)
        {
            tracing::warn!(name = %agent_name, error = %e, "aginx registration failed (clone still installed)");
        }

        tracing::info!(
            name = %agent_name,
            id = %id,
            warnings = security_warnings.len(),
            file_count = files.len(),
            plugins = ?plugins,
            "Clone installed (dup file-level flow)"
        );

        Ok((id.to_string(), agent_name, display_name))
    }
}

// ── MemorySubstrateHandle — wraps MemorySubstrate to implement MemoryHandle ──

/// Thin wrapper that implements `MemoryHandle` by delegating to `MemorySubstrate`.
/// Needed because MemorySubstrate can't depend on the runtime crate's trait.
pub struct MemorySubstrateHandle {
    inner: Arc<MemorySubstrate>,
}

impl MemorySubstrateHandle {
    pub fn new(inner: Arc<MemorySubstrate>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl MemoryHandle for MemorySubstrateHandle {
    fn kv_set(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> CarrierResult<()> {
        self.inner
            .system_kv_set(agent_id, owner_id, user_id, key, value)
    }

    fn kv_get(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<Option<serde_json::Value>> {
        self.inner.system_kv_get(agent_id, owner_id, user_id, key)
    }

    fn kv_list(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
    ) -> CarrierResult<Vec<(String, serde_json::Value)>> {
        self.inner.list_kv(agent_id, owner_id, user_id)
    }

    fn kv_delete(
        &self,
        agent_id: &str,
        owner_id: &str,
        user_id: &str,
        key: &str,
    ) -> CarrierResult<()> {
        self.inner
            .system_kv_delete(agent_id, owner_id, user_id, key)
    }

    async fn tree_ingest(
        &self,
        req: carrier_types::memory_tree::IngestRequest,
    ) -> CarrierResult<carrier_types::memory_tree::IngestResult> {
        self.inner.tree_ingest_async(req).await
    }

    async fn tree_query_source(
        &self,
        req: carrier_types::memory_tree::SourceQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse> {
        self.inner.tree_query_source_async(req).await
    }

    async fn tree_query_global(
        &self,
        req: carrier_types::memory_tree::GlobalQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse> {
        self.inner.tree_query_global_async(req).await
    }

    async fn tree_query_topic(
        &self,
        req: carrier_types::memory_tree::TopicQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse> {
        self.inner.tree_query_topic_async(req).await
    }

    async fn tree_search_entities(
        &self,
        req: carrier_types::memory_tree::EntitySearch<'_>,
    ) -> CarrierResult<Vec<carrier_types::memory_tree::EntityMatch>> {
        self.inner.tree_search_entities_async(req).await
    }

    async fn tree_drill_down(
        &self,
        req: carrier_types::memory_tree::DrillDownQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse> {
        self.inner.tree_drill_down_async(req).await
    }

    async fn tree_fetch_leaves(
        &self,
        req: carrier_types::memory_tree::FetchLeavesQuery<'_>,
    ) -> CarrierResult<carrier_types::memory_tree::QueryResponse> {
        self.inner.tree_fetch_leaves_async(req).await
    }

    async fn tree_list_sources(
        &self,
        owner_id: &str,
        source_kind: Option<&str>,
        limit: usize,
    ) -> CarrierResult<Vec<carrier_types::memory_tree::TreeSummary>> {
        self.inner
            .tree_list_sources_async(owner_id, source_kind, limit)
            .await
    }

    fn analytics_user_stats(
        &self,
        agent_id: &str,
        active_days: u32,
    ) -> CarrierResult<serde_json::Value> {
        self.inner.analytics_user_stats(agent_id, active_days)
    }

    fn analytics_user_lookup(
        &self,
        agent_id: &str,
        sender_id: &str,
    ) -> CarrierResult<serde_json::Value> {
        self.inner.analytics_user_lookup(agent_id, sender_id)
    }

    fn analytics_usage(&self, agent_id: &str, days: u32) -> CarrierResult<serde_json::Value> {
        self.inner.analytics_usage(agent_id, days)
    }

    fn analytics_recent_conversations(
        &self,
        agent_id: &str,
        limit: u32,
    ) -> CarrierResult<serde_json::Value> {
        self.inner.analytics_recent_conversations(agent_id, limit)
    }
}

/// Build the memory handle for injection into agent_loop/tools/compaction.
///
/// Memory is LOCAL: in-process `MemorySubstrate` (sqlite, WAL) — the phone
/// and the server twin run the same substrate, no PG service in between
/// (M35, 2026-09-03: the `AGINXMEMORY_URL`/`HttpMemoryHandle` delegation
/// path was removed; roaming lands as substrate-level sync, M37).
pub fn make_memory_handle(
    memory: Arc<MemorySubstrate>,
) -> Arc<dyn carrier_runtime::memory_handle::MemoryHandle> {
    Arc::new(MemorySubstrateHandle::new(memory))
}

fn mime_from_image_url(url: &str) -> String {
    // Strip query string for extension detection.
    let path = url.split('?').next().unwrap_or(url);
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
    .to_string()
}

/// Resolve a relative one-shot schedule (`{"kind":"at","in_secs":N}`) into an
/// absolute `at` timestamp, server-side. Agents computing absolute times
/// themselves is a recurring timezone failure (08-19 白云调图事故: agent
/// computed `at` in the past twice and got "scheduled time must be in the
/// future" rejections), so relative delays are the recommended chaining
/// form. Absolute `at` passes through untouched.
fn resolve_relative_at(mut schedule: serde_json::Value) -> CarrierResult<serde_json::Value> {
    let is_at = schedule
        .get("kind")
        .and_then(|k| k.as_str())
        .is_some_and(|k| k == "at");
    if !is_at {
        return Ok(schedule);
    }
    let in_secs = match schedule.get("in_secs") {
        None | Some(serde_json::Value::Null) => return Ok(schedule),
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
            .ok_or_else(|| {
                CarrierError::InvalidInput(
                    "schedule.in_secs must be a positive integer (seconds from now)".into(),
                )
            })?,
    };
    if in_secs == 0 {
        return Err(CarrierError::InvalidInput(
            "schedule.in_secs must be > 0".into(),
        ));
    }
    // JSON null counts as absent - an LLM echoing the documented schema with
    // a null'd optional `at` while using `in_secs` must not be rejected.
    if matches!(schedule.get("at"), Some(v) if !v.is_null()) {
        return Err(CarrierError::InvalidInput(
            "schedule: pass EITHER 'at' (absolute RFC3339) OR 'in_secs' (relative seconds), not both"
                .into(),
        ));
    }
    let at = chrono::Utc::now() + chrono::Duration::seconds(in_secs as i64);
    let obj = schedule
        .as_object_mut()
        .expect("kind check above implies object");
    obj.insert("at".to_string(), serde_json::Value::String(at.to_rfc3339()));
    obj.remove("in_secs");
    Ok(schedule)
}

/// Chain identity validation for cron_create: chain_id is interpolated
/// verbatim into the system prompt's output-dir template
/// (output/{chain_id}/), so it must be a single path-safe
/// segment - the same guarantee task_id gets from slugify. And the
/// "pipeline:<id>" session_label convention must carry the SAME id: the
/// prompt steers file output by chain_id while the turn runs in the
/// session_label session; a mismatch splits pipeline state across two ids
/// (08-19 坑4 in a new shape).
fn validate_chain_identity(
    c: &carrier_types::scheduler::ChainMeta,
    action: &carrier_types::scheduler::CronAction,
) -> CarrierResult<()> {
    if !c
        .chain_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
        || c.chain_id == ".."
    {
        return Err(CarrierError::InvalidInput(format!(
            "chain.chain_id must be path-safe (letters, digits, '-', '_', '.'; no '/'、'..'、空格): got {:?} - \
             it becomes the output directory output/<chain_id>/",
            c.chain_id
        )));
    }
    if let carrier_types::scheduler::CronAction::AgentTurn {
        session_label: Some(label),
        ..
    } = action
    {
        if let Some(label_id) = label.strip_prefix("pipeline:") {
            if label_id != c.chain_id {
                return Err(CarrierError::InvalidInput(format!(
                    "chain.chain_id ({}) and action.session_label ({label}) name different pipelines - \
                     they must use the SAME id (session_label = \"pipeline:<chain_id>\")",
                    c.chain_id
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::resolve_relative_at;
    use carrier_types::scheduler::CronAction;

    #[test]
    fn relative_at_resolves_to_future_rfc3339() {
        let before = chrono::Utc::now();
        let out = resolve_relative_at(serde_json::json!({
            "kind": "at", "in_secs": 120
        }))
        .expect("relative at resolves");
        let at = out["at"].as_str().expect("at injected");
        let parsed = chrono::DateTime::parse_from_rfc3339(at).expect("rfc3339");
        let delta = parsed.signed_duration_since(before).num_seconds();
        assert!(
            (115..=125).contains(&delta),
            "at should be ~now+120s, got delta {delta}s"
        );
        assert!(out.get("in_secs").is_none(), "in_secs stripped");
        assert_eq!(out["kind"], "at");
    }

    #[test]
    fn relative_at_accepts_stringified_secs() {
        let out = resolve_relative_at(serde_json::json!({
            "kind": "at", "in_secs": "60"
        }))
        .expect("string in_secs accepted");
        assert!(out["at"].is_string());
    }

    #[test]
    fn absolute_at_passthrough() {
        let sched = serde_json::json!({
            "kind": "at", "at": "2030-01-01T00:00:00Z"
        });
        let out = resolve_relative_at(sched.clone()).expect("passthrough");
        assert_eq!(out, sched);
    }

    #[test]
    fn non_at_kinds_untouched() {
        let sched = serde_json::json!({"kind": "every", "every_secs": 300});
        let out = resolve_relative_at(sched.clone()).expect("passthrough");
        assert_eq!(out, sched);
    }

    #[test]
    fn both_at_and_in_secs_rejected() {
        let err = resolve_relative_at(serde_json::json!({
            "kind": "at", "at": "2030-01-01T00:00:00Z", "in_secs": 60
        }))
        .expect_err("ambiguous schedule rejected");
        assert!(err.to_string().contains("EITHER"));
    }

    #[test]
    fn zero_and_invalid_in_secs_rejected() {
        assert!(resolve_relative_at(serde_json::json!({"kind": "at", "in_secs": 0})).is_err());
        assert!(resolve_relative_at(serde_json::json!({"kind": "at", "in_secs": "soon"})).is_err());
    }

    /// JSON null `at` counts as absent - an LLM echoing the documented schema
    /// with a null'd optional must not hit the EITHER/OR rejection.
    #[test]
    fn null_at_with_in_secs_accepted() {
        let out = resolve_relative_at(serde_json::json!({
            "kind": "at", "at": null, "in_secs": 120
        }))
        .expect("null at treated as absent");
        assert!(out["at"].is_string(), "at injected from in_secs");
        assert!(out.get("in_secs").is_none());
    }

    fn chain(chain_id: &str) -> carrier_types::scheduler::ChainMeta {
        carrier_types::scheduler::ChainMeta {
            chain_id: chain_id.to_string(),
            step: 1,
            total_steps: 3,
        }
    }

    fn agent_turn(label: Option<&str>) -> CronAction {
        CronAction::AgentTurn {
            message: "m".into(),
            timeout_secs: None,
            active_flow: None,
            session_label: label.map(str::to_string),
            model_override: None,
        }
    }

    /// chain_id becomes output/<chain_id>/ in the prompt - it must be a
    /// single path-safe segment.
    #[test]
    fn chain_id_must_be_path_safe() {
        use super::validate_chain_identity;
        let action = agent_turn(None);
        for ok in ["pipeline-20260820-baiyun", "self_growth_v2", "a.b"] {
            assert!(
                validate_chain_identity(&chain(ok), &action).is_ok(),
                "{ok} should be accepted"
            );
        }
        for bad in [
            "pipeline/2026-08-20",
            "..",
            "a/../b",
            "pipeline x",
            "流水线",
        ] {
            assert!(
                validate_chain_identity(&chain(bad), &action).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    /// "pipeline:<id>" session_label must carry the same id as chain_id.
    #[test]
    fn chain_id_and_pipeline_session_label_must_agree() {
        use super::validate_chain_identity;
        let c = chain("pipeline-x");
        assert!(validate_chain_identity(&c, &agent_turn(Some("pipeline:pipeline-x"))).is_ok());
        assert!(validate_chain_identity(&c, &agent_turn(Some("pipeline-x"))).is_ok());
        assert!(validate_chain_identity(&c, &agent_turn(None)).is_ok());
        assert!(
            validate_chain_identity(&c, &agent_turn(Some("pipeline:other-id"))).is_err(),
            "mismatched pipeline: label must be rejected"
        );
    }
}
