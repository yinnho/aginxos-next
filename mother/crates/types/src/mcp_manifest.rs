//! MCP Server package manifest (.mcp.json).
//!
//! Defines the format for distributable MCP server descriptors that can be
//! installed, upgraded, and managed independently from the core system.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level MCP server package manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerManifest {
    /// Format version ("1").
    pub format_version: String,
    /// Unique server name (lowercase, hyphens allowed).
    pub name: String,
    /// Human-readable display name.
    #[serde(default)]
    pub display_name: String,
    /// One-line description.
    #[serde(default)]
    pub description: String,
    /// Author name or organization.
    #[serde(default)]
    pub author: String,
    /// Category for Hub browsing (e.g., "资讯", "开发", "搜索").
    #[serde(default)]
    pub category: String,
    /// Icon (emoji or URL).
    #[serde(default)]
    pub icon: String,
    /// Tags for search and filtering.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Transport type: "docker", "npx", or "cloud".
    pub transport_type: String,
    /// Installation instructions (varies by transport_type).
    pub install: McpInstallConfig,
    /// Runtime connection details.
    pub runtime: McpRuntimeConfig,
    /// Preview of available tools (for display before install).
    #[serde(default)]
    pub tools_preview: Vec<McpToolPreview>,
    /// Homepage URL.
    #[serde(default)]
    pub homepage_url: String,
    /// License identifier (e.g., "MIT").
    #[serde(default)]
    pub license: String,
    /// Version string (semver preferred).
    #[serde(default)]
    pub version: String,
}

/// Installation configuration — varies by transport type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpInstallConfig {
    /// Docker-based: pull image, generate compose file.
    Docker(McpDockerInstall),
    /// NPX-based: spawn stdio subprocess.
    Npx(McpNpxInstall),
    /// Cloud/remote: connect to URL with API key.
    Cloud(McpCloudInstall),
}

/// Docker installation details.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpDockerInstall {
    /// Docker image (e.g., "wantcat/trendradar:latest").
    pub image: String,
    /// Container name prefix (default: "carrier-mcp-{name}").
    pub container_name: String,
    /// MCP JSON-RPC port inside container.
    pub mcp_port: u16,
    /// Optional web UI port.
    pub web_port: Option<u16>,
    /// Volume mounts.
    pub volumes: Vec<McpVolume>,
    /// Environment variables.
    pub environment: HashMap<String, String>,
    /// Restart policy (default: "unless-stopped").
    pub restart_policy: String,
}

impl Default for McpDockerInstall {
    fn default() -> Self {
        Self {
            image: String::new(),
            container_name: String::new(),
            mcp_port: 0,
            web_port: None,
            volumes: Vec::new(),
            environment: HashMap::new(),
            restart_policy: "unless-stopped".to_string(),
        }
    }
}

/// NPX installation details.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpNpxInstall {
    /// NPM package name (e.g., "@anthropic/mcp-server-github").
    pub package: String,
    /// Arguments passed to the package.
    pub args: Vec<String>,
}

/// Cloud/remote installation details.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpCloudInstall {
    /// MCP endpoint URL.
    pub url: String,
    /// Required environment variable names (e.g., ["API_KEY"]).
    pub env_required: Vec<String>,
    /// Optional environment variable names.
    pub env_optional: Vec<String>,
}

/// Volume mount definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpVolume {
    /// Host path or named volume.
    pub host: String,
    /// Container path.
    pub container: String,
}

/// Runtime connection details.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpRuntimeConfig {
    /// Transport details for connecting at runtime.
    pub transport: McpRuntimeTransport,
    /// Default timeout in seconds.
    pub timeout_secs: u64,
}

impl Default for McpRuntimeConfig {
    fn default() -> Self {
        Self {
            transport: McpRuntimeTransport::default(),
            timeout_secs: 60,
        }
    }
}

/// Runtime transport configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpRuntimeTransport {
    /// HTTP POST (JSON-RPC over HTTP).
    Sse { url: String },
    /// Subprocess (JSON-RPC over stdin/stdout).
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl Default for McpRuntimeTransport {
    fn default() -> Self {
        Self::Sse { url: String::new() }
    }
}

/// Tool preview entry (for display before install).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolPreview {
    /// Tool name.
    pub name: String,
    /// Brief description.
    pub description: String,
}

/// Local installation record (stored in installed.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpInstalledRecord {
    /// Server name.
    pub name: String,
    /// Installed version.
    pub version: String,
    /// Transport type.
    pub transport_type: String,
    /// Install timestamp (unix epoch).
    pub installed_at: u64,
    /// Path to the .mcp.json descriptor.
    pub manifest_path: String,
    /// Whether the server is currently enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}
