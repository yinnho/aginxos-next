//! Gateway hub tools: contacts_list, contact_prompt.
//!
//! The system agent「me」uses these to see and reach EVERY contact — local
//! clones (via kernel) and remote agent:// contacts on other people's gateways
//! (via carrier-gateway's ACP client). This is the agent-side twin of what the
//! webui does over HTTP: same endpoints, same credentials (the webui ledger's
//! owner/visitor slots), same `@target~agent` contact-id grammar.
//!
//! Deliberately NOT in CORE_TOOL_NAMES and deliberately Write-level: only a
//! clone whose flow declares these tools gets them, so ordinary clones see no
//! new tool surface at all.

use super::ToolModule;
use crate::kernel_handle::KernelHandle;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use carrier_gateway::agent_client::{AgentConn, AgentEndpoint};
use carrier_gateway::tool_store::ToolStore;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};
use serde_json::Value;
use std::sync::Arc;

/// kv key prefix for per-contact sessionId bookkeeping (auto-resume).
/// Keyed under the calling agent + owner so each owner's "me" keeps its own
/// continuation map; user_id is left empty (contact continuity is per-owner,
/// not per-chat-user — remote sessions belong to the owner identity).
const SESSION_KEY_PREFIX: &str = "gw_session:";

/// Parse a contact id into its local / remote halves.
/// Local ids never start with `@` and must not contain `~`; remote ids are
/// exactly `@<target>~<agent>`.
fn split_contact(id: &str) -> Result<ContactRef, String> {
    if let Some(rest) = id.strip_prefix('@') {
        let (target, agent) = rest
            .split_once('~')
            .ok_or_else(|| format!("远程联系人格式应为 @target~agent，得到 '{id}'"))?;
        if target.is_empty() || agent.is_empty() {
            return Err(format!("远程联系人 target/agent 不能为空：'{id}'"));
        }
        Ok(ContactRef::Remote {
            target: target.to_string(),
            agent: agent.to_string(),
        })
    } else if id.contains('~') {
        Err(format!("联系人 id 含 '~' 但缺 '@' 前缀：'{id}'"))
    } else if id.is_empty() {
        Err("联系人 id 为空".to_string())
    } else {
        Ok(ContactRef::Local(id.to_string()))
    }
}

enum ContactRef {
    Local(String),
    Remote { target: String, agent: String },
}

/// Load the shared webui ledger. Same file the HTTP surface uses, so contacts
/// added/bound from the webui are instantly visible here and vice versa.
fn shared_store(home_dir: Option<&std::path::Path>) -> CarrierResult<ToolStore> {
    let base = home_dir
        .map(|p| p.to_path_buf())
        .unwrap_or_else(carrier_types::config::home_dir);
    Ok(ToolStore::load(base.join("webui/external-tools.json")))
}

async fn handle_contacts_list(
    _input: &Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    ctx: &ToolContext<'_>,
) -> CarrierResult<String> {
    let mut out: Vec<Value> = Vec::new();

    // Local clones — registry view (id is the clone name for prompt purposes).
    if let Some(kh) = kernel {
        for a in kh.list_agents() {
            out.push(serde_json::json!({
                "id": a.name,
                "name": a.display_name,
                "description": a.description,
                "kind": "local",
                "state": a.state,
            }));
        }
    }

    // Remote contacts — webui ledger (added remote agents with metadata).
    let store = shared_store(ctx.home_dir)?;
    for (id, meta) in store.added_remote() {
        out.push(serde_json::json!({
            "id": id,
            "name": meta.name,
            "description": meta.description,
            "kind": "remote",
            "gateway": meta.gateway,
        }));
    }

    // Bare-id gateway agents (e.g. local CLI tools like claude) added from the
    // webui: reachable via the same agent:// path, just on our own relay.
    for id in store.added() {
        if !id.starts_with('@') {
            out.push(serde_json::json!({
                "id": id,
                "name": id,
                "description": "网关 agent（本机 relay）",
                "kind": "remote",
                "gateway": "",
            }));
        }
    }

    if out.is_empty() {
        return Ok("当前没有任何联系人（本地分身与远程联系人均为空）。".to_string());
    }
    Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string()))
}

async fn handle_contact_prompt(
    input: &Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    ctx: &ToolContext<'_>,
) -> CarrierResult<String> {
    let contact = input["contact"]
        .as_str()
        .ok_or(CarrierError::InvalidInput("Missing 'contact' parameter".into()))?;
    let message = input["message"]
        .as_str()
        .ok_or(CarrierError::InvalidInput("Missing 'message' parameter".into()))?;

    match split_contact(contact).map_err(CarrierError::InvalidInput)? {
        ContactRef::Local(name) => {
            let kh = crate::tools::require_kernel(kernel)?;
            // Bare id: registry clone first; fall back to the gateway ledger
            // (webui-added CLI tools like claude ride our own relay).
            let is_local = kh.list_agents().iter().any(|a| a.name == name);
            if is_local {
                return prompt_local(kh, &name, message, ctx).await;
            }
            if shared_store(ctx.home_dir)?.is_added(&name) {
                return prompt_remote(ctx, &name, None, message, input).await;
            }
            Err(CarrierError::AgentNotFound(format!(
                "联系人 '{name}' 既不是本地分身也不在通讯录里"
            )))
        }
        ContactRef::Remote { target, agent } => {
            let store = shared_store(ctx.home_dir)?;
            let url = store.gateway_url(&target).ok_or_else(|| {
                CarrierError::InvalidInput(format!("远程网关 {target} 不在地址簿（先在 App 里扫码添加）"))
            })?;
            prompt_remote(ctx, &agent, Some((url.to_string(), target)), message, input).await
        }
    }
}

/// Local fork: inter-agent send with call-depth guard.
async fn prompt_local(
    kh: &Arc<dyn KernelHandle>,
    name: &str,
    message: &str,
    ctx: &ToolContext<'_>,
) -> CarrierResult<String> {
    crate::tools::check_call_depth()?;
    let current_depth = crate::tool_runner::AGENT_CALL_DEPTH.try_with(|d| d.get()).unwrap_or(0);
    crate::tool_runner::AGENT_CALL_DEPTH
        .scope(std::cell::Cell::new(current_depth + 1), async {
            kh.send_to_agent(name, message, ctx.sender_id, None, ctx.caller_agent_id, ctx.owner_id, None)
                .await
        })
        .await
}

/// Remote fork: agent:// prompt over the relay. `explicit_target` carries the
/// address-book (url, target) for `@target~agent` ids; None = our own relay
/// config (bare-id gateway agents).
#[allow(clippy::too_many_arguments)]
async fn prompt_remote(
    ctx: &ToolContext<'_>,
    agent: &str,
    explicit_target: Option<(String, String)>,
    message: &str,
    input: &Value,
) -> CarrierResult<String> {
    let mut ep = match &explicit_target {
        Some((url, _)) => AgentEndpoint::from_url_with_local_secret(url)
            .ok_or_else(|| CarrierError::InvalidInput(format!("网关地址无效: {url}")))?,
        None => AgentEndpoint::from_gateway_config().ok_or_else(|| {
            CarrierError::Internal("本机网关未配置（~/.aginx/config.toml [relay] 段缺失）".to_string())
        })?,
    };
    if let Some((_, target)) = &explicit_target {
        ep.auth_token = shared_store(ctx.home_dir)?.gateway_token(target);
    }

    // Auto-resume: deterministic bookkeeping, not LLM-visible state.
    let contact_label = explicit_target
        .as_ref()
        .map(|(_, t)| format!("@{}~{}", t, agent))
        .unwrap_or_else(|| agent.to_string());
    let explicit = input
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let session_id = match explicit {
        Some(v) if v.is_empty() => None,
        Some(v) => Some(v),
        None => {
            let agent_id = ctx.caller_agent_id.unwrap_or("");
            let owner = ctx.owner_id.unwrap_or("");
            match ctx.memory {
                Some(mem) => mem
                    .kv_get(agent_id, owner, "", &format!("{SESSION_KEY_PREFIX}{contact_label}"))?
                    .and_then(|v| v.as_str().map(str::to_string)),
                None => None,
            }
        }
    };

    let mut conn = AgentConn::connect(&ep)
        .await
        .map_err(|e| CarrierError::Internal(format!("连接网关失败: {e}")))?;
    let ok = conn
        .initialize()
        .await
        .map_err(|e| CarrierError::Internal(format!("握手失败: {e}")))?;
    if !ok {
        return Err(CarrierError::CapabilityDenied(format!(
            "对方网关拒绝了本次访问（{contact_label} 未授权或凭证失效）——请在 App 里重新绑定或申请访问"
        )));
    }
    let result = conn
        .prompt(agent, message, session_id.as_deref(), |_| true)
        .await
        .map_err(|e| CarrierError::Internal(format!("对方响应失败: {e}")))?;

    // Persist harvested sessionId for next-turn continuation.
    if let Some(sid) = &result.session_id {
        if let Some(mem) = ctx.memory {
            let agent_id = ctx.caller_agent_id.unwrap_or("");
            let owner = ctx.owner_id.unwrap_or("");
            let _ = mem.kv_set(
                agent_id,
                owner,
                "",
                &format!("{SESSION_KEY_PREFIX}{contact_label}"),
                Value::String(sid.clone()),
            );
        }
    }

    let mut out = serde_json::json!({ "response": result.text });
    if let Some(sid) = &result.session_id {
        out["session_id"] = Value::String(sid.clone());
    }
    if let Some(cost) = result.cost_usd {
        out["cost_usd"] = serde_json::json!(cost);
    }
    Ok(out.to_string())
}

/// Hub tools for the system agent「me」— see module docs.
pub struct GatewayHubTools;

#[async_trait]
impl ToolModule for GatewayHubTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "contacts_list".to_string(),
                description: "列出全部联系人——本地分身（kind=local）与远程联系人（kind=remote，id 形如 @target~agent）。派活前先看一眼谁在。".to_string(),
                input_schema: serde_json::json!({"type": "object", "properties": {}}),
            },
            ToolDefinition {
                name: "contact_prompt".to_string(),
                description: "向一个联系人发消息并等结果。本地分身直接对话；远程联系人经 agent:// 网关。同一联系人的会话自动续接（无需传 session_id）；传空字符串 session_id 可强制开新会话。耗时可能较长（对方在做任务），请耐心等待。".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "required": ["contact", "message"],
                    "properties": {
                        "contact": {"type": "string", "description": "联系人 id（contacts_list 里拿到的）"},
                        "message": {"type": "string", "description": "要发给对方的任务或问题"},
                        "session_id": {"type": "string", "description": "可选；省略=自动续接上次会话，传\"\"=开新会话"}
                    }
                }),
            },
        ]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        match name {
            "contacts_list" => Some(handle_contacts_list(input, ctx.kernel, ctx).await),
            "contact_prompt" => Some(handle_contact_prompt(input, ctx.kernel, ctx).await),
            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            // Read-only listing — safe like agent_list.
            "contacts_list" => PermissionLevel::None,
            // Write-level by design: reachable under default max_tool_level,
            // no flow elevation required. It sends messages but creates nothing
            // irreversible; remote spend is gated by the owner's consent flow.
            "contact_prompt" => PermissionLevel::Write,
            _ => PermissionLevel::Dangerous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_local_and_remote_contacts() {
        assert!(matches!(
            split_contact("tiny-pilot"),
            Ok(ContactRef::Local(n)) if n == "tiny-pilot"
        ));
        match split_contact("@abc123~writer") {
            Ok(ContactRef::Remote { target, agent }) => {
                assert_eq!(target, "abc123");
                assert_eq!(agent, "writer");
            }
            _ => panic!("remote parse failed"),
        }
        // Bare-gateway agents on our own relay are still "local" grammar-wise:
        // no @ means the kernel registry resolves them (or fails visibly).
        assert!(matches!(
            split_contact("claude"),
            Ok(ContactRef::Local(_))
        ));
    }

    #[test]
    fn rejects_bad_contact_ids() {
        assert!(split_contact("").is_err());
        assert!(split_contact("~x").is_err()); // ~ without @
        assert!(split_contact("@abc").is_err()); // missing ~agent
        assert!(split_contact("@abc~").is_err()); // empty agent
        assert!(split_contact("@~writer").is_err()); // empty target
    }

    #[test]
    fn session_key_is_namespaced() {
        let k = format!("{SESSION_KEY_PREFIX}@t1~writer");
        assert_eq!(k, "gw_session:@t1~writer");
    }
}
