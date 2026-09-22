//! Delegation and user profile tool module.
//!
//! Handles `delegate_*` wildcard tools (subagent delegation) and the
//! `user_profile` tool. All other agent tools have been split into
//! domain-specific modules: agent_mgmt, training, scheduling,
//! collaboration, a2a.

use super::ToolModule;
use crate::kernel_handle::KernelHandle;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::ToolDefinition;

// ---------------------------------------------------------------------------
// User profile tool (multi-tenancy)
// ---------------------------------------------------------------------------

async fn tool_user_profile(
    input: &serde_json::Value,
    home_dir: Option<&Path>,
    agent_name: Option<&str>,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
) -> CarrierResult<String> {
    let sender = sender_id.ok_or(CarrierError::Internal(
        "user_profile requires a sender context (sender_id). This tool is only available when a user identity is provided.".to_string(),
    ))?;
    let hd = home_dir.ok_or(CarrierError::Internal(
        "user_profile requires home_dir".to_string(),
    ))?;
    let an = agent_name.ok_or(CarrierError::Internal(
        "user_profile requires agent_name".to_string(),
    ))?;
    let oid = crate::tools::sanitize_path_component(owner_id.unwrap_or(sender))?;
    let sender = crate::tools::sanitize_path_component(sender)?;

    let action = input["action"].as_str().unwrap_or("read");
    let profile_path =
        carrier_types::config::sender_data_dir(hd, oid, an, Some(sender)).join("profile.json");

    match action {
        "read" => {
            if profile_path.exists() {
                let content = tokio::fs::read_to_string(&profile_path)
                    .await
                    .map_err(|e| CarrierError::Internal(format!("Failed to read profile: {e}")))?;
                Ok(content)
            } else {
                // Return empty profile template
                let template = serde_json::json!({
                    "sender_id": sender,
                    "display_name": null,
                    "preferences": {},
                    "interaction_patterns": {},
                    "notes": null,
                    "conversation_count": 0,
                    "first_seen": null,
                    "last_seen": null,
                });
                Ok(serde_json::to_string_pretty(&template).unwrap_or_else(|_| "{}".to_string()))
            }
        }
        "update" => {
            // Load existing profile or create new
            let mut profile: serde_json::Value = if profile_path.exists() {
                let content = tokio::fs::read_to_string(&profile_path)
                    .await
                    .map_err(|e| CarrierError::Internal(format!("Failed to read profile: {e}")))?;
                serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
            } else {
                serde_json::json!({
                    "sender_id": sender,
                    "conversation_count": 0,
                    "first_seen": chrono::Utc::now().to_rfc3339(),
                })
            };

            // Ensure sender_id is set
            profile["sender_id"] = serde_json::Value::String(sender.to_string());
            profile["last_seen"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());

            // Merge updates
            if let Some(updates) = input.get("updates").and_then(|u| u.as_object()) {
                for (key, value) in updates {
                    match key.as_str() {
                        "display_name" | "notes" => {
                            profile[key] = value.clone();
                        }
                        "preferences" => apply_preferences_update(&mut profile, value),
                        "interaction_patterns" => {
                            apply_object_shallow_merge(&mut profile, "interaction_patterns", value)
                        }
                        _ => {}
                    }
                }
            }

            // Ensure directory exists
            if let Some(parent) = profile_path.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|e| {
                    CarrierError::Internal(format!("Failed to create user directory: {e}"))
                })?;
            }

            let output = serde_json::to_string_pretty(&profile).map_err(|e| {
                CarrierError::Serialization(format!("Failed to serialize profile: {e}"))
            })?;
            tokio::fs::write(&profile_path, &output)
                .await
                .map_err(|e| CarrierError::Internal(format!("Failed to write profile: {e}")))?;
            Ok(format!("Profile updated for user '{}'", sender))
        }
        "remove_account" => {
            // Delete one wechat_accounts entry by app_id. The update path can
            // only add/modify (merge by app_id never removes), so revoking a
            // stale account needs an explicit action.
            let app_id = input["app_id"]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    CarrierError::InvalidInput(
                        "remove_account requires a non-empty 'app_id'".to_string(),
                    )
                })?;
            if !profile_path.exists() {
                return Ok(format!(
                    "No profile for user '{sender}' — nothing to remove."
                ));
            }
            let mut profile: serde_json::Value = {
                let content = tokio::fs::read_to_string(&profile_path)
                    .await
                    .map_err(|e| CarrierError::Internal(format!("Failed to read profile: {e}")))?;
                serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
            };
            let accounts = profile
                .get_mut("preferences")
                .and_then(|p| p.get_mut("wechat_accounts"))
                .and_then(|a| a.as_array_mut());
            let Some(accounts) = accounts else {
                return Ok(format!(
                    "User '{sender}' has no wechat_accounts — nothing to remove."
                ));
            };
            let before = accounts.len();
            accounts.retain(|a| a.get("app_id").and_then(|v| v.as_str()) != Some(app_id));
            if accounts.len() == before {
                return Ok(format!(
                    "app_id '{app_id}' not found in user '{sender}' wechat_accounts."
                ));
            }
            let output = serde_json::to_string_pretty(&profile).map_err(|e| {
                CarrierError::Serialization(format!("Failed to serialize profile: {e}"))
            })?;
            tokio::fs::write(&profile_path, &output)
                .await
                .map_err(|e| CarrierError::Internal(format!("Failed to write profile: {e}")))?;
            Ok(format!(
                "Removed app_id '{app_id}' from user '{sender}' wechat_accounts."
            ))
        }
        other => Err(CarrierError::InvalidInput(format!(
            "Unknown action '{other}'. Use 'read' or 'update'."
        ))),
    }
}

/// Shallow-merge `updates.preferences` onto the existing object.
///
/// `wechat_accounts` is merged by `app_id` (same id updates fields, new id
/// appends, unmentioned accounts stay). An omitted or empty incoming array
/// does not wipe stored credentials — models routinely send only the account
/// they just heard.
fn apply_preferences_update(profile: &mut serde_json::Value, incoming: &serde_json::Value) {
    let Some(new_prefs) = incoming.as_object() else {
        return;
    };
    let mut prefs = profile
        .get("preferences")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    for (k, v) in new_prefs {
        if k == "wechat_accounts" {
            let existing = prefs
                .get("wechat_accounts")
                .and_then(|a| a.as_array())
                .cloned()
                .unwrap_or_default();
            let incoming_accts = v.as_array().cloned().unwrap_or_default();
            prefs.insert(
                "wechat_accounts".into(),
                serde_json::Value::Array(merge_wechat_accounts(&existing, &incoming_accts)),
            );
        } else {
            prefs.insert(k.clone(), v.clone());
        }
    }
    profile["preferences"] = serde_json::Value::Object(prefs);
}

/// Shallow-merge an object field: incoming keys overwrite, unmentioned stay.
fn apply_object_shallow_merge(
    profile: &mut serde_json::Value,
    field: &str,
    incoming: &serde_json::Value,
) {
    let Some(new_obj) = incoming.as_object() else {
        return;
    };
    let mut cur = profile
        .get(field)
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    for (k, v) in new_obj {
        cur.insert(k.clone(), v.clone());
    }
    profile[field] = serde_json::Value::Object(cur);
}

/// Merge incoming OA accounts into `existing` by `app_id`.
///
/// Items without `app_id` are skipped. Same `app_id` overwrites fields on the
/// existing object (so a new `app_secret` updates, other keys stay unless
/// resent). New ids are appended. Incoming empty → existing unchanged.
fn merge_wechat_accounts(
    existing: &[serde_json::Value],
    incoming: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let mut accounts = existing.to_vec();
    for inc in incoming {
        let Some(id) = inc.get("app_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(pos) = accounts
            .iter()
            .position(|a| a.get("app_id").and_then(|v| v.as_str()) == Some(id))
        {
            if let (Some(old), Some(new_fields)) = (accounts[pos].as_object_mut(), inc.as_object())
            {
                for (k, v) in new_fields {
                    old.insert(k.clone(), v.clone());
                }
            }
        } else {
            accounts.push(inc.clone());
        }
    }
    accounts
}

// ---------------------------------------------------------------------------
// Subagent delegation tools (delegate_{name})
// ---------------------------------------------------------------------------

async fn tool_delegate_subagent(
    subagent_name: &str,
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    caller_agent_id: Option<&str>,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let message = input["message"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'message' parameter".to_string(),
    ))?;
    let aid = caller_agent_id.ok_or(CarrierError::Internal(
        "delegate_* requires caller_agent_id".to_string(),
    ))?;

    // Check + increment inter-agent call depth
    crate::tools::check_call_depth()?;
    let current_depth = crate::tool_runner::AGENT_CALL_DEPTH
        .try_with(|d| d.get())
        .unwrap_or(0);

    tracing::info!(
        subagent = %subagent_name,
        depth = current_depth + 1,
        "Delegating to subagent"
    );

    // Route through kernel: send to self with channel_type hint for subagent
    // The kernel will see the same agent_id and apply subagent tool filtering
    let subagent_channel = format!("subagent:{}", subagent_name);

    crate::tool_runner::AGENT_CALL_DEPTH
        .scope(std::cell::Cell::new(current_depth + 1), async {
            kh.send_to_agent(
                aid,
                message,
                sender_id,
                None,
                caller_agent_id,
                owner_id,
                Some(&subagent_channel),
            )
            .await
        })
        .await
}

// ---------------------------------------------------------------------------
// ToolModule implementation
// ---------------------------------------------------------------------------

/// Delegation (delegate_*) and user_profile tools.
pub struct DelegationTools;

#[async_trait]
impl ToolModule for DelegationTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            // --- User profile tool (multi-tenancy) ---
            ToolDefinition {
                name: "user_profile".to_string(),
                description: "Read or update the current user's profile. The profile stores preferences, habits, and interaction patterns between this clone and a specific user. Requires a sender context (sender_id).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action": {"type": "string", "enum": ["read", "update", "remove_account"], "description": "Read the profile, update it with new key-value pairs, or remove one wechat_accounts entry"},
                        "updates": {"type": "object", "description": "Key-value pairs to merge into the profile (only for action=update). Supported keys: display_name, preferences (object; wechat_accounts merged by app_id, other keys shallow-merged), interaction_patterns (object), notes (string)"},
                        "app_id": {"type": "string", "description": "app_id of the wechat_accounts entry to delete (only for action=remove_account)"},
                    },
                    "required": ["action"],
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
        let caller_agent_id = ctx.caller_agent_id;
        let sender_id = ctx.sender_id;
        let owner_id = ctx.owner_id;

        match name {
            // User profile
            "user_profile" => Some(
                tool_user_profile(input, ctx.home_dir, ctx.agent_name, owner_id, sender_id).await,
            ),

            // Subagent delegation (delegate_{name})
            name if name.starts_with("delegate_") => {
                let subagent_name = &name["delegate_".len()..];
                Some(
                    tool_delegate_subagent(
                        subagent_name,
                        input,
                        kernel,
                        caller_agent_id,
                        owner_id,
                        sender_id,
                    )
                    .await,
                )
            }

            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> carrier_types::tool::PermissionLevel {
        match tool_name {
            "user_profile" => carrier_types::tool::PermissionLevel::None,
            name if name.starts_with("delegate_") => carrier_types::tool::PermissionLevel::Execute,
            _ => carrier_types::tool::PermissionLevel::Dangerous,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn acct(app_id: &str, secret: &str) -> serde_json::Value {
        json!({"app_id": app_id, "app_secret": secret})
    }

    #[test]
    fn merge_appends_new_app_id() {
        let existing = vec![acct("wxAAA", "old")];
        let incoming = vec![acct("wxBBB", "new")];
        let out = merge_wechat_accounts(&existing, &incoming);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["app_secret"], "old");
        assert_eq!(out[1]["app_id"], "wxBBB");
        assert_eq!(out[1]["app_secret"], "new");
    }

    #[test]
    fn merge_updates_secret_same_app_id() {
        let existing = vec![acct("wxAAA", "old")];
        let incoming = vec![acct("wxAAA", "rotated")];
        let out = merge_wechat_accounts(&existing, &incoming);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["app_secret"], "rotated");
    }

    #[test]
    fn merge_empty_incoming_keeps_existing() {
        let existing = vec![acct("wxAAA", "old")];
        let out = merge_wechat_accounts(&existing, &[]);
        assert_eq!(out, existing);
    }

    #[test]
    fn merge_skips_items_without_app_id() {
        let existing = vec![acct("wxAAA", "old")];
        let incoming = vec![json!({"app_secret": "orphan"}), acct("wxBBB", "b")];
        let out = merge_wechat_accounts(&existing, &incoming);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1]["app_id"], "wxBBB");
    }

    #[test]
    fn prefs_new_account_keeps_old_and_other_keys() {
        let mut profile = json!({
            "preferences": {
                "tone": "正式",
                "wechat_accounts": [{"app_id": "wxAAA", "app_secret": "AAA"}]
            }
        });
        apply_preferences_update(
            &mut profile,
            &json!({"wechat_accounts": [{"app_id": "wxBBB", "app_secret": "BBB"}]}),
        );
        let accts = profile["preferences"]["wechat_accounts"]
            .as_array()
            .unwrap();
        assert_eq!(accts.len(), 2);
        assert_eq!(accts[0]["app_secret"], "AAA");
        assert_eq!(accts[1]["app_secret"], "BBB");
        assert_eq!(profile["preferences"]["tone"], "正式");
    }

    #[test]
    fn interaction_patterns_shallow_merge() {
        let mut profile = json!({
            "interaction_patterns": { "asks_for_schedule": true, "lang": "zh" }
        });
        apply_object_shallow_merge(&mut profile, "interaction_patterns", &json!({"lang": "en"}));
        assert_eq!(profile["interaction_patterns"]["asks_for_schedule"], true);
        assert_eq!(profile["interaction_patterns"]["lang"], "en");
    }

    #[test]
    fn prefs_tone_only_does_not_drop_accounts() {
        let mut profile = json!({
            "preferences": {
                "wechat_accounts": [{"app_id": "wxAAA", "app_secret": "AAA"}]
            }
        });
        apply_preferences_update(&mut profile, &json!({"tone": "轻松"}));
        let accts = profile["preferences"]["wechat_accounts"]
            .as_array()
            .unwrap();
        assert_eq!(accts.len(), 1);
        assert_eq!(accts[0]["app_secret"], "AAA");
        assert_eq!(profile["preferences"]["tone"], "轻松");
    }

    #[tokio::test]
    async fn user_profile_update_merges_accounts_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let input = json!({
            "action": "update",
            "updates": {
                "preferences": {
                    "wechat_accounts": [{"app_id": "wxAAA", "app_secret": "AAA"}]
                }
            }
        });
        tool_user_profile(
            &input,
            Some(home),
            Some("agent1"),
            Some("owner1"),
            Some("owner1"),
        )
        .await
        .unwrap();

        let input2 = json!({
            "action": "update",
            "updates": {
                "preferences": {
                    "wechat_accounts": [{"app_id": "wxBBB", "app_secret": "BBB"}]
                }
            }
        });
        tool_user_profile(
            &input2,
            Some(home),
            Some("agent1"),
            Some("owner1"),
            Some("owner1"),
        )
        .await
        .unwrap();

        let path = carrier_types::config::sender_data_dir(home, "owner1", "agent1", Some("owner1"))
            .join("profile.json");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let accts = saved["preferences"]["wechat_accounts"].as_array().unwrap();
        assert_eq!(accts.len(), 2);
        assert_eq!(accts[0]["app_id"], "wxAAA");
        assert_eq!(accts[1]["app_id"], "wxBBB");
    }

    #[tokio::test]
    async fn user_profile_remove_account_deletes_only_named_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        for app_id in ["wxAAA", "wxBBB"] {
            let input = json!({
                "action": "update",
                "updates": {
                    "preferences": {
                        "wechat_accounts": [{"app_id": app_id, "app_secret": "s"}]
                    }
                }
            });
            tool_user_profile(
                &input,
                Some(home),
                Some("agent1"),
                Some("owner1"),
                Some("owner1"),
            )
            .await
            .unwrap();
        }

        let out = tool_user_profile(
            &json!({"action": "remove_account", "app_id": "wxAAA"}),
            Some(home),
            Some("agent1"),
            Some("owner1"),
            Some("owner1"),
        )
        .await
        .unwrap();
        assert!(out.contains("Removed app_id 'wxAAA'"));

        let path = carrier_types::config::sender_data_dir(home, "owner1", "agent1", Some("owner1"))
            .join("profile.json");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let accts = saved["preferences"]["wechat_accounts"].as_array().unwrap();
        assert_eq!(accts.len(), 1);
        assert_eq!(accts[0]["app_id"], "wxBBB");
    }

    #[tokio::test]
    async fn user_profile_remove_account_unknown_id_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        tool_user_profile(
            &json!({
                "action": "update",
                "updates": {
                    "preferences": {
                        "wechat_accounts": [{"app_id": "wxAAA", "app_secret": "s"}]
                    }
                }
            }),
            Some(home),
            Some("agent1"),
            Some("owner1"),
            Some("owner1"),
        )
        .await
        .unwrap();

        let out = tool_user_profile(
            &json!({"action": "remove_account", "app_id": "wxNOPE"}),
            Some(home),
            Some("agent1"),
            Some("owner1"),
            Some("owner1"),
        )
        .await
        .unwrap();
        assert!(out.contains("not found"));

        let path = carrier_types::config::sender_data_dir(home, "owner1", "agent1", Some("owner1"))
            .join("profile.json");
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let accts = saved["preferences"]["wechat_accounts"].as_array().unwrap();
        assert_eq!(accts.len(), 1, "unknown app_id must not delete anything");
    }

    #[tokio::test]
    async fn user_profile_remove_account_requires_app_id() {
        let tmp = tempfile::tempdir().unwrap();
        let err = tool_user_profile(
            &json!({"action": "remove_account"}),
            Some(tmp.path()),
            Some("agent1"),
            Some("owner1"),
            Some("owner1"),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("app_id"));
    }
}
