//! Build AgentManifest from an extracted v3 workspace.
//!
//! Replaces the old `convert_to_manifest(CloneData)` — instead of converting
//! from an in-memory struct, this reads the workspace files directly.

use std::path::Path;

use anyhow::Result;
use carrier_types::agent::{
    AgentManifest, CloneSource, ManifestCapabilities, ModelConfig, ResourceQuota,
};
use tracing::debug;

use crate::loader::{parse_template_manifest_lenient, TemplateManifest};

/// Build an `AgentManifest` from an extracted v3 workspace directory.
///
/// Reads `template.json`, `profile.md`, and scans `flows/` and `knowledge/`
/// to construct the manifest needed for `spawn_agent`.
pub fn build_manifest_from_workspace(
    workspace: &Path,
    name: &str,
    hub_template_id: Option<String>,
) -> Result<AgentManifest> {
    // 1. Read template.json
    let template = read_template_json(workspace);

    // 2. Read profile.md for description
    let description = read_profile_description(workspace);

    // 3. Scan flows/ directory — collect names and tools
    let scan_result = scan_flows(workspace);
    let flow_names = scan_result.names;
    let flow_tools = scan_result.tools;

    // 4. Collect knowledge file names
    let knowledge_files = collect_knowledge_files(workspace);

    // 5. Build tools list from flow-declared tools + evolution defaults
    // Core tools (file_read, flow_load, etc.) are auto-loaded at runtime and
    // should NOT be listed in capabilities.tools. MCP tools (mcp_*) are loaded
    // separately via mcp_servers config. So we only collect non-core builtins.
    let core_tools: &[&str] = &[
        "session_summarize",
        "flow_load",
        "knowledge_read",
        "knowledge_list",
        "file_read",
        "file_list",
        "cron_create",
        "cron_list",
        "cron_cancel",
        "memory_tree",
        "task_plan",
    ];

    let evolution_tools: &[&str] = &[
        "knowledge_add",
        "knowledge_list",
        "knowledge_read",
        "knowledge_lint",
        "knowledge_extract",
        "knowledge_index",
        "flow_create",
        "flow_update",
        "flow_load",
        "session_summarize",
        "file_read",
        "file_write",
        "file_list",
        "user_profile",
    ];

    let mut tools: Vec<String> = Vec::new();

    // Add evolution defaults (needed for self-evolution)
    for tool in evolution_tools {
        let t = tool.to_string();
        if !tools.contains(&t) {
            tools.push(t);
        }
    }

    // Add tools declared in flows (non-core builtins only, skip MCP tools)
    for tool in &flow_tools {
        if tool.starts_with("mcp_") {
            continue;
        }
        if core_tools.contains(&tool.as_str()) {
            continue;
        }
        if !tools.contains(tool) {
            tools.push(tool.clone());
        }
    }

    tools.sort();
    tools.dedup();

    // 5.5 Derive mcp_servers: merge template.json + flow-declared MCP tool prefixes
    let mut mcp_servers: Vec<String> = template
        .as_ref()
        .map(|t| t.mcp_servers.clone())
        .unwrap_or_default();
    for tool in &flow_tools {
        if let Some(server) = extract_mcp_server(tool) {
            if !mcp_servers.contains(&server) {
                mcp_servers.push(server);
            }
        }
    }
    mcp_servers.sort();

    // 6. Build CloneSource
    let clone_source = CloneSource {
        template_name: name.to_string(),
        template_author: template
            .as_ref()
            .map(|t| t.author.clone())
            .unwrap_or_default(),
        installed_at: chrono::Utc::now().timestamp().to_string(),
        agx_version: template
            .as_ref()
            .map(|t| t.version.clone())
            .unwrap_or_else(|| "1".to_string()),
        hub_template_id,
    };

    // 7. Assemble manifest
    let manifest = AgentManifest {
        name: name.to_string(),
        display_name: template
            .as_ref()
            .map(|t| t.display_name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_default(),
        version: template
            .as_ref()
            .map(|t| t.version.clone())
            .unwrap_or_else(|| "0.1.0".to_string()),
        description: if description.is_empty() {
            template
                .as_ref()
                .map(|t| t.description.clone())
                .unwrap_or_default()
        } else {
            description
        },
        author: template
            .as_ref()
            .map(|t| t.author.clone())
            .unwrap_or_default(),
        module: "builtin:chat".to_string(),
        schedule: carrier_types::agent::ScheduleMode::default(),
        model: ModelConfig {
            max_tokens: 8192,
            temperature: 0.7,
            system_prompt: String::new(), // clone prompts built dynamically from workspace
            modality: "chat".to_string(),
        },
        resources: ResourceQuota::default(),
        priority: carrier_types::agent::Priority::default(),
        capabilities: ManifestCapabilities {
            tools,
            network: vec![],
            memory_read: vec!["self.*".to_string()],
            memory_write: vec!["self.*".to_string()],
            ..Default::default()
        },
        flows: flow_names,
        tags: template
            .as_ref()
            .map(|t| t.tags.clone())
            .unwrap_or_default(),
        clone_source: Some(clone_source),
        knowledge_files,
        plugins: template
            .as_ref()
            .map(|t| t.plugins.clone())
            .unwrap_or_default(),
        mcp_servers,
        // default_flow: classifier-miss fallback flow (see AgentManifest::default_flow).
        // Read from template.json so clones can declare it in their definition layer
        // (dup-synced) rather than having to hand-edit the runtime agent.toml.
        default_flow: template.as_ref().and_then(|t| t.default_flow.clone()),
        generate_identity_files: false, // .agx already has identity files
        ..Default::default()
    };

    debug!(
        "Built manifest for '{}' from workspace: {} flows, {} knowledge files, {} tools",
        name,
        manifest.flows.len(),
        manifest.knowledge_files.len(),
        manifest.capabilities.tools.len(),
    );

    Ok(manifest)
}

fn read_template_json(workspace: &Path) -> Option<TemplateManifest> {
    let path = workspace.join("template.json");
    let content = std::fs::read_to_string(&path).ok()?;
    // Lenient fallback on struct drift — a whole-struct failure here would
    // silently drop display_name/description/plugins/mcp_servers/default_flow
    // from every installed manifest (the mcp_servers object-array shape is
    // the documented real-world drift). See parse_template_manifest_lenient.
    parse_template_manifest_lenient(&content)
}

fn read_profile_description(workspace: &Path) -> String {
    let content = match std::fs::read_to_string(workspace.join("profile.md")) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            let frontmatter = &content[3..3 + end];
            for line in frontmatter.lines() {
                let trimmed = line.trim();
                if let Some(val) = trimmed.strip_prefix("description:") {
                    return val.trim().trim_matches('"').to_string();
                }
            }
        }
    }

    String::new()
}

/// Scan workspace/flows/ to collect flow names and all declared tools.
struct FlowScanResult {
    names: Vec<String>,
    tools: Vec<String>,
}

fn scan_flows(workspace: &Path) -> FlowScanResult {
    let flows_dir = workspace.join("flows");
    if !flows_dir.is_dir() {
        return FlowScanResult {
            names: Vec::new(),
            tools: Vec::new(),
        };
    }

    let mut names = Vec::new();
    let mut tools: Vec<String> = Vec::new();

    let Ok(entries) = std::fs::read_dir(&flows_dir) else {
        return FlowScanResult { names, tools };
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        // Prefer canonical flow.md, fall back to legacy SKILL.md.
        let flow_md = path.join("flow.md");
        let flow_md = if flow_md.exists() {
            flow_md
        } else {
            path.join("SKILL.md")
        };
        if !flow_md.exists() {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&flow_md) {
            if let Some(name) = parse_flow_name(&content) {
                names.push(name);
            }
            tools.extend(parse_flow_tools(&content));
        }
    }

    tools.sort();
    tools.dedup();

    FlowScanResult { names, tools }
}

/// Parse flow frontmatter to extract name.
fn parse_flow_name(content: &str) -> Option<String> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("---")?;
    let frontmatter = &rest[..end];
    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(val) = line.strip_prefix("name:") {
            let name = val.trim().trim_matches('"').trim_matches('\'').to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Parse flow frontmatter to extract tools list (builtin + MCP).
fn parse_flow_tools(content: &str) -> Vec<String> {
    let Some(rest) = content.strip_prefix("---") else {
        return Vec::new();
    };
    let Some(end) = rest.find("---") else {
        return Vec::new();
    };
    let frontmatter = &rest[..end];
    let lines: Vec<&str> = frontmatter.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("tools:") {
            let inline = val.trim();
            // Inline array on same line: tools: ["a", "b"]
            if inline.starts_with('[') {
                return parse_yaml_string_list(inline);
            }
            // tools: followed by block list on subsequent indented lines
            let mut block = inline.to_string();
            for subsequent in &lines[i + 1..] {
                let sub = subsequent.trim();
                // Stop at next top-level key (no leading whitespace after trim, ends with ':')
                // or non-list, non-empty line that isn't indented
                if sub.is_empty() {
                    continue;
                }
                if !sub.starts_with('-')
                    && !subsequent.starts_with(' ')
                    && !subsequent.starts_with('\t')
                {
                    break;
                }
                block.push('\n');
                block.push_str(sub);
            }
            return parse_yaml_string_list(&block);
        }
    }
    Vec::new()
}

/// Parse a YAML string list like `["a", "b"]` or `- a\n- b`.
fn parse_yaml_string_list(input: &str) -> Vec<String> {
    let input = input.trim();
    if input.starts_with('[') {
        // Inline array: ["a", "b"]
        let inner = input.trim_start_matches('[').trim_end_matches(']');
        inner
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if input.starts_with('-') || input.is_empty() {
        // YAML list or empty
        input
            .lines()
            .filter_map(|l| {
                l.trim()
                    .strip_prefix('-')
                    .map(|s| s.trim().trim_matches('"').trim_matches('\'').to_string())
                    .filter(|s| !s.is_empty())
            })
            .collect()
    } else {
        Vec::new()
    }
}

/// Collect knowledge file names from workspace/knowledge/.
fn collect_knowledge_files(workspace: &Path) -> Vec<String> {
    let knowledge_dir = workspace.join("knowledge");
    if !knowledge_dir.is_dir() {
        return Vec::new();
    }

    let mut files = Vec::new();
    collect_knowledge_recursive(&knowledge_dir, &knowledge_dir, &mut files);
    files.sort();
    files
}

fn collect_knowledge_recursive(base: &Path, current: &Path, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_knowledge_recursive(base, &path, files);
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            if let Ok(rel) = path.strip_prefix(base) {
                files.push(rel.to_string_lossy().to_string());
            }
        }
    }
}

/// Extract an MCP server name from a tool name like `mcp_{server}_{tool}`.
/// Returns None if the tool name doesn't match the MCP prefix pattern.
fn extract_mcp_server(tool_name: &str) -> Option<String> {
    let rest = tool_name.strip_prefix("mcp_")?;
    let underscore_pos = rest.find('_')?;
    let server = &rest[..underscore_pos];
    if server.is_empty() {
        return None;
    }
    Some(server.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flow_tools_inline_array() {
        let md = "---\nname: test\ntools: [\"sqlite_query\", \"web_fetch\"]\n---\nbody";
        let tools = parse_flow_tools(md);
        assert_eq!(tools, vec!["sqlite_query", "web_fetch"]);
    }

    #[test]
    fn test_parse_flow_tools_block_list() {
        let md = "---\nname: test\ntools:\n  - \"sqlite_query\"\n  - \"sqlite_schema\"\n  - \"web_fetch\"\n---\nbody";
        let tools = parse_flow_tools(md);
        assert_eq!(tools, vec!["sqlite_query", "sqlite_schema", "web_fetch"]);
    }

    #[test]
    fn test_parse_flow_tools_block_list_unquoted() {
        let md = "---\nname: test\ntools:\n  - sqlite_query\n  - web_fetch\n---\nbody";
        let tools = parse_flow_tools(md);
        assert_eq!(tools, vec!["sqlite_query", "web_fetch"]);
    }

    #[test]
    fn test_parse_flow_tools_empty() {
        let md = "---\nname: test\n---\nbody";
        let tools = parse_flow_tools(md);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_parse_flow_tools_stops_at_next_key() {
        let md =
            "---\nname: test\ntools:\n  - sqlite_query\n  - web_fetch\nother_key: value\n---\nbody";
        let tools = parse_flow_tools(md);
        assert_eq!(tools, vec!["sqlite_query", "web_fetch"]);
    }
}
