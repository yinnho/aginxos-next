//! Local MCP server installation registry.
//!
//! Manages `~/.aginx/carrier/mcp-servers/installed.json` and the per-server
//! directory structure for installed MCP servers. Generates a single config
//! snippet file that can be included from config.toml.

use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use carrier_types::mcp_manifest::{McpInstalledRecord, McpServerManifest};

/// Base directory for MCP server data: `~/.aginx/carrier/mcp-servers/`.
pub fn mcp_base_dir() -> PathBuf {
    carrier_types::config::home_dir().join("mcp-servers")
}

/// Config snippets directory: `~/.aginx/carrier/mcp-servers.d/`.
pub fn mcp_config_snippets_dir() -> PathBuf {
    carrier_types::config::home_dir().join("mcp-servers.d")
}

/// Path to the unified MCP config snippet (included by config.toml).
pub fn mcp_config_path() -> PathBuf {
    mcp_config_snippets_dir().join("mcp-servers.toml")
}

/// Path to the installation registry file.
pub fn installed_json_path() -> PathBuf {
    mcp_base_dir().join("installed.json")
}

/// Read the installation registry. Returns empty list if file doesn't exist.
pub fn read_installed() -> Vec<McpInstalledRecord> {
    let path = installed_json_path();
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(records) => records,
            Err(e) => {
                warn!("Failed to parse installed.json: {e}");
                Vec::new()
            }
        },
        Err(e) => {
            warn!("Failed to read installed.json: {e}");
            Vec::new()
        }
    }
}

/// Write the installation registry.
fn write_installed(records: &[McpInstalledRecord]) -> Result<(), String> {
    let dir = mcp_base_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create mcp-servers dir: {e}"))?;
    let content =
        serde_json::to_string_pretty(records).map_err(|e| format!("Failed to serialize: {e}"))?;
    std::fs::write(installed_json_path(), content)
        .map_err(|e| format!("Failed to write installed.json: {e}"))
}

/// Check if an MCP server is installed by name.
pub fn is_installed(name: &str) -> bool {
    read_installed().iter().any(|r| r.name == name && r.enabled)
}

/// Install an MCP server from a manifest.
///
/// 1. Creates `~/.aginx/carrier/mcp-servers/{name}/` directory
/// 2. Copies the manifest as `mcp.json`
/// 3. Regenerates the unified config snippet
/// 4. Updates `installed.json`
pub fn install(manifest: &McpServerManifest, manifest_src: &Path) -> Result<(), String> {
    let name = &manifest.name;
    let server_dir = mcp_base_dir().join(name);
    std::fs::create_dir_all(&server_dir)
        .map_err(|e| format!("Failed to create server dir: {e}"))?;

    // Copy manifest
    let dest = server_dir.join("mcp.json");
    std::fs::copy(manifest_src, &dest).map_err(|e| format!("Failed to copy manifest: {e}"))?;

    // Update installed.json
    let mut records = read_installed();
    records.retain(|r| r.name != *name);
    records.push(McpInstalledRecord {
        name: name.clone(),
        version: manifest.version.clone(),
        transport_type: manifest.transport_type.clone(),
        installed_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        manifest_path: dest.to_string_lossy().to_string(),
        enabled: true,
    });
    write_installed(&records)?;

    // Regenerate unified config snippet
    regenerate_config_snippet()?;

    info!(server = %name, version = %manifest.version, "MCP server installed");
    Ok(())
}

/// Uninstall an MCP server.
///
/// 1. Removes server directory
/// 2. Updates installed.json
/// 3. Regenerates the unified config snippet
pub fn uninstall(name: &str) -> Result<(), String> {
    // Remove server directory
    let server_dir = mcp_base_dir().join(name);
    if server_dir.exists() {
        std::fs::remove_dir_all(&server_dir)
            .map_err(|e| format!("Failed to remove server dir: {e}"))?;
    }

    // Update installed.json
    let mut records = read_installed();
    records.retain(|r| r.name != name);
    write_installed(&records)?;

    // Regenerate unified config snippet
    regenerate_config_snippet()?;

    info!(server = %name, "MCP server uninstalled");
    Ok(())
}

/// Regenerate the unified `mcp-servers.d/mcp-servers.toml` from all installed
/// and enabled MCP servers. This file is included by config.toml.
fn regenerate_config_snippet() -> Result<(), String> {
    let dir = mcp_config_snippets_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create snippets dir: {e}"))?;

    let records = read_installed();
    let enabled: Vec<&McpInstalledRecord> = records.iter().filter(|r| r.enabled).collect();

    if enabled.is_empty() {
        // Remove the file if no servers installed
        let path = mcp_config_path();
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
        return Ok(());
    }

    let mut toml =
        String::from("# Auto-generated by carrier mcp install — do not edit manually.\n\n");

    for record in &enabled {
        let manifest = match read_manifest(&record.name) {
            Some(m) => m,
            None => {
                warn!(server = %record.name, "Manifest not found, skipping");
                continue;
            }
        };
        toml.push_str(&format!(
            "# --- {} ({}) ---\n",
            manifest.display_name, record.name
        ));
        match &manifest.runtime.transport {
            carrier_types::mcp_manifest::McpRuntimeTransport::Sse { url } => {
                toml.push_str(&format!(
                    "[[mcp_servers]]\n\
                     name = \"{name}\"\n\
                     timeout_secs = {timeout}\n\
                     \n\
                     [mcp_servers.transport]\n\
                     type = \"sse\"\n\
                     url = \"{url}\"\n\n",
                    name = manifest.name,
                    timeout = manifest.runtime.timeout_secs,
                    url = url,
                ));
            }
            carrier_types::mcp_manifest::McpRuntimeTransport::Stdio { command, args } => {
                let args_toml = if args.is_empty() {
                    String::new()
                } else {
                    let items: Vec<toml::Value> = args
                        .iter()
                        .map(|s| toml::Value::String(s.clone()))
                        .collect();
                    let arr = toml::Value::Array(items);
                    format!("\nargs = {}", toml::to_string(&arr).unwrap_or_default())
                };
                toml.push_str(&format!(
                    "[[mcp_servers]]\n\
                     name = \"{name}\"\n\
                     timeout_secs = {timeout}\n\
                     \n\
                     [mcp_servers.transport]\n\
                     type = \"stdio\"\n\
                     command = \"{command}\"{args}\n\n",
                    name = manifest.name,
                    timeout = manifest.runtime.timeout_secs,
                    command = command,
                    args = args_toml,
                ));
            }
        }
    }

    let path = mcp_config_path();
    std::fs::write(&path, toml).map_err(|e| format!("Failed to write config snippet: {e}"))?;

    debug!(path = %path.display(), "Unified config snippet regenerated");
    Ok(())
}

/// Ensure config.toml has the include entry for MCP servers.
/// Call this once during init or first install.
pub fn ensure_config_include() -> Result<(), String> {
    let config_path = carrier_types::config::home_dir().join("config.toml");

    if !config_path.exists() {
        return Err("config.toml not found".to_string());
    }

    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.toml: {e}"))?;

    let include_entry = "mcp-servers.d/mcp-servers.toml";

    // Check if already present
    if content.contains(include_entry) {
        return Ok(());
    }

    // Append include at the end
    let new_content = format!(
        "{content}\n\
         # MCP server configs (auto-managed by `carrier mcp install`)\n\
         include = [\"{include_entry}\"]\n"
    );

    std::fs::write(&config_path, new_content)
        .map_err(|e| format!("Failed to update config.toml: {e}"))?;

    info!("Added MCP include to config.toml");
    Ok(())
}

/// List all installed MCP servers.
pub fn list_installed() -> Vec<McpInstalledRecord> {
    read_installed()
}

/// Read a manifest from the local installed copy.
pub fn read_manifest(name: &str) -> Option<McpServerManifest> {
    let path = mcp_base_dir().join(name).join("mcp.json");
    if !path.exists() {
        return None;
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}
