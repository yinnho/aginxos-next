//! Declarative API tool types — TOML-driven HTTP tool definitions.
//!
//! Tools are defined in `api_tools.toml` (global or per-workspace) and
//! registered at startup as `ToolProvider` instances. No Rust code needed
//! for the common "call HTTP endpoint, extract JSON fields" pattern.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A single tool definition from `api_tools.toml`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToolDef {
    pub name: String,
    pub description: String,
    pub url: String,
    #[serde(default = "default_method")]
    pub method: String,
    /// Env var name holding the API key (e.g. "AMAP_API_KEY").
    pub auth_env: Option<String>,
    /// Query param name for the API key (e.g. "key").
    pub auth_param: Option<String>,
    #[serde(default)]
    pub params: HashMap<String, ApiParamDef>,
    #[serde(default)]
    pub extract: HashMap<String, ApiExtractDef>,
    #[serde(default)]
    pub error_check: Option<ApiErrorCheck>,
    #[serde(default)]
    pub resolve: HashMap<String, ApiResolveDef>,
    #[serde(default)]
    pub cron: Option<ApiCronDef>,
    /// Context-field injection: auto-fill a param from the ToolContext (e.g.
    /// inject the sender's openid so the agent doesn't have to pass it). Only
    /// injects when the agent didn't already provide the field.
    #[serde(default)]
    pub inject: HashMap<String, ApiInjectDef>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// JSON request body: named params go into the body (signed+sent as-is
    /// when `hmac` is set), the rest go to the query string. Omit if all
    /// params are query params.
    #[serde(default)]
    pub body: Option<ApiBodyDef>,
    /// HMAC-SHA256 request signing (e.g. 86bus `/api/ai/` gateway). When set,
    /// the signature is computed over the sign_template and the resulting
    /// values are sent as headers. The exact serialized body string is both
    /// signed and sent (never re-serialized), matching a signed-request gateway.
    #[serde(default)]
    pub hmac: Option<ApiHmacDef>,
}

/// JSON request body configuration. Listed `fields` are serialized into a JSON
/// object (absent or empty-string params omitted) and sent as the request body.
/// Fields not listed here continue to go to the query string.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiBodyDef {
    /// Param names to include in the JSON body, in this order.
    #[serde(default)]
    pub fields: Vec<String>,
}

/// HMAC-SHA256 request signing configuration.
///
/// Example (86bus charter gateway):
/// ```toml
/// [tool.hmac]
/// key_id_env = "CHARTER_AK"
/// secret_env = "CHARTER_SK"
/// sign_template = "{method}\n{path}\n{timestamp}\n{body}"
/// headers = { "X-Api-Key" = "{key_id}", "X-Timestamp" = "{timestamp}", "X-Signature" = "{signature}" }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiHmacDef {
    /// Env var holding the key id (e.g. CHARTER_AK).
    pub key_id_env: String,
    /// Env var holding the HMAC secret (e.g. CHARTER_SK).
    pub secret_env: String,
    /// Sign string template. Placeholders: `{method}`, `{path}`, `{timestamp}`,
    /// `{body}` (exact serialized body, empty if no body), `{key_id}`.
    pub sign_template: String,
    /// Algorithm. Only `"sha256"` is supported.
    #[serde(default = "default_hmac_algorithm")]
    pub algorithm: String,
    /// Header name -> value template. Placeholders: `{key_id}`, `{timestamp}`,
    /// `{signature}` (hex HMAC-SHA256).
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

fn default_hmac_algorithm() -> String {
    "sha256".to_string()
}

fn default_method() -> String {
    "GET".to_string()
}

/// Input parameter definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiParamDef {
    #[serde(default)]
    pub required: bool,
    #[serde(default = "default_type_string")]
    pub r#type: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
}

fn default_type_string() -> String {
    "string".to_string()
}

/// Output field extraction from JSON response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiExtractDef {
    /// Dot-path into response JSON (e.g. "route.paths[0].distance").
    pub path: Option<String>,
    /// Output type: "int", "float", "string".
    #[serde(default)]
    pub r#type: Option<String>,
    /// Built-in transform: "divide_1000_round1", "divide_60_round", etc.
    pub transform: Option<String>,
    /// If true, this field is derived from other extracted fields, not from API.
    #[serde(default)]
    pub derived: Option<bool>,
    /// For derived fields: which other extracted field to derive from.
    pub from: Option<String>,
    /// For derived tier mapping.
    pub tiers: Option<Vec<ApiTier>>,
}

/// Tier mapping for derived fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiTier {
    /// Less-than-or-equal threshold.
    pub le: Option<f64>,
    /// Output value when condition matches.
    pub value: String,
}

/// Error check: validate response before extraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorCheck {
    /// Field to check in response (e.g. "status").
    pub field: String,
    /// Expected value (e.g. "1" for Amap).
    pub expect: String,
}

/// Pre-request resolution: call another tool to resolve a parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResolveDef {
    /// Name of another api_tool to call.
    pub tool: String,
    /// Param name to pass to that tool.
    pub param: String,
    /// Field to extract from that tool's result.
    pub extract: String,
    /// Condition for when to resolve (e.g. "not_coordinates").
    pub condition: Option<String>,
}

/// Cron definition for periodic API calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiCronDef {
    /// Cron expression (e.g. "0 */6 * * *").
    pub schedule: String,
    /// SQLite database path (relative to workspace).
    pub save_to: Option<String>,
    /// Table name for auto-creation.
    pub table: Option<String>,
}

/// Context-field injection — auto-fill a param from the ToolContext so the
/// agent doesn't have to pass it. Example: inject the 公众号 sender's openid
/// into a `query` call (`openid = { from = "sender_id", channel = "weixin-oa" }`).
/// Only injects when (a) the agent didn't already provide the field, and
/// (b) the optional `channel` matches the turn's channel_type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiInjectDef {
    /// Context source. Currently only `"sender_id"` (the turn's sender id —
    /// for weixin-oa this is the 服务号 openid).
    pub from: String,
    /// Only inject when the turn's `channel_type` equals this (e.g.
    /// "weixin-oa"). None = inject on any channel.
    #[serde(default)]
    pub channel: Option<String>,
    /// Only inject when ALL of these fields are absent from the agent's args
    /// (e.g. `["order_no", "phone"]` — don't inject openid if the agent already
    /// gave an alternative identifier). Empty = no such guard.
    #[serde(default)]
    pub only_if_absent: Vec<String>,
}

/// Parsed api_tools.toml — array of tool definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToolsConfig {
    pub tool: Vec<ApiToolDef>,
}

impl ApiToolDef {
    /// Build the JSON Schema for this tool's parameters.
    pub fn input_schema_json(&self) -> String {
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for (name, param) in &self.params {
            let mut prop = serde_json::json!({
                "type": match param.r#type.as_str() {
                    "int" | "integer" => "integer",
                    "float" | "number" => "number",
                    "bool" | "boolean" => "boolean",
                    _ => "string",
                }
            });
            if !param.description.is_empty() {
                prop["description"] = serde_json::Value::String(param.description.clone());
            }
            if let Some(ref default) = param.default {
                prop["default"] = default.clone();
            }
            properties.insert(name.clone(), prop);
            if param.required {
                required.push(serde_json::Value::String(name.clone()));
            }
        }

        // Add auth param if not already in params (some APIs include it in URL template)
        // Don't expose auth params to the LLM — they're injected automatically.

        let schema = serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
        });

        serde_json::to_string(&schema).unwrap_or_else(|_| "{}".to_string())
    }
}
