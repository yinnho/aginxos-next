//! Collaboration tools: agent_find, task_post, task_claim, task_complete,
//! task_list, task_plan, event_publish.

use super::ToolModule;
use crate::kernel_handle::KernelHandle;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};

// ---------------------------------------------------------------------------
// Collaboration tools
// ---------------------------------------------------------------------------

fn tool_agent_find(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let query = input["query"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'query' parameter".to_string(),
    ))?;
    let agents = kh.find_agents(query);
    if agents.is_empty() {
        return Ok(format!("No agents found matching '{query}'."));
    }
    let result: Vec<serde_json::Value> = agents
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "name": a.name,
                "state": a.state,
                "description": a.description,
                "tags": a.tags,
                "tools": a.tools,
                "model": format!("{}:{}", a.modality, a.model),
            })
        })
        .collect();
    serde_json::to_string_pretty(&result).map_err(|e| CarrierError::Serialization(e.to_string()))
}

async fn tool_task_post(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let title = input["title"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'title' parameter".to_string(),
    ))?;
    let description = input["description"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'description' parameter".to_string(),
        ))?;
    let assigned_to = input["assigned_to"].as_str();
    let task_id = kh
        .task_post(title, description, assigned_to, caller_agent_id)
        .await?;
    Ok(format!("Task created with ID: {task_id}"))
}

fn tool_task_plan(input: &serde_json::Value) -> CarrierResult<String> {
    let title = input["title"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'title' parameter".to_string(),
    ))?;
    let steps = input["steps"].as_array().ok_or(CarrierError::InvalidInput(
        "Missing 'steps' parameter".to_string(),
    ))?;
    if steps.is_empty() {
        return Err(CarrierError::InvalidInput(
            "Steps array must not be empty".to_string(),
        ));
    }
    let ids: Vec<&str> = steps.iter().filter_map(|s| s["id"].as_str()).collect();
    if ids.len() != steps.len() {
        return Err(CarrierError::InvalidInput(
            "All steps must have an 'id' field".to_string(),
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for &id in &ids {
        if !seen.insert(id) {
            return Err(CarrierError::InvalidInput(format!(
                "Duplicate step id: '{}'",
                id
            )));
        }
    }
    for step in steps {
        let id = step["id"]
            .as_str()
            .ok_or(CarrierError::InvalidInput("Step missing 'id'".to_string()))?;
        if step["prompt"].as_str().is_none() {
            return Err(CarrierError::InvalidInput(format!(
                "Step '{}' missing 'prompt'",
                id
            )));
        }
        if let Some(deps) = step["depends_on"].as_array() {
            for dep in deps {
                let dep_str = dep.as_str().unwrap_or("");
                if !ids.contains(&dep_str) {
                    return Err(CarrierError::InvalidInput(format!(
                        "Step '{}' depends_on unknown step '{}'",
                        id, dep_str
                    )));
                }
            }
        }
    }
    Ok(format!(
        "Plan '{}' accepted with {} steps. Execution will begin now.",
        title,
        steps.len()
    ))
}

async fn tool_task_claim(
    kernel: Option<&Arc<dyn KernelHandle>>,
    caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let agent_id = caller_agent_id.ok_or(CarrierError::Internal(
        "Missing caller agent identity".to_string(),
    ))?;
    match kh.task_claim(agent_id).await? {
        Some(task) => serde_json::to_string_pretty(&task)
            .map_err(|e| CarrierError::Serialization(e.to_string())),
        None => Ok("No tasks available.".to_string()),
    }
}

async fn tool_task_complete(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let task_id = input["task_id"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'task_id' parameter".to_string(),
    ))?;
    let result = input["result"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'result' parameter".to_string(),
    ))?;
    kh.task_complete(task_id, result).await?;
    Ok(format!("Task {task_id} marked as completed."))
}

async fn tool_task_list(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let status = input["status"].as_str();
    let tasks = kh.task_list(status).await?;
    if tasks.is_empty() {
        return Ok("No tasks found.".to_string());
    }
    serde_json::to_string_pretty(&tasks).map_err(|e| CarrierError::Serialization(e.to_string()))
}

async fn tool_event_publish(
    input: &serde_json::Value,
    kernel: Option<&Arc<dyn KernelHandle>>,
    _caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let kh = crate::tools::require_kernel(kernel)?;
    let event_type = input["event_type"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'event_type' parameter".to_string(),
        ))?;
    let payload = input
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    kh.publish_event(event_type, payload).await?;
    Ok(format!("Event '{event_type}' published successfully."))
}

// ---------------------------------------------------------------------------
// ToolModule implementation
// ---------------------------------------------------------------------------

/// Collaboration tools: agent_find, task_post, task_claim, task_complete,
/// task_list, task_plan, event_publish.
pub struct CollaborationTools;

#[async_trait]
impl ToolModule for CollaborationTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "agent_find".to_string(),
                description: "Discover agents by name, tag, tool, or description. Use to find specialists before delegating work.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query (matches agent name, tags, tools, description)" }
                    },
                    "required": ["query"]
                }),
            },
            ToolDefinition {
                name: "task_post".to_string(),
                description: "Post a task to the shared task queue for another agent to pick up.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Short task title" },
                        "description": { "type": "string", "description": "Detailed task description" },
                        "assigned_to": { "type": "string", "description": "Agent name or ID to assign the task to (optional)" }
                    },
                    "required": ["title", "description"]
                }),
            },
            ToolDefinition {
                name: "task_claim".to_string(),
                description: "Claim the next available task from the task queue assigned to you or unassigned.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "task_complete".to_string(),
                description: "Mark a previously claimed task as completed with a result.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "task_id": { "type": "string", "description": "The task ID to complete" },
                        "result": { "type": "string", "description": "The result or outcome of the task" }
                    },
                    "required": ["task_id", "result"]
                }),
            },
            ToolDefinition {
                name: "task_list".to_string(),
                description: "List tasks in the shared queue, optionally filtered by status (pending, in_progress, completed).".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "status": { "type": "string", "description": "Filter by status: pending, in_progress, completed (optional)" }
                    }
                }),
            },
            ToolDefinition {
                name: "event_publish".to_string(),
                description: "Publish a custom event that can trigger proactive agents. Use to broadcast signals to the agent fleet.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "event_type": { "type": "string", "description": "Type identifier for the event (e.g., 'code_review_requested')" },
                        "payload": { "type": "object", "description": "JSON payload data for the event" }
                    },
                    "required": ["event_type"]
                }),
            },
            ToolDefinition {
                name: "task_plan".to_string(),
                description: "Split a complex task into ordered steps with dependencies. Each step runs as an independent agent turn (up to 15 iterations). Use this when the task is too complex for a single turn — e.g. multi-stage workflows like research -> write -> format -> publish. Steps without dependencies run in parallel; steps with depends_on wait for those steps to complete first. Previous step outputs are injected into the step's prompt automatically. IMPORTANT: when a step should follow a specific flow's rules (e.g. 'article-writer'), set that step's `flow` to the flow name — the flow body + declared tools + max_iterations are then injected for that step just like an active_flow turn. Without `flow`, the step runs with NO flow guidance and the full agent toolset, so it may ignore the flow's tool/usage rules.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Short title for the overall plan"
                        },
                        "steps": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Unique step identifier (e.g. 'research', 'write', 'publish')" },
                                    "prompt": { "type": "string", "description": "What to do in this step — detailed instructions" },
                                    "depends_on": {
                                        "type": "array",
                                        "items": { "type": "string" },
                                        "description": "IDs of steps that must complete before this step starts. Empty = run immediately."
                                    },
                                    "flow": {
                                        "type": "string",
                                        "description": "Optional named flow to load for this step (e.g. 'article-writer'). When set, the flow body + declared tools + max_iterations are injected for this step, exactly like an active_flow turn — set this whenever the step should obey a flow's tool/usage rules. Omit only for steps with no flow."
                                    }
                                },
                                "required": ["id", "prompt"]
                            },
                            "description": "Ordered list of steps. Each step gets its own agent turn."
                        }
                    },
                    "required": ["title", "steps"]
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

        match name {
            // Collaboration tools
            "agent_find" => Some(tool_agent_find(input, kernel, caller_agent_id)),
            "task_post" => Some(tool_task_post(input, kernel, caller_agent_id).await),
            "task_claim" => Some(tool_task_claim(kernel, caller_agent_id).await),
            "task_complete" => Some(tool_task_complete(input, kernel, caller_agent_id).await),
            "task_list" => Some(tool_task_list(input, kernel, caller_agent_id).await),
            "task_plan" => Some(tool_task_plan(input)),
            "event_publish" => Some(tool_event_publish(input, kernel, caller_agent_id).await),
            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> PermissionLevel {
        match tool_name {
            "agent_find" | "task_list" => PermissionLevel::None,
            "task_post" | "task_claim" | "task_complete" | "task_plan" | "event_publish" => {
                PermissionLevel::Write
            }
            _ => PermissionLevel::Dangerous,
        }
    }
}
