//! Shared tool name mappings for Carrier.
//!
//! These mappings normalize LLM-hallucinated tool names into
//! Carrier equivalents.

/// Map an LLM-hallucinated tool name to its Carrier equivalent.
///
/// Returns `None` if the name has no known mapping (may already be
/// an Carrier tool name — check with [`is_known_carrier_tool`]).
pub fn map_tool_name(name: &str) -> Option<&'static str> {
    match name {
        // Claude-style tool names (capitalized)
        "Read" | "read" | "read_file" => Some("file_read"),
        "Write" | "write" | "write_file" => Some("file_write"),
        "Edit" | "edit" => Some("file_write"),
        "Glob" | "glob" | "list_files" => Some("file_list"),
        "Grep" | "grep" => Some("file_list"),
        "Bash" | "bash" | "exec" | "execute_command" => Some("shell_exec"),
        "WebFetch" | "fetch_url" | "web_fetch" => Some("web_fetch"),
        "WebSearch" => Some("web_search"),
        "browser_navigate" => Some("browser_navigate"),
        "sessions_send" | "agent_message" => Some("agent_send"),
        "sessions_list" | "agents_list" | "agent_list" => Some("agent_list"),
        "sessions_spawn" => Some("agent_send"),

        // KV memory aliases
        "memory_recall" | "system_kv_recall" => Some("kv_get"),
        "memory_store" | "system_kv_store" => Some("kv_set"),

        // LLM-hallucinated aliases (fs-* style names)
        "fs-read" | "fs_read" | "fsRead" | "readFile" => Some("file_read"),
        "fs-write" | "fs_write" | "fsWrite" | "writeFile" => Some("file_write"),
        "fs-list" | "fs_list" | "fsList" | "listFiles" | "list_dir" | "ls" => Some("file_list"),
        "fs-exec" | "run" | "run_command" | "runCommand" | "execute" | "shell" => {
            Some("shell_exec")
        }

        _ => None,
    }
}

/// Strip whitespace / wrapping quotes / trailing punctuation that LLMs often
/// append when emitting tool names in free text (e.g. `web_search,` from
/// `[Called web_search,]` or list-style "tools: web_search, web_fetch").
///
/// Returns a subslice of `name` (no allocation). Does not rewrite aliases —
/// use [`normalize_tool_name`] for full mapping.
pub fn sanitize_tool_name(name: &str) -> &str {
    let name = name
        .trim()
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '“' | '”' | '‘' | '’' | '«' | '»'));
    name.trim_end_matches(|c: char| {
        matches!(
            c,
            ',' | ';' | ':' | '.' | '!' | '?' | '，' | '。' | '、' | '；' | '：'
        )
    })
    .trim()
}

/// Normalize a tool name to its canonical Carrier form.
///
/// Sanitizes trailing punctuation first, then if the name is already a known
/// Carrier tool returns it as-is. Otherwise tries [`map_tool_name`].
/// Returns the (possibly sanitized) original name if no mapping is found.
pub fn normalize_tool_name(name: &str) -> &str {
    let name = sanitize_tool_name(name);
    if is_known_carrier_tool(name) {
        return name;
    }
    if let Some(mapped) = map_tool_name(name) {
        return mapped;
    }
    // Fix LLM-hallucinated double-prefixed MCP tool names.
    // e.g. "mcp__tools__mcp_wechat_oa_create_draft" → "mcp_wechat_oa_create_draft"
    // Pattern: starts with "mcp__" and contains another "mcp_" later
    if let Some(after_first) = name.strip_prefix("mcp__") {
        if let Some(idx) = after_first.find("mcp_") {
            return &after_first[idx..];
        }
    }
    name
}

/// Check if a tool name is a known Carrier built-in tool.
pub fn is_known_carrier_tool(name: &str) -> bool {
    matches!(
        name,
        "file_read"
            | "file_write"
            | "file_list"
            | "shell_exec"
            | "web_fetch"
            | "web_search"
            | "browser_navigate"
            | "agent_send"
            | "agent_list"
            | "agent_spawn"
            | "agent_kill"
            | "agent_find"
            | "task_post"
            | "task_claim"
            | "task_complete"
            | "task_list"
            | "event_publish"
            | "schedule_create"
            | "schedule_list"
            | "schedule_delete"
            | "image_analyze"
            | "location_get"
            | "memory_tree"
            | "kv_get"
            | "kv_set"
            | "kv_list"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_map_tool_name_all_mappings() {
        // Claude-style capitalized
        assert_eq!(map_tool_name("Read"), Some("file_read"));
        assert_eq!(map_tool_name("Write"), Some("file_write"));
        assert_eq!(map_tool_name("Edit"), Some("file_write"));
        assert_eq!(map_tool_name("Glob"), Some("file_list"));
        assert_eq!(map_tool_name("Grep"), Some("file_list"));
        assert_eq!(map_tool_name("Bash"), Some("shell_exec"));
        assert_eq!(map_tool_name("WebFetch"), Some("web_fetch"));
        assert_eq!(map_tool_name("WebFetch"), Some("web_fetch"));

        // Lowercase variants
        assert_eq!(map_tool_name("read"), Some("file_read"));
        assert_eq!(map_tool_name("write"), Some("file_write"));
        assert_eq!(map_tool_name("edit"), Some("file_write"));
        assert_eq!(map_tool_name("glob"), Some("file_list"));
        assert_eq!(map_tool_name("grep"), Some("file_list"));
        assert_eq!(map_tool_name("bash"), Some("shell_exec"));
        assert_eq!(map_tool_name("exec"), Some("shell_exec"));
        assert_eq!(map_tool_name("execute_command"), Some("shell_exec"));

        // Other aliases
        assert_eq!(map_tool_name("read_file"), Some("file_read"));
        assert_eq!(map_tool_name("write_file"), Some("file_write"));
        assert_eq!(map_tool_name("list_files"), Some("file_list"));
        assert_eq!(map_tool_name("fetch_url"), Some("web_fetch"));
        assert_eq!(map_tool_name("web_fetch"), Some("web_fetch"));
        assert_eq!(map_tool_name("web_fetch"), Some("web_fetch"));
        assert_eq!(map_tool_name("browser_navigate"), Some("browser_navigate"));
        assert_eq!(map_tool_name("sessions_send"), Some("agent_send"));
        assert_eq!(map_tool_name("agent_message"), Some("agent_send"));
        assert_eq!(map_tool_name("sessions_list"), Some("agent_list"));
        assert_eq!(map_tool_name("agents_list"), Some("agent_list"));
        assert_eq!(map_tool_name("agent_list"), Some("agent_list"));
        assert_eq!(map_tool_name("sessions_spawn"), Some("agent_send"));

        // LLM-hallucinated fs-* aliases
        assert_eq!(map_tool_name("fs-read"), Some("file_read"));
        assert_eq!(map_tool_name("fs_read"), Some("file_read"));
        assert_eq!(map_tool_name("fsRead"), Some("file_read"));
        assert_eq!(map_tool_name("readFile"), Some("file_read"));
        assert_eq!(map_tool_name("fs-write"), Some("file_write"));
        assert_eq!(map_tool_name("fs_write"), Some("file_write"));
        assert_eq!(map_tool_name("fsWrite"), Some("file_write"));
        assert_eq!(map_tool_name("writeFile"), Some("file_write"));
        assert_eq!(map_tool_name("fs-list"), Some("file_list"));
        assert_eq!(map_tool_name("fs_list"), Some("file_list"));
        assert_eq!(map_tool_name("fsList"), Some("file_list"));
        assert_eq!(map_tool_name("listFiles"), Some("file_list"));
        assert_eq!(map_tool_name("list_dir"), Some("file_list"));
        assert_eq!(map_tool_name("ls"), Some("file_list"));
        assert_eq!(map_tool_name("fs-exec"), Some("shell_exec"));
        assert_eq!(map_tool_name("run"), Some("shell_exec"));
        assert_eq!(map_tool_name("run_command"), Some("shell_exec"));
        assert_eq!(map_tool_name("runCommand"), Some("shell_exec"));
        assert_eq!(map_tool_name("execute"), Some("shell_exec"));
        assert_eq!(map_tool_name("shell"), Some("shell_exec"));

        // Unknown
        assert_eq!(map_tool_name("unknown_tool"), None);
        assert_eq!(map_tool_name(""), None);
    }

    #[test]
    fn test_sanitize_tool_name_strips_trailing_punct() {
        assert_eq!(sanitize_tool_name("web_search,"), "web_search");
        assert_eq!(sanitize_tool_name("web_fetch;"), "web_fetch");
        assert_eq!(sanitize_tool_name("  file_read  "), "file_read");
        assert_eq!(sanitize_tool_name("\"web_search\""), "web_search");
        assert_eq!(sanitize_tool_name("web_search，"), "web_search");
    }

    #[test]
    fn test_normalize_tool_name() {
        // Known Carrier tools pass through unchanged
        assert_eq!(normalize_tool_name("file_read"), "file_read");
        assert_eq!(normalize_tool_name("file_write"), "file_write");
        assert_eq!(normalize_tool_name("shell_exec"), "shell_exec");
        assert_eq!(normalize_tool_name("web_fetch"), "web_fetch");

        // Trailing punctuation from text tool-call recovery (e.g. "web_search,")
        assert_eq!(normalize_tool_name("web_search,"), "web_search");
        assert_eq!(normalize_tool_name("web_fetch,"), "web_fetch");
        assert_eq!(normalize_tool_name("fs-write,"), "file_write");

        // Aliases get normalized to canonical names
        assert_eq!(normalize_tool_name("fs-read"), "file_read");
        assert_eq!(normalize_tool_name("fs-write"), "file_write");
        assert_eq!(normalize_tool_name("fs-list"), "file_list");
        assert_eq!(normalize_tool_name("fs-exec"), "shell_exec");
        assert_eq!(normalize_tool_name("Read"), "file_read");
        assert_eq!(normalize_tool_name("Bash"), "shell_exec");

        // LLM-hallucinated double-prefixed MCP tool names
        assert_eq!(
            normalize_tool_name("mcp__tools__mcp_wechat_oa_create_draft"),
            "mcp_wechat_oa_create_draft"
        );
        assert_eq!(
            normalize_tool_name("mcp__xxx__mcp_server_tool"),
            "mcp_server_tool"
        );

        // Unknown names pass through unchanged (after sanitize)
    }

    #[test]
    fn test_is_known_carrier_tool() {
        // All 21 built-in tools + location_get
        let known = [
            "file_read",
            "file_write",
            "file_list",
            "shell_exec",
            "web_fetch",
            "web_fetch",
            "browser_navigate",
            "agent_send",
            "agent_list",
            "agent_spawn",
            "agent_kill",
            "agent_find",
            "task_post",
            "task_claim",
            "task_complete",
            "task_list",
            "event_publish",
            "schedule_create",
            "schedule_list",
            "schedule_delete",
            "image_analyze",
            "location_get",
        ];
        for tool in &known {
            assert!(is_known_carrier_tool(tool), "Expected {tool} to be known");
        }

        // Unknown
        assert!(!is_known_carrier_tool("unknown"));
        assert!(!is_known_carrier_tool("Read"));
        assert!(!is_known_carrier_tool("Bash"));
    }
}
