//! Tool definition and result types.

use serde::{Deserialize, Serialize};

/// Permission level for a tool — used to filter tools per channel.
///
/// Levels are ordered: None < ReadOnly < Write < Execute < Dangerous.
/// A channel's `max_permission` caps which tools are visible to the LLM.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionLevel {
    /// Pure queries with no side effects: knowledge_read, etc.
    None,
    /// Read from external sources: web_fetch, file_read.
    ReadOnly,
    /// Write within sandbox: file_write (workspace), system_kv_store (own ns).
    #[default]
    Write,
    /// Cross-boundary writes: file_write (arbitrary), agent_send, process_start.
    Execute,
    /// Irreversible operations: shell_exec, file_delete, process_kill.
    Dangerous,
}

impl<'de> serde::Deserialize<'de> for PermissionLevel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "none" => Ok(Self::None),
            "readonly" | "read_only" | "read" => Ok(Self::ReadOnly),
            "write" => Ok(Self::Write),
            "execute" | "exec" => Ok(Self::Execute),
            "dangerous" | "danger" => Ok(Self::Dangerous),
            other => Err(serde::de::Error::unknown_variant(
                other,
                &["none", "readonly", "write", "execute", "dangerous"],
            )),
        }
    }
}

impl PermissionLevel {
    /// Serde default for max_tool_level: agents default to Write, not None.
    pub const fn default_max_tool_level() -> Self {
        Self::Write
    }

    /// Get the permission level for a tool by name.
    ///
    /// Uses a centralized mapping of tool names to levels. Falls back to
    /// Dangerous for unknown tools (fail-safe). MCP tools (mcp_*) default
    /// to Write since they are user-configured and trusted by default.
    ///
    /// Toolset-prefixed names (e.g. `filesystem__file_write`) are stripped
    /// to the base name (`file_write`) before matching.
    pub fn for_tool(name: &str) -> Self {
        // Strip toolset prefix: "filesystem__file_write" → "file_write"
        let base_name = if let Some(pos) = name.find("__") {
            &name[pos + 2..]
        } else {
            name
        };
        match base_name {
            // None — pure queries, no side effects
            "session_summarize"
            | "knowledge_list" | "knowledge_read" | "flow_load"
            | "agent_find" | "agent_list"
            | "train_read" | "train_list" | "train_knowledge_list"
            | "train_knowledge_read" | "train_evaluate" | "user_profile"
            | "task_list" | "schedule_list" | "cron_list"
            | "a2a_discover" | "clone_evaluate"
            | "knowledge_lint" | "knowledge_index" | "knowledge_extract"
            | "train_knowledge_lint" | "clone_export"
            | "memory_tree" | "data_analyze" => Self::None,

            // ReadOnly — reads from external sources
            "file_read" | "file_list" | "file_convert"
            | "web_fetch"
            | "web_search"
            | "browser_navigate" | "browser_read_page" | "browser_evaluate"
            | "browser_click" | "browser_type" | "browser_scroll"
            | "browser_wait" | "browser_back" | "browser_screenshot"
            | "browser_close"
            | "image_analyze" | "media_describe" | "media_transcribe"
            | "speech_to_text" | "location_get" | "system_time" => Self::ReadOnly,

            // Write — writes within sandbox
            "file_write"
            | "knowledge_add" | "knowledge_remove" | "knowledge_import"
            | "knowledge_heal" | "flow_create" | "flow_update"
            | "apply_patch" | "train_write"
            | "image_generate" | "text_to_speech" | "canvas_present"
            | "task_post" | "task_claim" | "task_complete"
            | "event_publish" | "schedule_create" | "schedule_delete"
            | "cron_create" | "cron_cancel"
            // clone_install — writes into workspaces/<name>/ with name + traversal validation
            // clone_publish — external push, gated by admin-configured hub api_key + URL validation
            | "clone_install" | "clone_publish"
            // charter_create_order — creates a real order + notifies admins (external side effect)
            | "charter_create_order"
            // contact_prompt — send a message to a local clone or remote agent://
            // contact (spend gated by the owner's consent flow on the far side);
            // contacts_list — read-only listing. Both are the system identity
            // 「me」's hub tools, injected via its flow's tools: declaration.
            | "contact_prompt" | "contacts_list" => Self::Write,

            // Execute — cross-boundary writes
            "process_start" | "process_poll"
            | "process_write" | "process_list"
            | "agent_send" | "agent_spawn" | "agent_restart"
            | "a2a_send" => Self::Execute,

            // Subagent delegation
            n if n.starts_with("delegate_") => Self::Execute,

            // Dangerous — irreversible operations
            "shell_exec" | "process_kill" | "agent_kill" => Self::Dangerous,

            // Whitelisted CLI execution — restricted to config allowlist, safe for Write-level agents
            "cli_exec" => Self::Write,

            // MCP tools default to Write (user-configured, trusted by default)
            n if n.starts_with("mcp_") || name.starts_with("mcp_") => Self::Write,

            // SQLite tools — database queries and writes (agent's own workspace)
            "sqlite_query" | "sqlite_schema" => Self::Write,

            // Unknown tools default to Dangerous (fail-safe)
            _ => Self::Dangerous,
        }
    }
}

/// Whether a tool requires a clone admin (creator or approved admin) to execute.
///
/// This is an **additional gate, orthogonal to `PermissionLevel`**: even when an
/// agent's `max_tool_level` would permit the tool, these irreversible /
/// brand-affecting actions still require the caller to be an admin:
/// - `shell_exec` — arbitrary system command execution
/// - `*_publish_draft` — publishing content to a public account (e.g.
///   `mcp_wechat_oa_publish_draft`); drafting (`create_draft`) stays open
///
/// Self-evolution tools (`flow_create`, `flow_update`, `knowledge_add`) are
/// intentionally **not** gated — the clone uses them in its autonomous
/// judgment/evolution loop. See `docs/ADMIN-MECHANISM.md`.
pub fn is_admin_gated(name: &str) -> bool {
    // Strip toolset prefix ("filesystem__shell_exec" → "shell_exec"); mcp_ prefix
    // is left intact since `*_publish_draft` matches by suffix either way.
    let base = if let Some(pos) = name.find("__") {
        &name[pos + 2..]
    } else {
        name
    };
    base == "shell_exec"
        || base == "data_analyze"
        || base == "train_write"
        || base == "knowledge_remove"
        || base.ends_with("publish_draft")
}

/// Definition of a tool that an agent can use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// Unique tool identifier.
    pub name: String,
    /// Human-readable description for the LLM.
    pub description: String,
    /// JSON Schema for the tool's input parameters.
    pub input_schema: serde_json::Value,
}

/// A tool call requested by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique ID for this tool use instance.
    pub id: String,
    /// Which tool to call.
    pub name: String,
    /// The input parameters.
    pub input: serde_json::Value,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The tool_use ID this result corresponds to.
    pub tool_use_id: String,
    /// The output content.
    pub content: String,
    /// Whether the tool execution resulted in an error.
    pub is_error: bool,
}

/// Normalize a JSON Schema for cross-provider compatibility.
///
/// Some providers reject `anyOf` in tool schemas. This function:
/// - Converts `anyOf` arrays of simple types to flat `enum` arrays
/// - Strips `$schema` keys (not accepted by most providers)
/// - Recursively walks `properties` and `items`
pub fn normalize_schema_for_provider(
    schema: &serde_json::Value,
    _provider: &str,
) -> serde_json::Value {
    normalize_schema_recursive(schema)
}

fn normalize_schema_recursive(schema: &serde_json::Value) -> serde_json::Value {
    let obj = match schema.as_object() {
        Some(o) => o,
        None => {
            // If the schema is a JSON string, try to parse it as a JSON object.
            // Some MCP servers / skill definitions serialize schemas as strings.
            if let Some(s) = schema.as_str() {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                    if parsed.is_object() {
                        return normalize_schema_recursive(&parsed);
                    }
                }
            }
            // Non-object schema (null, number, bool, unparseable string, array) —
            // return a valid empty object schema so providers don't reject it.
            return serde_json::json!({"type": "object", "properties": {}});
        }
    };

    // Resolve $ref references before processing.
    // If the schema has $defs and $ref, inline the referenced definition.
    let resolved = resolve_refs(obj);
    let obj = resolved.as_object().unwrap_or(obj);

    let mut result = serde_json::Map::new();

    for (key, value) in obj {
        // Strip fields unsupported by Gemini and most non-Anthropic providers
        if matches!(
            key.as_str(),
            "$schema"
                | "$defs"
                | "$ref"
                | "additionalProperties"
                | "default"
                | "$id"
                | "$comment"
                | "examples"
                | "title"
                | "const"
                | "format"
        ) {
            continue;
        }

        // Convert anyOf/oneOf to flat type + enum when possible
        if key == "anyOf" || key == "oneOf" {
            if let Some(converted) = try_flatten_any_of(value) {
                for (k, v) in converted {
                    result.insert(k, v);
                }
                continue;
            }
            // Can't flatten — strip entirely rather than leave unsupported keyword
            continue;
        }

        // Flatten type arrays like ["string", "null"] to single type.
        // Note: we do NOT emit "nullable": true because many OpenAI-compatible
        // providers (Kimi, etc.) don't support it and return parse errors.
        if key == "type" {
            if let Some(arr) = value.as_array() {
                let types: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                let has_null = types.contains(&"null");
                let non_null: Vec<&&str> = types.iter().filter(|&&t| t != "null").collect();
                if has_null && non_null.len() == 1 {
                    // ["string", "null"] → type: "string"
                    result.insert(
                        "type".to_string(),
                        serde_json::Value::String(non_null[0].to_string()),
                    );
                    continue;
                } else if non_null.len() == 1 {
                    // ["string"] → type: "string"
                    result.insert(
                        "type".to_string(),
                        serde_json::Value::String(non_null[0].to_string()),
                    );
                    continue;
                } else if !non_null.is_empty() {
                    // Multiple non-null types — pick first (best effort)
                    result.insert(
                        "type".to_string(),
                        serde_json::Value::String(non_null[0].to_string()),
                    );
                    continue;
                }
            }
            // Scalar type string — pass through
            result.insert(key.clone(), value.clone());
            continue;
        }

        // Recurse into properties
        if key == "properties" {
            if let Some(props) = value.as_object() {
                let mut new_props = serde_json::Map::new();
                for (prop_name, prop_schema) in props {
                    new_props.insert(prop_name.clone(), normalize_schema_recursive(prop_schema));
                }
                result.insert(key.clone(), serde_json::Value::Object(new_props));
                continue;
            }
        }

        // Recurse into items
        if key == "items" {
            result.insert(key.clone(), normalize_schema_recursive(value));
            continue;
        }

        result.insert(key.clone(), value.clone());
    }

    serde_json::Value::Object(result)
}

/// Resolve `$ref` references by inlining definitions from `$defs`.
///
/// If the schema has `$defs` and any property uses `$ref: "#/$defs/Foo"`,
/// replace the `$ref` with the actual definition. This is needed because
/// Gemini and most providers don't support `$ref`/`$defs`.
fn resolve_refs(obj: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let defs = match obj.get("$defs").and_then(|d| d.as_object()) {
        Some(d) => d.clone(),
        None => return serde_json::Value::Object(obj.clone()),
    };

    let mut result = obj.clone();
    result.remove("$defs");

    // Recursively replace $ref in the schema
    fn inline_refs(
        val: &mut serde_json::Value,
        defs: &serde_json::Map<String, serde_json::Value>,
        depth: u32,
    ) {
        if depth > 20 {
            return;
        }
        match val {
            serde_json::Value::Object(map) => {
                // If this object is a $ref, replace it with the definition
                if let Some(ref_val) = map.get("$ref").and_then(|r| r.as_str()) {
                    let ref_name = ref_val
                        .strip_prefix("#/$defs/")
                        .or_else(|| ref_val.strip_prefix("#/definitions/"));
                    if let Some(name) = ref_name {
                        if let Some(def) = defs.get(name) {
                            *val = def.clone();
                            // Recurse into the inlined definition
                            inline_refs(val, defs, depth + 1);
                            return;
                        }
                    }
                }
                // Recurse into all values
                for v in map.values_mut() {
                    inline_refs(v, defs, depth + 1);
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr.iter_mut() {
                    inline_refs(item, defs, depth + 1);
                }
            }
            _ => {}
        }
    }

    let mut resolved = serde_json::Value::Object(result);
    inline_refs(&mut resolved, &defs, 0);
    resolved
}

/// Try to flatten an `anyOf` array into a simple type + enum.
///
/// Works when all variants are simple types (string, number, etc.) or
/// when it's a nullable pattern like `anyOf: [{type: "string"}, {type: "null"}]`.
fn try_flatten_any_of(any_of: &serde_json::Value) -> Option<Vec<(String, serde_json::Value)>> {
    let items = any_of.as_array()?;
    if items.is_empty() {
        return None;
    }

    // Check if this is a simple type union (all items have just "type")
    let mut types = Vec::new();
    let mut has_null = false;
    let mut non_null_type = None;

    for item in items {
        let obj = item.as_object()?;
        let type_val = obj.get("type")?.as_str()?;

        if type_val == "null" {
            has_null = true;
        } else {
            types.push(type_val.to_string());
            non_null_type = Some(type_val.to_string());
        }
    }

    // If it's a nullable pattern (type + null), emit the non-null type only.
    // We do NOT emit "nullable": true — many OpenAI-compatible providers
    // (e.g. Kimi) reject it with parse errors.
    if has_null && types.len() == 1 {
        let result = vec![(
            "type".to_string(),
            serde_json::Value::String(non_null_type.unwrap()),
        )];
        return Some(result);
    }

    // If all items are simple types, pick the first non-null type (best effort).
    // Gemini rejects type arrays, so we can't emit ["string", "number"].
    if types.len() == items.len() && types.len() > 1 {
        let result = vec![(
            "type".to_string(),
            serde_json::Value::String(types[0].clone()),
        )];
        return Some(result);
    }

    // Can't flatten — caller will strip the key entirely
    None
}

// ---------------------------------------------------------------------------
// Plugin tool provider trait (migrated from carrier-plugin-sdk)
// ---------------------------------------------------------------------------

/// A tool definition provided by a plugin.
pub struct PluginToolDef {
    /// Unique tool name (must be unique across all plugins).
    pub name: String,
    /// Description shown to the LLM.
    pub description: String,
    /// JSON Schema for the tool's parameters (pre-serialized string).
    pub parameters_json: String,
}

/// A tool provider exposes a callable tool that agents can use.
pub trait ToolProvider: Send + Sync {
    /// Return the tool definition (name, description, parameter schema).
    fn definition(&self) -> PluginToolDef;

    /// Execute the tool with the given arguments and context.
    fn execute(
        &self,
        args: &serde_json::Value,
        context: &crate::plugin::PluginToolContext,
    ) -> Result<String, PluginToolError>;
}

/// Error type returned by tool providers.
pub struct PluginToolError {
    message: String,
}

impl PluginToolError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
        }
    }

    pub fn tool(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
}

impl std::fmt::Display for PluginToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::fmt::Debug for PluginToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PluginToolError({:?})", self.message)
    }
}

impl std::error::Error for PluginToolError {}

impl From<String> for PluginToolError {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for PluginToolError {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

/// Core tool names always included in `CompletionRequest.tools`.
///
/// These are the bootstrap tools every agent gets. （M31 D3 批1 起 tool_search
/// 退役——发现面走 `ag commands`；未列出的工具按 per-agent manifest 注入。）
pub const CORE_TOOL_NAMES: &[&str] = &[
    "flow_load",
    "flow_create",
    "flow_update",
    "session_summarize",
    "knowledge_add",
    "knowledge_read",
    "knowledge_list",
    // In-place knowledge editing is core (not catalog): self-evolution is a
    // first-class clone behavior (server = evolution subject). Without this in
    // the assembled core set, bare interactive turns genuinely can only
    // add/read/list (08-21 86bus: "只能增不能改" complaint) and glm won't know
    // to ask for a tool it has never seen.
    "knowledge_update",
    "file_read",
    "file_list",
    "web_search",
    "web_fetch",
    "kv_get",
    "kv_set",
    "kv_list",
    "cron_create",
    "cron_list",
    "cron_cancel",
    "task_plan",
    "image_generate",
    "document_generate",
    "api_tool_register",
    // `user_profile` must be core (not just catalog): it's the ONLY writer of
    // per-user preferences (e.g. wechat_accounts OA credentials for multi-user
    // clones). The flow `tools:` hard sandbox (e255801) freezes the allow-list
    // to `base ∪ flow.tools`; if user_profile weren't in the assembled core
    // set, publish flows (draft-publisher declares only `file_read`) couldn't
    // save credentials the user provides mid-flow — regressing "一直给 app_secret
    // 一直要". Core = always assembled = always inside every flow's allow-list.
    "user_profile",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_admin_gated() {
        // shell_exec and toolset-prefixed variant
        assert!(is_admin_gated("shell_exec"));
        assert!(is_admin_gated("filesystem__shell_exec"));

        // publish_draft in any mcp namespace
        assert!(is_admin_gated("mcp_wechat_oa_publish_draft"));
        assert!(is_admin_gated("publish_draft"));

        // train_write (cross-clone training) and knowledge_remove (纠偏) are admin-gated
        assert!(is_admin_gated("train_write"));
        assert!(is_admin_gated("knowledge_remove"));

        // NOT gated: self-evolution / drafting tools stay open
        assert!(!is_admin_gated("create_draft"));
        assert!(!is_admin_gated("flow_create"));
        assert!(!is_admin_gated("flow_update"));
        assert!(!is_admin_gated("knowledge_add"));
        assert!(!is_admin_gated("file_write"));
        assert!(!is_admin_gated("knowledge_heal"));
        assert!(!is_admin_gated("apply_patch"));
    }

    #[test]
    fn test_for_tool_clone_lifecycle() {
        // clone_install / clone_publish are Write — callable at clone-creator's
        // max_tool_level=write without flow elevation (no shell_allow on clone-generate).
        assert_eq!(
            PermissionLevel::for_tool("clone_install"),
            PermissionLevel::Write
        );
        assert_eq!(
            PermissionLevel::for_tool("clone_publish"),
            PermissionLevel::Write
        );
        // clone_export is read-only manifest listing.
        assert_eq!(
            PermissionLevel::for_tool("clone_export"),
            PermissionLevel::None
        );
        // toolset-prefixed variants resolve the same way.
        assert_eq!(
            PermissionLevel::for_tool("training__clone_install"),
            PermissionLevel::Write
        );
        assert_eq!(
            PermissionLevel::for_tool("training__clone_export"),
            PermissionLevel::None
        );
    }

    #[test]
    fn test_tool_definition_serialization() {
        let tool = ToolDefinition {
            name: "web_fetch".to_string(),
            description: "Search the web".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query" }
                },
                "required": ["query"]
            }),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("web_fetch"));
    }

    #[test]
    fn test_normalize_schema_strips_dollar_schema() {
        let schema = serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert!(result.get("$schema").is_none());
        assert_eq!(result["type"], "object");
    }

    #[test]
    fn test_normalize_schema_flattens_anyof_nullable() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "null" }
                    ]
                }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        let value_prop = &result["properties"]["value"];
        assert_eq!(value_prop["type"], "string");
        assert!(value_prop.get("nullable").is_none());
        assert!(value_prop.get("anyOf").is_none());
    }

    #[test]
    fn test_normalize_schema_flattens_anyof_multi_type() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {
                    "anyOf": [
                        { "type": "string" },
                        { "type": "number" }
                    ]
                }
            }
        });
        let result = normalize_schema_for_provider(&schema, "groq");
        let value_prop = &result["properties"]["value"];
        // Gemini rejects type arrays — should flatten to first type
        assert_eq!(value_prop["type"], "string");
        assert!(value_prop.get("anyOf").is_none());
    }

    #[test]
    fn test_normalize_schema_nested_properties() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "outer": {
                    "type": "object",
                    "properties": {
                        "inner": {
                            "$schema": "strip_me",
                            "type": "string"
                        }
                    }
                }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert!(result["properties"]["outer"]["properties"]["inner"]
            .get("$schema")
            .is_none());
    }

    #[test]
    fn test_normalize_schema_string_parsed_to_object() {
        // MCP servers may return inputSchema as a JSON string
        let schema = serde_json::Value::String(
            r#"{"type":"object","properties":{"query":{"type":"string"}}}"#.to_string(),
        );
        let result = normalize_schema_for_provider(&schema, "openai");
        assert!(result.is_object());
        assert_eq!(result["type"], "object");
        assert!(result["properties"]["query"].is_object());
    }

    #[test]
    fn test_normalize_schema_null_becomes_empty_object() {
        let schema = serde_json::Value::Null;
        let result = normalize_schema_for_provider(&schema, "openai");
        assert!(result.is_object());
        assert_eq!(result["type"], "object");
    }

    #[test]
    fn test_normalize_schema_unparseable_string_becomes_empty_object() {
        let schema = serde_json::Value::String("not valid json".to_string());
        let result = normalize_schema_for_provider(&schema, "openai");
        assert!(result.is_object());
        assert_eq!(result["type"], "object");
    }

    #[test]
    fn test_normalize_schema_number_becomes_empty_object() {
        let schema = serde_json::json!(42);
        let result = normalize_schema_for_provider(&schema, "openai");
        assert!(result.is_object());
        assert_eq!(result["type"], "object");
    }

    #[test]
    fn test_normalize_schema_string_with_dollar_schema_stripped() {
        // String schema that contains $schema — should be parsed AND normalized
        let schema = serde_json::Value::String(
            r#"{"$schema":"http://json-schema.org/draft-07/schema#","type":"object","properties":{}}"#.to_string(),
        );
        let result = normalize_schema_for_provider(&schema, "openai");
        assert!(result.is_object());
        assert_eq!(result["type"], "object");
        assert!(result.get("$schema").is_none());
    }

    #[test]
    fn test_normalize_strips_additional_properties() {
        let schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "name": { "type": "string", "default": "hello", "title": "Name" }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert!(result.get("additionalProperties").is_none());
        assert!(result["properties"]["name"].get("default").is_none());
        assert!(result["properties"]["name"].get("title").is_none());
        assert_eq!(result["properties"]["name"]["type"], "string");
    }

    #[test]
    fn test_normalize_resolves_refs() {
        let schema = serde_json::json!({
            "type": "object",
            "$defs": {
                "Color": {
                    "type": "string",
                    "enum": ["red", "green", "blue"]
                }
            },
            "properties": {
                "color": { "$ref": "#/$defs/Color" }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert!(result.get("$defs").is_none());
        assert_eq!(result["properties"]["color"]["type"], "string");
        assert!(result["properties"]["color"]["enum"].is_array());
    }

    #[test]
    fn test_normalize_strips_defs_without_refs() {
        let schema = serde_json::json!({
            "type": "object",
            "$defs": { "Unused": { "type": "number" } },
            "properties": {
                "x": { "type": "string" }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert!(result.get("$defs").is_none());
        assert_eq!(result["properties"]["x"]["type"], "string");
    }

    // --- Issue #488 tests ---

    #[test]
    fn test_normalize_strips_const() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "version": { "type": "string", "const": "v1" }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert!(result["properties"]["version"].get("const").is_none());
        assert_eq!(result["properties"]["version"]["type"], "string");
    }

    #[test]
    fn test_normalize_strips_format() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "created_at": { "type": "string", "format": "date-time" },
                "email": { "type": "string", "format": "email" }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert!(result["properties"]["created_at"].get("format").is_none());
        assert!(result["properties"]["email"].get("format").is_none());
        assert_eq!(result["properties"]["created_at"]["type"], "string");
        assert_eq!(result["properties"]["email"]["type"], "string");
    }

    #[test]
    fn test_normalize_flattens_oneof_nullable() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "null" }
                    ]
                }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        let value_prop = &result["properties"]["value"];
        assert_eq!(value_prop["type"], "string");
        assert!(value_prop.get("nullable").is_none());
        assert!(value_prop.get("oneOf").is_none());
    }

    #[test]
    fn test_normalize_strips_oneof_complex() {
        // Complex oneOf that can't be flattened — should be stripped entirely
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "data": {
                    "oneOf": [
                        { "type": "object", "properties": { "a": { "type": "string" } } },
                        { "type": "object", "properties": { "b": { "type": "number" } } }
                    ]
                }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        let data_prop = &result["properties"]["data"];
        assert!(data_prop.get("oneOf").is_none());
    }

    #[test]
    fn test_normalize_flattens_type_array_nullable() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": ["string", "null"] }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        let name_prop = &result["properties"]["name"];
        assert_eq!(name_prop["type"], "string");
        // nullable is not emitted — many OpenAI-compatible providers don't support it
        assert!(name_prop.get("nullable").is_none());
    }

    #[test]
    fn test_normalize_flattens_type_array_multi() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "value": { "type": ["string", "number", "null"] }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        let value_prop = &result["properties"]["value"];
        // Should pick first non-null type
        assert_eq!(value_prop["type"], "string");
        assert!(value_prop.get("nullable").is_none());
    }

    #[test]
    fn test_normalize_flattens_type_array_single() {
        // Single-element type array
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "x": { "type": ["integer"] }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        assert_eq!(result["properties"]["x"]["type"], "integer");
        assert!(result["properties"]["x"].get("nullable").is_none());
    }

    #[test]
    fn test_normalize_strips_anyof_complex() {
        // Complex anyOf that can't be flattened — should be stripped entirely
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "payload": {
                    "anyOf": [
                        { "type": "object", "properties": { "url": { "type": "string" } } },
                        { "type": "array", "items": { "type": "string" } }
                    ]
                }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        let payload_prop = &result["properties"]["payload"];
        assert!(payload_prop.get("anyOf").is_none());
    }

    #[test]
    fn test_normalize_combined_issue_488() {
        // Real-world schema combining multiple #488 issues
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "api_version": { "type": "string", "const": "v2", "format": "semver" },
                "timestamp": { "type": "string", "format": "date-time" },
                "label": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "null" }
                    ]
                },
                "tags": { "type": ["string", "null"] }
            }
        });
        let result = normalize_schema_for_provider(&schema, "gemini");
        // const and format stripped
        assert!(result["properties"]["api_version"].get("const").is_none());
        assert!(result["properties"]["api_version"].get("format").is_none());
        assert!(result["properties"]["timestamp"].get("format").is_none());
        // oneOf flattened — nullable removed for provider compatibility
        assert_eq!(result["properties"]["label"]["type"], "string");
        assert!(result["properties"]["label"].get("nullable").is_none());
        assert!(result["properties"]["label"].get("oneOf").is_none());
        // type array flattened
        assert_eq!(result["properties"]["tags"]["type"], "string");
        assert!(result["properties"]["tags"].get("nullable").is_none());
    }
}
