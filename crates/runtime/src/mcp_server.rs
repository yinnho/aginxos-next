//! MCP Server — expose Carrier tools via the Model Context Protocol.
//!
//! Implements the server-side MCP protocol so external MCP clients
//! (Claude Desktop, VS Code, etc.) can use Carrier's built-in tools.
//!
//! This module provides a reusable handler function — the CLI team
//! wires it into a stdio transport.

use serde_json::json;
use carrier_types::tool::ToolDefinition;

/// MCP protocol version supported by this server.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Handle an incoming MCP JSON-RPC request and return a response.
///
/// This is a stateless handler that can be called from any transport
/// (stdio, HTTP, etc.). The caller provides the available tool definitions.
pub async fn handle_mcp_request(
    request: &serde_json::Value,
    tools: &[ToolDefinition],
) -> serde_json::Value {
    let method = request["method"].as_str().unwrap_or("");
    let id = request.get("id").cloned();

    match method {
        "initialize" => make_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "carrier",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "notifications/initialized" => {
            // Notification — no response needed
            json!(null)
        }
        "tools/list" => {
            let tool_list: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema,
                    })
                })
                .collect();

            make_response(id, json!({ "tools": tool_list }))
        }
        "tools/call" => {
            // Tool execution is intentionally NOT handled here.
            // The HTTP route (routes.rs::mcp_http) and CLI (cli/mcp.rs::handle_message)
            // handle tools/call themselves by calling execute_tool() directly,
            // because they need access to the kernel / agent context.
            // This handler only validates the request and returns an error
            // directing the caller to use the proper execution path.
            make_error(
                id,
                -32601,
                "tools/call must be handled by the transport layer (HTTP route or CLI), not by this handler",
            )
        }
        _ => make_error(id, -32601, &format!("Method not found: {method}")),
    }
}

/// Build a JSON-RPC 2.0 success response.
fn make_response(id: Option<serde_json::Value>, result: serde_json::Value) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

/// Build a JSON-RPC 2.0 error response.
fn make_error(id: Option<serde_json::Value>, code: i64, message: &str) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "file_read".to_string(),
                description: "Read a file".to_string(),
                input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
            },
            ToolDefinition {
                name: "web_fetch".to_string(),
                description: "Fetch a URL".to_string(),
                input_schema: json!({"type": "object"}),
            },
        ]
    }

    #[tokio::test]
    async fn test_mcp_server_tools_list() {
        let tools = test_tools();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
        });

        let response = handle_mcp_request(&request, &tools).await;
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 1);

        let tool_list = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tool_list.len(), 2);
        assert_eq!(tool_list[0]["name"], "file_read");
        assert_eq!(tool_list[1]["name"], "web_fetch");
    }

    #[tokio::test]
    async fn test_mcp_server_unknown_method() {
        let tools = test_tools();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "nonexistent/method",
        });

        let response = handle_mcp_request(&request, &tools).await;
        assert_eq!(response["jsonrpc"], "2.0");
        assert_eq!(response["id"], 5);
        assert_eq!(response["error"]["code"], -32601);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not found"));
    }

    #[tokio::test]
    async fn test_mcp_server_initialize() {
        let tools = test_tools();
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test"}
            }
        });

        let response = handle_mcp_request(&request, &tools).await;
        assert_eq!(response["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(response["result"]["serverInfo"]["name"]
            .as_str()
            .unwrap()
            .contains("carrier"));
    }
}
