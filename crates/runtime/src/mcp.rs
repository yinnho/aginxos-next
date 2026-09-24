//! MCP (Model Context Protocol) client — connect to external MCP servers.
//!
//! MCP uses JSON-RPC 2.0 over stdio or HTTP+SSE. This module lets Carrier
//! agents use tools from any MCP server (100+ available: GitHub, filesystem,
//! databases, APIs, etc.).
//!
//! All MCP tools are namespaced with `mcp_{server}_{tool}` to prevent collisions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, info, warn};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::ToolDefinition;

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Configuration for an MCP server connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Display name for this server (used in tool namespacing).
    pub name: String,
    /// Brief description of what this MCP server does (shown to LLM in system prompt).
    #[serde(default)]
    pub description: String,
    /// Transport configuration.
    pub transport: McpTransport,
    /// Request timeout in seconds (default: 30).
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Environment variables to pass through to the subprocess (sandboxed).
    #[serde(default)]
    pub env: Vec<String>,
}

fn default_timeout() -> u64 {
    60
}

/// Transport type for MCP server connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    /// Subprocess with JSON-RPC over stdin/stdout.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// HTTP POST to a remote MCP server endpoint.
    /// Despite the name "sse", this is a synchronous HTTP POST (JSON-RPC over HTTP),
    /// not true Server-Sent Events streaming. Named "sse" for config compatibility.
    Sse { url: String },
}

// ---------------------------------------------------------------------------
// Connection types
// ---------------------------------------------------------------------------

/// An active connection to an MCP server.
pub struct McpConnection {
    /// Configuration for this connection.
    config: McpServerConfig,
    /// Tools discovered from the server via tools/list.
    tools: Vec<ToolDefinition>,
    /// Map from namespaced tool name → original tool name from the server.
    /// Needed because `normalize_name` replaces hyphens with underscores,
    /// but the server expects the original name (e.g. "list-connections").
    original_names: HashMap<String, String>,
    /// Transport handle for sending requests.
    transport: McpTransportHandle,
    /// Next JSON-RPC request ID.
    next_id: u64,
}

/// Transport handle — abstraction over stdio subprocess or HTTP.
enum McpTransportHandle {
    Stdio {
        child: Box<tokio::process::Child>,
        stdin: tokio::process::ChildStdin,
        stdout: BufReader<tokio::process::ChildStdout>,
    },
    Sse {
        client: reqwest::Client,
        url: String,
        session_id: Option<String>,
    },
}

/// JSON-RPC 2.0 request.
#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 response.
#[derive(Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[allow(dead_code)]
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JSON-RPC error {}: {}", self.code, self.message)
    }
}

// ---------------------------------------------------------------------------
// McpConnection implementation
// ---------------------------------------------------------------------------

impl McpConnection {
    /// Connect to an MCP server, perform handshake, and discover tools.
    pub async fn connect(config: McpServerConfig) -> CarrierResult<Self> {
        let transport = match &config.transport {
            McpTransport::Stdio { command, args } => {
                Self::connect_stdio(command, args, &config.env).await?
            }
            McpTransport::Sse { url } => {
                // SSRF check: reject private/localhost URLs unless explicitly configured
                Self::connect_sse(url).await?
            }
        };

        let mut conn = Self {
            config,
            tools: Vec::new(),
            original_names: HashMap::new(),
            transport,
            next_id: 1,
        };

        // Initialize handshake
        conn.initialize().await?;

        // Discover tools
        conn.discover_tools().await?;

        info!(
            server = %conn.config.name,
            tools = conn.tools.len(),
            "MCP server connected"
        );

        Ok(conn)
    }

    /// Send the MCP `initialize` handshake.
    async fn initialize(&mut self) -> CarrierResult<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "carrier",
                "version": env!("CARGO_PKG_VERSION")
            }
        });

        let response = self.send_request("initialize", Some(params)).await?;

        if let Some(result) = response {
            info!(
                server = %self.config.name,
                server_info = %result,
                "MCP initialize response"
            );
        }

        // Send initialized notification (no response expected)
        self.send_notification("notifications/initialized", None)
            .await?;

        Ok(())
    }

    /// Discover available tools via `tools/list`.
    async fn discover_tools(&mut self) -> CarrierResult<()> {
        let response = self.send_request("tools/list", None).await?;

        if let Some(result) = response {
            if let Some(tools_array) = result.get("tools").and_then(|t| t.as_array()) {
                let server_name = &self.config.name;
                for tool in tools_array {
                    let raw_name = tool["name"].as_str().unwrap_or("unnamed");
                    let description = tool["description"].as_str().unwrap_or("");
                    let input_schema = tool
                        .get("inputSchema")
                        .cloned()
                        .and_then(|v| {
                            // Ensure input_schema is a JSON object. MCP servers may
                            // return it as a string, null, or omit it entirely.
                            match &v {
                                serde_json::Value::Object(_) => Some(v),
                                serde_json::Value::String(s) => {
                                    serde_json::from_str::<serde_json::Value>(s)
                                        .ok()
                                        .filter(|p| p.is_object())
                                }
                                _ => None,
                            }
                        })
                        .unwrap_or(serde_json::json!({"type": "object"}));

                    // Namespace: mcp_{server}_{tool}
                    let namespaced = format_mcp_tool_name(server_name, raw_name);

                    // Store original name so we can send it back to the server
                    self.original_names
                        .insert(namespaced.clone(), raw_name.to_string());

                    self.tools.push(ToolDefinition {
                        name: namespaced,
                        description: format!("[MCP:{server_name}] {description}"),
                        input_schema,
                    });
                }
            }
        }

        Ok(())
    }

    /// Call a tool on the MCP server.
    ///
    /// `name` should be the namespaced name (mcp_{server}_{tool}).
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: &serde_json::Value,
    ) -> CarrierResult<String> {
        // Look up the original tool name from the server (preserves hyphens etc.)
        let server_name = self.config.name.clone();
        let raw_name = self
            .original_names
            .get(name)
            .cloned()
            .or_else(|| strip_mcp_prefix(&server_name, name).map(|s| s.to_string()))
            .unwrap_or_else(|| name.to_string());

        let params = serde_json::json!({
            "name": raw_name,
            "arguments": arguments,
        });

        let raw_name_log = raw_name.clone();
        let response = self.send_request("tools/call", Some(params)).await.map_err(|e| {
            tracing::warn!(server = %server_name, tool = %raw_name_log, error = %e, "MCP call_tool failed");
            e
        })?;

        match response {
            Some(result) => {
                // Check MCP protocol isError flag
                let is_error = result
                    .get("isError")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Extract text content from the response
                let text = if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
                    let texts: Vec<&str> = content
                        .iter()
                        .filter_map(|item| {
                            if item["type"].as_str() == Some("text") {
                                item["text"].as_str()
                            } else {
                                None
                            }
                        })
                        .collect();
                    texts.join("\n")
                } else {
                    result.to_string()
                };

                if is_error {
                    warn!(server = %self.config.name, tool = %raw_name_log, "MCP tool returned isError=true");
                    Err(CarrierError::Network(format!("MCP tool error: {text}")))
                } else {
                    Ok(text)
                }
            }
            None => {
                warn!(server = %self.config.name, "No result from MCP tools/call");
                Err(CarrierError::Network(
                    "No result from MCP tools/call".to_string(),
                ))
            }
        }
    }

    /// Get the discovered tool definitions.
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }

    /// Get the server name.
    pub fn name(&self) -> &str {
        &self.config.name
    }

    /// Get a clone of the server config (for reconnection).
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }

    /// Ping the server to check if it's still alive.
    /// Sends a `tools/list` request as a lightweight health check.
    pub async fn ping(&mut self) -> CarrierResult<()> {
        self.send_request("tools/list", None).await?;
        Ok(())
    }

    // --- Transport helpers ---

    async fn send_request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> CarrierResult<Option<serde_json::Value>> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let request_json = serde_json::to_string(&request).map_err(|e| {
            CarrierError::Serialization(format!("Failed to serialize request: {e}"))
        })?;

        debug!(method, id, "MCP request");

        match &mut self.transport {
            McpTransportHandle::Stdio { stdin, stdout, .. } => {
                // Write request + newline
                stdin
                    .write_all(request_json.as_bytes())
                    .await
                    .map_err(|e| {
                        CarrierError::Network(format!("Failed to write to MCP stdin: {e}"))
                    })?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|e| CarrierError::Network(format!("Failed to write newline: {e}")))?;
                stdin
                    .flush()
                    .await
                    .map_err(|e| CarrierError::Network(format!("Failed to flush stdin: {e}")))?;

                // Read response line
                let mut line = String::new();
                let timeout = tokio::time::Duration::from_secs(self.config.timeout_secs);
                match tokio::time::timeout(timeout, stdout.read_line(&mut line)).await {
                    Ok(Ok(0)) => {
                        warn!(server = %self.config.name, "MCP server closed connection");
                        return Err(CarrierError::Network(
                            "MCP server closed connection".to_string(),
                        ));
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        warn!(server = %self.config.name, error = %e, "Failed to read MCP response");
                        return Err(CarrierError::Network(format!(
                            "Failed to read MCP response: {e}"
                        )));
                    }
                    Err(_) => {
                        warn!(server = %self.config.name, method, "MCP request timed out");
                        return Err(CarrierError::Network("MCP request timed out".to_string()));
                    }
                }

                let response: JsonRpcResponse = serde_json::from_str(line.trim()).map_err(|e| {
                    warn!(server = %self.config.name, error = %e, "Invalid MCP JSON-RPC response");
                    CarrierError::Serialization(format!("Invalid MCP JSON-RPC response: {e}"))
                })?;

                if let Some(err) = response.error {
                    warn!(server = %self.config.name, method, error = %err, "MCP JSON-RPC error");
                    return Err(CarrierError::Network(format!("{err}")));
                }

                Ok(response.result)
            }
            McpTransportHandle::Sse {
                client,
                url,
                session_id,
            } => {
                let mut req = client
                    .post(url.as_str())
                    .header("Accept", "application/json, text/event-stream")
                    .json(&request)
                    .timeout(std::time::Duration::from_secs(self.config.timeout_secs));

                if let Some(sid) = session_id.as_deref() {
                    req = req.header("mcp-session-id", sid);
                }

                let response = req.send().await.map_err(|e| {
                    warn!(server = %self.config.name, error = %e, "MCP SSE request failed");
                    CarrierError::Network(format!("MCP SSE request failed: {e}"))
                })?;

                if !response.status().is_success() {
                    let status = response.status();
                    warn!(server = %self.config.name, %status, "MCP SSE non-2xx response");
                    return Err(CarrierError::Network(format!("MCP SSE returned {status}")));
                }

                // Persist session ID from server for subsequent requests.
                if let Some(sid) = response.headers().get("mcp-session-id") {
                    if let Ok(sid_str) = sid.to_str() {
                        *session_id = Some(sid_str.to_string());
                    }
                }

                let body = response.text().await.map_err(|e| {
                    warn!(server = %self.config.name, error = %e, "Failed to read SSE response");
                    CarrierError::Network(format!("Failed to read SSE response: {e}"))
                })?;

                // Handle both plain JSON and SSE format (Streamable-HTTP).
                // SSE format: "event: message\ndata: {...}\n\n"
                let json_str = if body.starts_with('{') {
                    body.clone()
                } else {
                    body.lines()
                        .find_map(|line| line.strip_prefix("data: "))
                        .unwrap_or(&body)
                        .to_string()
                };

                let rpc_response: JsonRpcResponse = serde_json::from_str(&json_str)
                    .map_err(|e| {
                        warn!(server = %self.config.name, error = %e, "Invalid MCP SSE JSON-RPC response");
                        CarrierError::Serialization(format!("Invalid MCP SSE JSON-RPC response: {e}"))
                    })?;

                if let Some(err) = rpc_response.error {
                    warn!(server = %self.config.name, method, error = %err, "MCP SSE JSON-RPC error");
                    return Err(CarrierError::Network(format!("{err}")));
                }

                Ok(rpc_response.result)
            }
        }
    }

    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> CarrierResult<()> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params.unwrap_or(serde_json::json!({})),
        });

        let json = serde_json::to_string(&notification).map_err(|e| {
            CarrierError::Serialization(format!("Failed to serialize notification: {e}"))
        })?;

        match &mut self.transport {
            McpTransportHandle::Stdio { stdin, .. } => {
                stdin
                    .write_all(json.as_bytes())
                    .await
                    .map_err(|e| CarrierError::Network(format!("Write notification: {e}")))?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|e| CarrierError::Network(format!("Write newline: {e}")))?;
                stdin
                    .flush()
                    .await
                    .map_err(|e| CarrierError::Network(format!("Flush: {e}")))?;
            }
            McpTransportHandle::Sse {
                client,
                url,
                session_id,
            } => {
                let mut req = client
                    .post(url.as_str())
                    .header("Accept", "application/json, text/event-stream")
                    .json(&notification);

                if let Some(sid) = session_id.as_deref() {
                    req = req.header("mcp-session-id", sid);
                }
                if let Err(e) = req.send().await {
                    warn!(server = %self.config.name, error = %e, "MCP SSE notification send failed");
                }
            }
        }

        Ok(())
    }

    async fn connect_stdio(
        command: &str,
        args: &[String],
        env_whitelist: &[String],
    ) -> CarrierResult<McpTransportHandle> {
        // Validate command path (no path traversal)
        if command.contains("..") {
            return Err(CarrierError::InvalidInput(
                "MCP command path contains '..': rejected".to_string(),
            ));
        }

        // On Windows, npm/npx install as .cmd batch wrappers. Detect and adapt.
        let resolved_command: String = if cfg!(windows) {
            // If the user already specified .cmd/.bat, use as-is
            if command.ends_with(".cmd") || command.ends_with(".bat") {
                command.to_string()
            } else {
                // Check if the .cmd variant exists on PATH
                let cmd_variant = format!("{command}.cmd");
                let has_cmd = std::env::var("PATH")
                    .unwrap_or_default()
                    .split(';')
                    .any(|dir| std::path::Path::new(dir).join(&cmd_variant).exists());
                if has_cmd {
                    cmd_variant
                } else {
                    command.to_string()
                }
            }
        } else {
            command.to_string()
        };

        let mut cmd = tokio::process::Command::new(&resolved_command);
        cmd.args(args);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Sandbox: clear environment, only pass whitelisted vars
        cmd.env_clear();
        for var_name in env_whitelist {
            if let Ok(val) = std::env::var(var_name) {
                cmd.env(var_name, val);
            }
        }
        // Always pass PATH for binary resolution
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        // On Windows, npm/node need APPDATA, USERPROFILE, LOCALAPPDATA, and SystemRoot
        if cfg!(windows) {
            for var in &[
                "APPDATA",
                "LOCALAPPDATA",
                "USERPROFILE",
                "SystemRoot",
                "TEMP",
                "TMP",
                "HOME",
                "HOMEDRIVE",
                "HOMEPATH",
            ] {
                if let Ok(val) = std::env::var(var) {
                    cmd.env(var, val);
                }
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            CarrierError::Internal(format!(
                "Failed to spawn MCP server '{resolved_command}': {e}"
            ))
        })?;

        // Log stderr in background for debugging MCP server issues
        if let Some(stderr) = child.stderr.take() {
            let cmd_name = resolved_command.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    // INFO for errors/warnings from MCP server, DEBUG for routine output
                    if line.contains("ERROR")
                        || line.contains("WARN")
                        || line.contains("error")
                        || line.contains("panic")
                    {
                        tracing::warn!(mcp_server = %cmd_name, "stderr: {line}");
                    } else {
                        tracing::debug!(mcp_server = %cmd_name, "stderr: {line}");
                    }
                }
            });
        }

        let stdin = child.stdin.take().ok_or(CarrierError::Internal(
            "Failed to capture MCP server stdin".to_string(),
        ))?;
        let stdout = child.stdout.take().ok_or(CarrierError::Internal(
            "Failed to capture MCP server stdout".to_string(),
        ))?;

        Ok(McpTransportHandle::Stdio {
            child: Box::new(child),
            stdin,
            stdout: BufReader::new(stdout),
        })
    }

    async fn connect_sse(url: &str) -> CarrierResult<McpTransportHandle> {
        // Full SSRF protection using shared module
        carrier_types::ssrf::check_ssrf(url)?;

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| CarrierError::Network(format!("Failed to create HTTP client: {e}")))?;

        Ok(McpTransportHandle::Sse {
            client,
            url: url.to_string(),
            session_id: None,
        })
    }
}

impl Drop for McpConnection {
    fn drop(&mut self) {
        match &mut self.transport {
            McpTransportHandle::Stdio { ref mut child, .. } => {
                // Best-effort kill of the subprocess
                if let Err(e) = child.start_kill() {
                    warn!(server = %self.config.name, error = %e, "Failed to kill MCP subprocess");
                }
            }
            McpTransportHandle::Sse { client, .. } => {
                // reqwest Client uses a connection pool; this clears it.
                let _ = client;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tool namespacing helpers
// ---------------------------------------------------------------------------

/// Format a namespaced MCP tool name: `mcp_{server}_{tool}`.
///
/// If the tool name already starts with `{server_}`, the prefix is stripped
/// to avoid duplication (e.g. server="browser", tool="browser_navigate"
/// → "mcp_browser_navigate", not "mcp_browser_browser_navigate").
pub fn format_mcp_tool_name(server: &str, tool: &str) -> String {
    let norm_server = normalize_name(server);
    let norm_tool = normalize_name(tool);
    // Strip redundant server prefix from tool name
    let server_prefix = format!("{norm_server}_");
    let tool_part = norm_tool.strip_prefix(&server_prefix).unwrap_or(&norm_tool);
    format!("mcp_{norm_server}_{tool_part}")
}

/// Check if a tool name is an MCP-namespaced tool.
pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp_")
}

/// Extract server name from an MCP tool name.
///
/// Falls back to first-underscore heuristic, but prefer
/// `extract_mcp_server_from_known()` which handles server names containing
/// hyphens (normalized to underscores) correctly.
pub fn extract_mcp_server(tool_name: &str) -> Option<&str> {
    if !tool_name.starts_with("mcp_") {
        return None;
    }
    let rest = &tool_name[4..];
    rest.find('_').map(|pos| &rest[..pos])
}

/// Extract the original server name by matching against known server names.
///
/// This handles server names with hyphens (e.g. "my-api") correctly —
/// the normalized prefix "mcp_my_api_" is matched against each known
/// server's normalized name, returning the original (unhyphenated) name.
pub fn extract_mcp_server_from_known<'a>(
    tool_name: &str,
    server_names: &[&'a str],
) -> Option<&'a str> {
    if !tool_name.starts_with("mcp_") {
        return None;
    }
    // Sort by length descending so longer (more specific) names match first
    let mut sorted: Vec<&&str> = server_names.iter().collect();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.len()));
    for name in sorted {
        let prefix = format!("mcp_{}_", normalize_name(name));
        if tool_name.starts_with(&prefix) {
            return Some(name);
        }
    }
    None
}

/// Strip the MCP namespace prefix from a tool name.
fn strip_mcp_prefix<'a>(server: &str, tool_name: &'a str) -> Option<&'a str> {
    let prefix = format!("mcp_{}_", normalize_name(server));
    tool_name.strip_prefix(&prefix)
}

/// Normalize a name for use in tool namespacing (lowercase, replace hyphens).
pub fn normalize_name(name: &str) -> String {
    name.to_lowercase().replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_tool_namespacing() {
        assert_eq!(
            format_mcp_tool_name("github", "create_issue"),
            "mcp_github_create_issue"
        );
        assert_eq!(
            format_mcp_tool_name("my-server", "do_thing"),
            "mcp_my_server_do_thing"
        );
        // Redundant server prefix is stripped
        assert_eq!(
            format_mcp_tool_name("browser", "browser_navigate"),
            "mcp_browser_navigate"
        );
        assert_eq!(
            format_mcp_tool_name("wecom", "wecom_send_message"),
            "mcp_wecom_send_message"
        );
        assert_eq!(
            format_mcp_tool_name("feishu", "feishu_send_message"),
            "mcp_feishu_send_message"
        );
    }

    #[test]
    fn test_is_mcp_tool() {
        assert!(is_mcp_tool("mcp_github_create_issue"));
        assert!(!is_mcp_tool("file_read"));
        assert!(!is_mcp_tool(""));
    }

    #[test]
    fn test_hyphenated_tool_name_preserved() {
        // Tool names with hyphens get normalized to underscores for namespacing,
        // but original_names map preserves the original for call_tool dispatch.
        let namespaced = format_mcp_tool_name("sqlcl", "list-connections");
        assert_eq!(namespaced, "mcp_sqlcl_list_connections");

        // Simulate what discover_tools does
        let mut original_names = HashMap::new();
        original_names.insert(namespaced.clone(), "list-connections".to_string());

        // call_tool should resolve to original hyphenated name
        let raw = original_names
            .get(&namespaced)
            .map(|s| s.as_str())
            .unwrap_or("list_connections");
        assert_eq!(raw, "list-connections");
    }

    #[test]
    fn test_extract_mcp_server() {
        assert_eq!(
            extract_mcp_server("mcp_github_create_issue"),
            Some("github")
        );
        assert_eq!(extract_mcp_server("file_read"), None);
    }

    #[test]
    fn test_extract_mcp_server_from_known_with_hyphens() {
        // Server "my-api" normalized to "my_api" in tool prefix
        let servers = vec!["my-api", "github"];
        let tool = "mcp_my_api_get_data";
        assert_eq!(
            extract_mcp_server_from_known(tool, &servers),
            Some("my-api")
        );
        // Simple server name still works
        assert_eq!(
            extract_mcp_server_from_known("mcp_github_create_issue", &servers),
            Some("github")
        );
        // Non-MCP tool returns None
        assert_eq!(extract_mcp_server_from_known("file_read", &servers), None);
    }

    #[test]
    fn test_extract_mcp_server_from_known_longest_match() {
        // "my-api" and "my-api-v2" — should match the longer one
        let servers = vec!["my-api", "my-api-v2"];
        assert_eq!(
            extract_mcp_server_from_known("mcp_my_api_v2_get_users", &servers),
            Some("my-api-v2")
        );
        assert_eq!(
            extract_mcp_server_from_known("mcp_my_api_list_items", &servers),
            Some("my-api")
        );
    }

    #[test]
    fn test_mcp_jsonrpc_initialize() {
        // Verify the initialize request structure
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "initialize".to_string(),
            params: Some(serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "carrier",
                    "version": "0.1.0"
                }
            })),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("initialize"));
        assert!(json.contains("protocolVersion"));
        assert!(json.contains("carrier"));
    }

    #[test]
    fn test_mcp_jsonrpc_tools_list() {
        // Simulate a tools/list response
        let response_json = r#"{
            "jsonrpc": "2.0",
            "id": 2,
            "result": {
                "tools": [
                    {
                        "name": "create_issue",
                        "description": "Create a GitHub issue",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "title": {"type": "string"},
                                "body": {"type": "string"}
                            },
                            "required": ["title"]
                        }
                    }
                ]
            }
        }"#;

        let response: JsonRpcResponse = serde_json::from_str(response_json).unwrap();
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"].as_str().unwrap(), "create_issue");
    }

    #[test]
    fn test_mcp_transport_config_serde() {
        let config = McpServerConfig {
            name: "github".to_string(),
            description: String::new(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec![
                    "-y".to_string(),
                    "@modelcontextprotocol/server-github".to_string(),
                ],
            },
            timeout_secs: 30,
            env: vec!["GITHUB_PERSONAL_ACCESS_TOKEN".to_string()],
        };

        let json = serde_json::to_string(&config).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "github");
        assert_eq!(back.timeout_secs, 30);
        assert_eq!(back.env, vec!["GITHUB_PERSONAL_ACCESS_TOKEN"]);

        match back.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "npx");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("Expected Stdio transport"),
        }

        // SSE variant
        let sse_config = McpServerConfig {
            name: "test".to_string(),
            description: String::new(),
            transport: McpTransport::Sse {
                url: "https://example.com/mcp".to_string(),
            },
            timeout_secs: 60,
            env: vec![],
        };
        let json = serde_json::to_string(&sse_config).unwrap();
        let back: McpServerConfig = serde_json::from_str(&json).unwrap();
        match back.transport {
            McpTransport::Sse { url } => assert_eq!(url, "https://example.com/mcp"),
            _ => panic!("Expected SSE transport"),
        }
    }
}
