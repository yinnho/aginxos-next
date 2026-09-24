//! Automation-rule tools (admin-gated): list/upsert/delete per-app
//! "trigger -> fixed action" rules stored in `automation_rules`. Matched on
//! inbound at two gates, both WITHOUT routing to the agent LLM: the
//! weixin-oa webhook (subscribe/keyword/menu_click/scan) and the plugin
//! bridge's weixin iLink gate (keyword only — iLink has no event surface).
//!
//! Admin gate uses the 86bus `wechat_identity` role (`"admin"`), distinct from
//! the clone-admin (`is_admin_gated`) concept - do not mix them.

use std::sync::Arc;

use super::ToolModule;
use crate::kernel_handle::KernelHandle;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use carrier_types::automation::{AutomationRule, TaskKind, TriggerKind};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};

pub struct AutomationRulesTools;

/// 86bus admin gate. Only callers whose `wechat_identity` role is `"admin"`
/// may manage automation rules.
fn require_admin(sender_id: Option<&str>) -> CarrierResult<()> {
    let sid = sender_id.ok_or_else(|| {
        CarrierError::CapabilityDenied("automation_rule: no sender_id in context".into())
    })?;
    if crate::wechat_identity::get(sid).as_deref() != Some("admin") {
        return Err(CarrierError::CapabilityDenied(
            "automation_rule tools require 86bus admin role".into(),
        ));
    }
    Ok(())
}

#[async_trait]
impl ToolModule for AutomationRulesTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "automation_rule_list".to_string(),
                description: "List automation rules for a channel scope (admin only): weixin-oa app_id (default channel), or weixin (iLink) agent name with channel='weixin'. Rules fire fixed replies on matching inbound without invoking the agent.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "app_id": { "type": "string", "description": "Service-account app_id (bot_id)" },
                        "channel": { "type": "string", "description": "Channel name (default 'weixin-oa')" }
                    },
                    "required": ["app_id"]
                }),
            },
            ToolDefinition {
                name: "automation_rule_upsert".to_string(),
                description: "Create or update an automation rule (admin only). On a matching inbound event: push_text/push_miniprogram deliver a fixed reply and skip the agent; notify_admin pushes to admins (notify_type→notify_routes) as a bypass while the agent still replies to the user. trigger: subscribe|keyword|menu_click|scan (menu_click matches the menu EventKey substring; scan matches the QR scene substring on SCAN and on qrscene_ subscribes). channel='weixin' (iLink): app_id is the agent name (iLink rules scope per agent), trigger must be keyword, task push_text/push only (iLink has no events and no miniprogram cards).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "app_id": { "type": "string" },
                        "name": { "type": "string", "description": "Human-readable rule name" },
                        "trigger": { "type": "string", "enum": ["subscribe", "keyword", "menu_click", "scan"] },
                        "keyword": { "type": "string", "description": "Required when trigger=keyword (substring match on text); for menu_click/scan also accepted as the EventKey/scene substring" },
                        "key": { "type": "string", "description": "EventKey/scene substring for trigger=menu_click or trigger=scan" },
                        "task": { "type": "string", "enum": ["push_text", "push_miniprogram", "notify_admin"] },
                        "text": { "type": "string", "description": "Required when task=push_text" },
                        "miniprogram": { "type": "object", "description": "Required when task=push_miniprogram: {appid, pagepath, title, thumb_media_id}" },
                        "priority": { "type": "integer", "description": "Higher = evaluated first (default 0)" },
                        "enabled": { "type": "boolean", "description": "default true" },
                        "id": { "type": "string", "description": "Existing rule id to update (omit to create new)" }
                    },
                    "required": ["app_id", "name", "trigger", "task"]
                }),
            },
            ToolDefinition {
                name: "automation_rule_delete".to_string(),
                description: "Delete an automation rule by id (admin only).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "id": { "type": "string" } },
                    "required": ["id"]
                }),
            },
            ToolDefinition {
                name: "message_push".to_string(),
                description: "Immediately push a message to a specific user or all admins (admin only). Supports text and miniprogram card formats. target = user_id (e.g. wmVXjfCw... for wecom-kf, oOPNNv... for weixin-oa, xxx@im.wechat for iLink) or 'admins'. msgtype inferred from which content field you provide.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Recipient: user_id or 'admins'" },
                        "text": { "type": "string", "description": "Text content (msgtype=text)" },
                        "miniprogram": { "type": "object", "description": "Miniprogram card (msgtype=miniprogram): {appid, pagepath, title, thumb_media_id}", "properties": {
                            "appid": { "type": "string" },
                            "pagepath": { "type": "string", "description": "Must end with .html for wecom-kf" },
                            "title": { "type": "string" },
                            "thumb_media_id": { "type": "string" }
                        } },
                        "bot_id": { "type": "string", "description": "Source bot_id for OA routing (optional, auto-inferred from user_id)" }
                    },
                    "required": ["target"]
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
        let kernel = ctx.kernel;
        let sender_id = ctx.sender_id;
        match name {
            "automation_rule_list" => Some(tool_rule_list(input, kernel, sender_id).await),
            "automation_rule_upsert" => Some(tool_rule_upsert(input, kernel, sender_id).await),
            "automation_rule_delete" => Some(tool_rule_delete(input, kernel, sender_id).await),
            "message_push" => {
                let agent_id = ctx.caller_agent_id;
                Some(tool_message_push(input, kernel, sender_id, agent_id).await)
            }
            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "automation_rule_list"
            | "automation_rule_upsert"
            | "automation_rule_delete"
            | "message_push" => PermissionLevel::Write,
            _ => PermissionLevel::Dangerous,
        }
    }
}

async fn tool_rule_list(
    input: &Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    sender_id: Option<&str>,
) -> CarrierResult<String> {
    require_admin(sender_id)?;
    let kh = crate::tools::require_kernel(kernel)?;
    let app_id = input["app_id"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'app_id'".to_string()))?;
    let channel = input["channel"].as_str().unwrap_or("weixin-oa").to_string();
    let rules = kh.automation_rule_list(&channel, app_id).await?;
    serde_json::to_string_pretty(&rules).map_err(|e| CarrierError::Serialization(e.to_string()))
}

async fn tool_rule_upsert(
    input: &Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    sender_id: Option<&str>,
) -> CarrierResult<String> {
    require_admin(sender_id)?;
    let kh = crate::tools::require_kernel(kernel)?;

    let app_id = input["app_id"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'app_id'".to_string()))?
        .to_string();
    let name = input["name"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'name'".to_string()))?
        .to_string();
    let channel = input["channel"].as_str().unwrap_or("weixin-oa").to_string();
    let trigger = input["trigger"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'trigger'".to_string()))?;
    let task = input["task"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'task'".to_string()))?;

    let trigger_kind = match trigger {
        "subscribe" => TriggerKind::Subscribe,
        "keyword" => TriggerKind::Keyword,
        "menu_click" => TriggerKind::MenuClick,
        "scan" => TriggerKind::Scan,
        other => {
            return Err(CarrierError::InvalidInput(format!(
                "unknown trigger '{other}' (subscribe|keyword|menu_click|scan)"
            )))
        }
    };
    let task_kind = match task {
        "push_text" => TaskKind::PushText,
        "push_miniprogram" => TaskKind::PushMiniprogram,
        "notify_admin" => TaskKind::NotifyAdmin,
        "push" => TaskKind::Push,
        other => {
            return Err(CarrierError::InvalidInput(format!(
                "unknown task '{other}' (push_text|push_miniprogram|notify_admin|push)"
            )))
        }
    };

    validate_weixin_scope(&channel, trigger_kind, task_kind)?;

    let trigger_data = match trigger_kind {
        TriggerKind::Keyword => input["keyword"]
            .as_str()
            .ok_or_else(|| {
                CarrierError::InvalidInput("trigger=keyword requires 'keyword'".to_string())
            })?
            .to_string(),
        // Menu click: EventKey substring. Accept "key" or the shared "keyword"
        // habit; an empty key would match every click, which is almost never
        // intended, so it is required.
        TriggerKind::MenuClick => input["key"]
            .as_str()
            .or_else(|| input["keyword"].as_str())
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                CarrierError::InvalidInput(
                    "trigger=menu_click requires 'key' (menu EventKey substring)".to_string(),
                )
            })?
            .to_string(),
        // QR scan: scene substring, matched against SCAN EventKey and against
        // qrscene_* on subscribe (new follow via QR).
        TriggerKind::Scan => input["key"]
            .as_str()
            .or_else(|| input["keyword"].as_str())
            .filter(|k| !k.trim().is_empty())
            .ok_or_else(|| {
                CarrierError::InvalidInput(
                    "trigger=scan requires 'key' (QR scene substring)".to_string(),
                )
            })?
            .to_string(),
        TriggerKind::Subscribe => String::new(),
    };

    let task_payload = match task_kind {
        TaskKind::PushText => {
            let text = input["text"].as_str().ok_or_else(|| {
                CarrierError::InvalidInput("task=push_text requires 'text'".to_string())
            })?;
            serde_json::json!({ "text": text })
        }
        TaskKind::PushMiniprogram => {
            let mp = input.get("miniprogram").ok_or_else(|| {
                CarrierError::InvalidInput(
                    "task=push_miniprogram requires 'miniprogram' {appid,pagepath,title,thumb_media_id}"
                        .to_string(),
                )
            })?;
            let appid = mp["appid"].as_str().ok_or_else(|| {
                CarrierError::InvalidInput("miniprogram.appid required".to_string())
            })?;
            let pagepath = mp["pagepath"].as_str().ok_or_else(|| {
                CarrierError::InvalidInput("miniprogram.pagepath required".to_string())
            })?;
            let title = mp["title"].as_str().ok_or_else(|| {
                CarrierError::InvalidInput("miniprogram.title required".to_string())
            })?;
            let thumb_media_id = mp["thumb_media_id"].as_str().ok_or_else(|| {
                CarrierError::InvalidInput("miniprogram.thumb_media_id required".to_string())
            })?;
            serde_json::json!({
                "miniprogram": {
                    "appid": appid,
                    "pagepath": pagepath,
                    "title": title,
                    "thumb_media_id": thumb_media_id
                }
            })
        }
        TaskKind::NotifyAdmin => {
            let notify_type = input["notify_type"].as_str().ok_or_else(|| {
                CarrierError::InvalidInput(
                    "task=notify_admin requires 'notify_type' (matches a notify_routes entry)"
                        .to_string(),
                )
            })?;
            serde_json::json!({ "notify_type": notify_type })
        }
        TaskKind::Push => {
            // Unified: build a ContentDescriptor (the same path as message_push)
            // and serialize it. Sharing build_content_descriptor keeps the stored
            // payload contract identical to the direct-push path — e.g. it
            // preserves `thumb_url`, which wecom-kf delivery requires (an OA
            // thumb_media_id alone is invalid in wecom's media library), and
            // avoids the divergent validation the hand-written parser had.
            let content = build_content_descriptor(input)?;
            serde_json::to_value(content).map_err(|e| CarrierError::Serialization(e.to_string()))?
        }
    };

    let priority = input["priority"].as_i64().unwrap_or(0);
    let enabled = input["enabled"].as_bool().unwrap_or(true);
    let id = input["id"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let now = chrono::Utc::now().to_rfc3339();

    let rule = AutomationRule {
        id: id.clone(),
        app_id,
        channel,
        name,
        enabled,
        priority,
        trigger_kind,
        trigger_data,
        task_kind,
        task_payload,
        target: input["target"].as_str().unwrap_or("current").to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    kh.automation_rule_upsert(rule).await?;
    Ok(format!("Automation rule saved: {id}"))
}

async fn tool_rule_delete(
    input: &Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    sender_id: Option<&str>,
) -> CarrierResult<String> {
    require_admin(sender_id)?;
    let kh = crate::tools::require_kernel(kernel)?;
    let id = input["id"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'id'".to_string()))?;
    kh.automation_rule_delete(id).await?;
    Ok(format!("Automation rule deleted: {id}"))
}

/// iLink (channel `"weixin"`) scope constraints: iLink has no event surface
/// (only text messages arrive), cannot deliver miniprogram cards, and the
/// bridge gate implements keyword → push_text/push only. Reject at write time
/// so misconfigured rules never silently no-op in production.
fn validate_weixin_scope(channel: &str, trigger: TriggerKind, task: TaskKind) -> CarrierResult<()> {
    if channel != "weixin" {
        return Ok(());
    }
    if trigger != TriggerKind::Keyword {
        return Err(CarrierError::InvalidInput(
            "channel 'weixin' (iLink) only supports trigger=keyword — iLink has no subscribe/menu/scan events".into(),
        ));
    }
    if !matches!(task, TaskKind::PushText | TaskKind::Push) {
        return Err(CarrierError::InvalidInput(
            "channel 'weixin' (iLink) only supports task=push_text/push — no miniprogram cards, no notify_admin on iLink".into(),
        ));
    }
    Ok(())
}

/// Build a `ContentDescriptor` from tool input fields (text or miniprogram).
fn build_content_descriptor(input: &Value) -> CarrierResult<carrier_types::content::ContentDescriptor> {
    use carrier_types::content::{ContentDescriptor, MiniprogramContent};

    if let Some(text) = input["text"].as_str() {
        return Ok(ContentDescriptor {
            text: Some(text.to_string()),
            ..Default::default()
        });
    }
    if let Some(mp) = input.get("miniprogram") {
        let appid = mp["appid"]
            .as_str()
            .ok_or_else(|| CarrierError::InvalidInput("miniprogram.appid required".to_string()))?;
        let pagepath = mp["pagepath"].as_str().ok_or_else(|| {
            CarrierError::InvalidInput("miniprogram.pagepath required".to_string())
        })?;
        let title = mp["title"]
            .as_str()
            .ok_or_else(|| CarrierError::InvalidInput("miniprogram.title required".to_string()))?;
        let thumb_media_id = mp["thumb_media_id"].as_str().map(String::from);
        let thumb_url = mp["thumb_url"].as_str().map(String::from);
        return Ok(ContentDescriptor {
            miniprogram: Some(MiniprogramContent {
                appid: appid.to_string(),
                pagepath: pagepath.to_string(),
                title: title.to_string(),
                thumb_media_id,
                thumb_url,
                thumb_file: None,
            }),
            ..Default::default()
        });
    }
    Err(CarrierError::InvalidInput(
        "requires 'text' or 'miniprogram' content".to_string(),
    ))
}

/// Immediately push a message to a specific user or all admins (admin only).
async fn tool_message_push(
    input: &Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    sender_id: Option<&str>,
    caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    require_admin(sender_id)?;
    let kh = crate::tools::require_kernel(kernel)?;
    let target = input["target"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'target'".to_string()))?;
    let content = build_content_descriptor(input)?;
    let agent_id = caller_agent_id.unwrap_or("");
    let bot_id = input["bot_id"].as_str().unwrap_or("");
    kh.push_message(
        target.to_string(),
        content,
        agent_id.to_string(),
        bot_id.to_string(),
    )
    .await?;
    Ok(format!("Message pushed to {target}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_admin_gate() {
        // 86bus role gate: only role=="admin" passes. Distinct from clone-admin.
        crate::wechat_identity::set("sid_nonadmin", "carrier_user");
        assert!(require_admin(Some("sid_nonadmin")).is_err());
        crate::wechat_identity::set("sid_admin", "admin");
        assert!(require_admin(Some("sid_admin")).is_ok());
        assert!(require_admin(None).is_err()); // no sender_id
        crate::wechat_identity::set("sid_empty", "");
        assert!(require_admin(Some("sid_empty")).is_err()); // empty role != admin
    }

    #[test]
    fn weixin_scope_validation() {
        // Allowed: keyword + push_text / push.
        assert!(validate_weixin_scope("weixin", TriggerKind::Keyword, TaskKind::PushText).is_ok());
        assert!(validate_weixin_scope("weixin", TriggerKind::Keyword, TaskKind::Push).is_ok());
        // Rejected triggers: iLink has no event surface.
        assert!(
            validate_weixin_scope("weixin", TriggerKind::Subscribe, TaskKind::PushText).is_err()
        );
        assert!(
            validate_weixin_scope("weixin", TriggerKind::MenuClick, TaskKind::PushText).is_err()
        );
        assert!(validate_weixin_scope("weixin", TriggerKind::Scan, TaskKind::PushText).is_err());
        // Rejected tasks: no miniprogram cards, no notify_admin on iLink.
        assert!(
            validate_weixin_scope("weixin", TriggerKind::Keyword, TaskKind::PushMiniprogram)
                .is_err()
        );
        assert!(
            validate_weixin_scope("weixin", TriggerKind::Keyword, TaskKind::NotifyAdmin).is_err()
        );
        // Other channels unrestricted.
        assert!(
            validate_weixin_scope("weixin-oa", TriggerKind::Subscribe, TaskKind::NotifyAdmin)
                .is_ok()
        );
        assert!(
            validate_weixin_scope("wecom", TriggerKind::Scan, TaskKind::PushMiniprogram).is_ok()
        );
    }
}
