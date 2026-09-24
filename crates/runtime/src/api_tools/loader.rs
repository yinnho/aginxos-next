//! API tools loader — reads api_tools.toml from disk and returns parsed configs.

use std::path::Path;
use carrier_types::api_tool::{ApiToolDef, ApiToolsConfig};

/// Load api_tools.toml from a path. Returns empty vec if file doesn't exist.
pub fn load_api_tools_file(path: &Path) -> Vec<ApiToolDef> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    match toml::from_str::<ApiToolsConfig>(&content) {
        Ok(config) => {
            tracing::info!(path = %path.display(), count = config.tool.len(), "Loaded api_tools.toml");
            config.tool
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "Failed to parse api_tools.toml");
            Vec::new()
        }
    }
}

/// Load all api_tools: global + per-workspace. Workspace tools override global by name.
pub fn load_all_api_tools(home_dir: &Path, workspace_dir: Option<&Path>) -> Vec<ApiToolDef> {
    let global_path = home_dir.join("api_tools.toml");
    let mut tools = load_api_tools_file(&global_path);

    // Merge workspace-level tools (override by name)
    if let Some(ws) = workspace_dir {
        let ws_path = ws.join("api_tools.toml");
        let ws_tools = load_api_tools_file(&ws_path);

        let global_names: std::collections::HashSet<String> =
            tools.iter().map(|t| t.name.clone()).collect();

        for tool in ws_tools {
            if global_names.contains(&tool.name) {
                // Override: remove global version, add workspace version
                tools.retain(|t| t.name != tool.name);
                tracing::info!(tool = %tool.name, "Workspace api_tool overrides global");
            }
            tools.push(tool);
        }
    }

    tools
}
