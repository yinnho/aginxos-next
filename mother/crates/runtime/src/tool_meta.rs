//! Tool metadata registry — single source of truth for per-tool display and
//! result-size metadata.
//!
//! [`tool_meta`] backs `prompt_builder::tool_category` / `tool_hint` and
//! `tool_runner::tool_max_result_chars`, which were three separate string-match
//! tables that had drifted out of sync (different tool sets, different coverage).
//! Adding or reclassifying a tool now means editing one match arm here.
//!
//! NOTE: This is the *display/grouping* taxonomy only. It is deliberately NOT
//! the permission taxonomy (`carrier_types::tool::PermissionLevel`) or the toolset
//! grouping (`kernel::tool_to_toolset`) — those are different semantic
//! dimensions and must not be merged into this table.

/// Display category for grouping tools in the system prompt's tools section.
///
/// `label()` returns the exact strings the old `tool_category` returned, so the
/// rendered system prompt is byte-identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    Files,
    Web,
    Browser,
    Shell,
    Agents,
    Media,
    Scheduling,
    Processes,
    Mcp,
    Flows,
    Other,
}

impl ToolCategory {
    /// Display label, byte-identical to the legacy `tool_category` strings.
    pub fn label(self) -> &'static str {
        match self {
            ToolCategory::Files => "Files",
            ToolCategory::Web => "Web",
            ToolCategory::Browser => "Browser",
            ToolCategory::Shell => "Shell",
            ToolCategory::Agents => "Agents",
            ToolCategory::Media => "Media",
            ToolCategory::Scheduling => "Scheduling",
            ToolCategory::Processes => "Processes",
            ToolCategory::Mcp => "MCP",
            ToolCategory::Flows => "Flows",
            ToolCategory::Other => "Other",
        }
    }
}

/// Per-tool metadata: display category, one-line hint, and per-tool result cap.
///
/// `hint == ""` means no hint; `max_result_chars == None` means no per-tool cap
/// (dynamic context-budget truncation still applies).
pub struct ToolMeta {
    pub category: ToolCategory,
    pub hint: &'static str,
    pub max_result_chars: Option<usize>,
}

/// Look up a tool's metadata by name.
///
/// The match covers the union of the three legacy tables. Any name not listed
/// explicitly falls through to [`fallback_category`] with an empty hint and no
/// cap. Some mappings are intentionally "non-obvious" to preserve legacy
/// behavior byte-for-byte — e.g. `image_analyze`/`media_*`/`knowledge_read`/
/// `sqlite_query` categorize as `Other` (they were never in the explicit
/// category lists) but carry a result cap; `flow_*` tools categorize as `Flows`
/// via the prefix rule. Do not "fix" these without a deliberate behavior change.
pub fn tool_meta(name: &str) -> ToolMeta {
    use ToolCategory::*;
    let (category, hint, max_result_chars) = match name {
        // Files
        "file_read" => (Files, "read file contents", Some(50_000)),
        "file_write" => (Files, "create or overwrite a file", None),
        "file_list" => (Files, "list directory contents", None),
        "file_delete" => (Files, "delete a file", None),
        "file_move" => (Files, "move or rename a file", None),
        "file_copy" => (Files, "copy a file", None),
        "file_search" => (Files, "search files by name pattern", None),

        // Web
        "web_fetch" => (
            Web,
            "fetch a URL and get its content as markdown",
            Some(20_000),
        ),

        // Browser
        "browser_navigate" => (
            Browser,
            "open a URL in the browser and return content",
            None,
        ),
        "browser_click" => (Browser, "click an element on the page via JS", None),
        "browser_type" => (Browser, "type text into an input field via JS", None),
        "browser_screenshot" => (
            Browser,
            "capture a screenshot (not supported — use browser_navigate)",
            None,
        ),
        "browser_read_page" => (Browser, "extract page content as text/markdown", None),
        "browser_close" => (
            Browser,
            "close the browser session (no-op for AginxBrowser)",
            None,
        ),
        "browser_scroll" => (Browser, "scroll the page via JS", None),
        "browser_wait" => (Browser, "wait for an element or condition via JS", None),
        "browser_evaluate" => (Browser, "run arbitrary JavaScript on the page", None),
        "browser_select" => (Browser, "select a dropdown option via JS", None),
        "browser_back" => (
            Browser,
            "go back to the previous page (not supported — use browser_navigate)",
            None,
        ),

        // Shell
        "shell_exec" => (Shell, "execute a shell command", Some(10_000)),
        "shell_background" => (Shell, "run a command in the background", None),

        // Agents
        "agent_send" => (Agents, "send a message to another agent", None),
        "agent_spawn" => (Agents, "create a new agent", None),
        "agent_list" => (Agents, "list running agents", None),
        "agent_kill" => (Agents, "terminate an agent", None),

        // Media
        "image_describe" => (Media, "describe an image", None),
        "image_generate" => (Media, "generate an image from a prompt", None),
        "audio_transcribe" => (Media, "transcribe audio to text", None),
        "tts_speak" => (Media, "convert text to speech", None),

        // Scheduling
        "cron_create" => (Scheduling, "schedule a recurring task", None),
        "cron_list" => (Scheduling, "list scheduled tasks", None),
        "cron_delete" => (Scheduling, "remove a scheduled task", None),

        // Processes
        "process_start" => (
            Processes,
            "start a long-running process (REPL, server)",
            None,
        ),
        "process_poll" => (Processes, "read stdout/stderr from a running process", None),
        "process_write" => (Processes, "write to a process's stdin", None),
        "process_kill" => (Processes, "terminate a running process", None),
        "process_list" => (Processes, "list active processes", None),

        // Evolution (self-improvement) — hint-only legacy entries. knowledge_*
        // and session_summarize were never in the explicit category lists, so
        // they fall to `Other`; flow_* hit the `flow_` prefix → `Flows`.
        "knowledge_extract" => (
            Other,
            "extract and save new knowledge from conversation",
            None,
        ),
        "knowledge_index" => (Other, "rebuild knowledge index (MEMORY.md)", None),
        "flow_create" => (Flows, "create a new flow", None),
        "flow_update" => (Flows, "update an existing flow", None),
        "flow_load" => (Flows, "load full flow content", None),
        "session_summarize" => (Other, "save a conversation summary", None),

        // Result-cap-only entries — no explicit category (→ Other), no hint.
        "knowledge_read" => (Other, "", Some(30_000)),
        "sqlite_query" => (Other, "", Some(30_000)),
        "image_analyze" => (Other, "", Some(10_000)),
        "media_describe" => (Other, "", Some(10_000)),
        "media_transcribe" => (Other, "", Some(10_000)),

        _ => {
            return ToolMeta {
                category: fallback_category(name),
                hint: "",
                max_result_chars: None,
            }
        }
    };
    ToolMeta {
        category,
        hint,
        max_result_chars,
    }
}

/// Category for names not explicitly listed in [`tool_meta`]: prefix rules
/// (`mcp_` → MCP, `flow_` → Flows), else `Other`. Matches the legacy fallthrough.
fn fallback_category(name: &str) -> ToolCategory {
    if name.starts_with("mcp_") {
        ToolCategory::Mcp
    } else if name.starts_with("flow_") {
        ToolCategory::Flows
    } else {
        ToolCategory::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(name: &str) -> (&'static str, &'static str, Option<usize>) {
        let m = tool_meta(name);
        (m.category.label(), m.hint, m.max_result_chars)
    }

    #[test]
    fn union_table_matches_legacy_three_tables() {
        // Category + hint + cap agree with the old tool_category/tool_hint/
        // tool_max_result_chars for representative names across every bucket.
        assert_eq!(
            meta("file_read"),
            ("Files", "read file contents", Some(50_000))
        );
        assert_eq!(
            meta("web_fetch"),
            (
                "Web",
                "fetch a URL and get its content as markdown",
                Some(20_000)
            )
        );
        assert_eq!(
            meta("shell_exec"),
            ("Shell", "execute a shell command", Some(10_000))
        );
        assert_eq!(
            meta("browser_navigate"),
            (
                "Browser",
                "open a URL in the browser and return content",
                None
            )
        );
        assert_eq!(
            meta("agent_send"),
            ("Agents", "send a message to another agent", None)
        );
        assert_eq!(
            meta("image_generate"),
            ("Media", "generate an image from a prompt", None)
        );
        assert_eq!(
            meta("cron_create"),
            ("Scheduling", "schedule a recurring task", None)
        );
        assert_eq!(
            meta("process_start"),
            (
                "Processes",
                "start a long-running process (REPL, server)",
                None
            )
        );
    }

    #[test]
    fn non_obvious_legacy_mappings_preserved() {
        // Cap-carrying tools that were NEVER in the explicit category lists must
        // stay `Other` (not Media/etc.) to keep the system prompt byte-identical.
        assert_eq!(meta("image_analyze"), ("Other", "", Some(10_000)));
        assert_eq!(meta("media_describe"), ("Other", "", Some(10_000)));
        assert_eq!(meta("media_transcribe"), ("Other", "", Some(10_000)));
        assert_eq!(meta("knowledge_read"), ("Other", "", Some(30_000)));
        assert_eq!(meta("sqlite_query"), ("Other", "", Some(30_000)));

        // Hint-only evolution tools: knowledge_* / session_summarize → Other,
        // flow_* → Flows (prefix rule made explicit).
        assert_eq!(
            meta("knowledge_extract"),
            (
                "Other",
                "extract and save new knowledge from conversation",
                None
            )
        );
        assert_eq!(
            meta("session_summarize"),
            ("Other", "save a conversation summary", None)
        );
        assert_eq!(meta("flow_create"), ("Flows", "create a new flow", None));
        assert_eq!(meta("flow_load"), ("Flows", "load full flow content", None));
    }

    #[test]
    fn prefix_fallback_and_unknown() {
        assert_eq!(meta("mcp_github_search"), ("MCP", "", None));
        assert_eq!(meta("flow_some_custom"), ("Flows", "", None));
        assert_eq!(meta("totally_unknown_tool"), ("Other", "", None));
    }
}
