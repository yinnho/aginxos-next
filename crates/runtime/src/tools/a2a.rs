//! A2A outbound tools (cross-instance agent communication): a2a_discover, a2a_send.

use super::ToolModule;
use crate::kernel_handle::KernelHandle;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};

// ---------------------------------------------------------------------------
// A2A outbound tools
// ---------------------------------------------------------------------------

/// Discover an external A2A agent by fetching its agent card.
async fn tool_a2a_discover(input: &serde_json::Value) -> CarrierResult<String> {
    let url = input["url"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'url' parameter".to_string(),
    ))?;

    // SSRF protection: block private/metadata IPs
    if carrier_types::ssrf::check_ssrf(url).is_err() {
        return Err(CarrierError::InvalidInput(
            "SSRF blocked: URL resolves to a private or metadata address".to_string(),
        ));
    }

    let client = crate::a2a::A2aClient::new();
    let card = client.discover(url).await?;

    serde_json::to_string_pretty(&card)
        .map_err(|e| CarrierError::Serialization(format!("Serialization error: {e}")))
}

/// Send a task to an external A2A agent.
async fn tool_a2a_send(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let message = input["message"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'message' parameter".to_string(),
    ))?;

    // Resolve agent URL: either directly provided or looked up by name
    let url = if let Some(url) = input["agent_url"].as_str() {
        // SSRF protection
        if carrier_types::ssrf::check_ssrf(url).is_err() {
            return Err(CarrierError::InvalidInput(
                "SSRF blocked: URL resolves to a private or metadata address".to_string(),
            ));
        }
        url.to_string()
    } else if let Some(name) = input["agent_name"].as_str() {
        kh.get_a2a_agent_url(name).ok_or(CarrierError::InvalidInput(format!(
            "No known A2A agent with name '{name}'. Use a2a_discover first or provide agent_url directly."
        )))?
    } else {
        return Err(CarrierError::InvalidInput(
            "Missing 'agent_url' or 'agent_name' parameter".to_string(),
        ));
    };

    let session_id = input["session_id"].as_str();
    let client = crate::a2a::A2aClient::new();
    let task = client.send_task(&url, message, session_id).await?;

    serde_json::to_string_pretty(&task)
        .map_err(|e| CarrierError::Serialization(format!("Serialization error: {e}")))
}

// ---------------------------------------------------------------------------
// ToolModule implementation
// ---------------------------------------------------------------------------

/// A2A outbound tools (cross-instance agent communication).
pub struct A2aTools;

#[async_trait]
impl ToolModule for A2aTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "a2a_discover".to_string(),
                description: "Discover an external A2A agent by fetching its agent card from a URL. Returns the agent's name, description, skills, and supported protocols.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "Base URL of the remote OpenCarrier/A2A-compatible agent (e.g., 'https://agent.example.com')" }
                    },
                    "required": ["url"]
                }),
            },
            ToolDefinition {
                name: "a2a_send".to_string(),
                description: "Send a task/message to an external A2A agent and get the response. Use agent_name to send to a previously discovered agent, or agent_url for direct addressing.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string", "description": "The task/message to send to the remote agent" },
                        "agent_url": { "type": "string", "description": "Direct URL of the remote agent's A2A endpoint" },
                        "agent_name": { "type": "string", "description": "Name of a previously discovered A2A agent (looked up from kernel)" },
                        "session_id": { "type": "string", "description": "Optional session ID for multi-turn conversations" }
                    },
                    "required": ["message"]
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

        match name {
            "a2a_discover" => Some(tool_a2a_discover(input).await),
            "a2a_send" => Some(tool_a2a_send(input, kernel).await),
            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "a2a_discover" => PermissionLevel::None,
            "a2a_send" => PermissionLevel::Execute,
            _ => PermissionLevel::Dangerous,
        }
    }
}
