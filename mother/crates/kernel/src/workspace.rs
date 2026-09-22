//! Workspace management — directory structure, identity files, and daily logs.
//!
//! All functions are pure helpers: they take `&Path` and operate only on the
//! filesystem. No kernel state is accessed.

use crate::error::{KernelError, KernelResult};
use std::io::Write;
use std::path::Path;
use carrier_types::agent::AgentManifest;

/// Create workspace directory structure for an agent.
pub fn ensure_workspace(workspace: &Path) -> KernelResult<()> {
    for subdir in &["knowledge", "sessions", "flows", "logs", "history"] {
        std::fs::create_dir_all(workspace.join(subdir)).map_err(|e| {
            KernelError::Carrier(carrier_types::error::CarrierError::Internal(format!(
                "Failed to create workspace dir {}/{subdir}: {e}",
                workspace.display()
            )))
        })?;
    }
    // Write agent metadata file (best-effort)
    let meta = serde_json::json!({
        "created_at": chrono::Utc::now().to_rfc3339(),
        "workspace": workspace.display().to_string(),
    });
    let _ = std::fs::write(
        workspace.join("AGENT.json"),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    );
    Ok(())
}

/// Generate workspace identity files for an agent (SOUL.md, USER.md, TOOLS.md, MEMORY.md).
/// Uses `create_new` to never overwrite existing files (preserves user edits).
pub fn generate_identity_files(workspace: &Path, manifest: &AgentManifest) {
    use std::fs::OpenOptions;

    let soul_content = format!(
        "# Soul\n\
         You are {}. {}\n\
         Be genuinely helpful. Have opinions. Be resourceful before asking.\n\
         Treat user data with respect \u{2014} you are a guest in their life.\n",
        manifest.name,
        if manifest.description.is_empty() {
            "You are a helpful AI agent."
        } else {
            &manifest.description
        }
    );

    let user_content = "# User\n\
         <!-- Updated by the agent as it learns about the user -->\n\
         - Name:\n\
         - Timezone:\n\
         - Preferences:\n";

    let tools_content = "# Tools & Environment\n\
         <!-- Agent-specific environment notes (not synced) -->\n";

    let memory_content = "# Long-Term Memory\n\
         <!-- Curated knowledge the agent preserves across sessions -->\n";

    let agents_content = "# Agent Behavioral Guidelines\n\n\
         ## Core Principles\n\
         - Act first, narrate second. Use tools to accomplish tasks rather than describing what you'd do.\n\
         - Batch tool calls when possible \u{2014} don't output reasoning between each call.\n\
         - When a task is ambiguous, ask ONE clarifying question, not five.\n\
         - Store important context proactively using kv_set.\n\
         - Search stored context with kv_get before asking the user for information they may have given before.\n\n\
         ## Drawer Memory\n\
         You have a drawer memory (kv_get/kv_set/kv_list) for storing user-specific information.\n\
         - Before asking for info the user may have provided before, check the drawer: kv_get(\"category.name\")\n\
         - After learning important info (accounts, preferences, phone, decisions), store it: kv_set(\"category.name\", value)\n\
         - Drawer keys follow: category.specific_name (e.g. entity.wechat_accounts, preference.theme, profile.phone_numbers)\n\
         - Categories: profile (personal info), preference (likes/settings), entity (accounts/projects/orgs), fact (rules/constraints), event (decisions/timelines)\n\
         - Values are always arrays — use kv_list() to see all stored keys, kv_list(\"entity.\") to filter by prefix\n\
         - When you discover missing info during a task, ask the user, and after success store it + update the relevant flow.\n\n\
         ## Tool Usage Protocols\n\
         - file_read BEFORE file_write \u{2014} always understand what exists.\n\
         - web_fetch for specific URLs.\n\
         - browser_* for interactive sites that need clicks/JS evaluation.\n\
         - AginxBrowser powers browser tools: fast, lightweight, no screenshots.\n\
         - shell_exec: explain destructive commands before running.\n\n\
         ## Response Style\n\
         - Lead with the answer or result, not process narration.\n\
         - Keep responses concise unless the user asks for detail.\n\
         - Use formatting (headers, lists, code blocks) for readability.\n\
         - If a task fails, explain what went wrong and suggest alternatives.\n";

    let bootstrap_content = format!(
        "# First-Run Bootstrap\n\n\
         On your FIRST conversation with a new user, follow this protocol:\n\n\
         1. **Greet** \u{2014} Introduce yourself as {name} with a one-line summary of your specialty.\n\
         2. **Discover** \u{2014} Ask the user's name and one key preference relevant to your domain.\n\
         3. **Store** \u{2014} Use kv_set to save: user_name, their preference, and today's date as first_interaction.\n\
         4. **Orient** \u{2014} Briefly explain what you can help with (2-3 bullet points, not a wall of text).\n\
         5. **Serve** \u{2014} If the user included a request in their first message, handle it immediately after steps 1-3.\n\n\
         After bootstrap, this protocol is complete. Focus entirely on the user's needs.\n",
        name = manifest.name
    );

    let identity_content = format!(
        "---\n\
         name: {name}\n\
         archetype: assistant\n\
         vibe: helpful\n\
         emoji:\n\
         avatar_url:\n\
         greeting_style: warm\n\
         color:\n\
         ---\n\
         # Identity\n\
         <!-- Visual identity and personality at a glance. Edit these fields freely. -->\n",
        name = manifest.name
    );

    let files: &[(&str, &str)] = &[
        ("SOUL.md", &soul_content),
        ("USER.md", user_content),
        ("TOOLS.md", tools_content),
        ("MEMORY.md", memory_content),
        ("AGENTS.md", agents_content),
        ("BOOTSTRAP.md", &bootstrap_content),
        ("IDENTITY.md", &identity_content),
    ];

    // Conditionally generate HEARTBEAT.md for autonomous agents
    let heartbeat_content = if manifest.autonomous.is_some() {
        Some(
            "# Heartbeat Checklist\n\
             <!-- Proactive reminders to check during heartbeat cycles -->\n\n\
             ## Every Heartbeat\n\
             - [ ] Check for pending tasks or messages\n\
             - [ ] Review memory for stale items\n\n\
             ## Daily\n\
             - [ ] Summarize today's activity for the user\n\n\
             ## Weekly\n\
             - [ ] Archive old sessions and clean up memory\n"
                .to_string(),
        )
    } else {
        None
    };

    for (filename, content) in files {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(workspace.join(filename))
        {
            Ok(mut f) => {
                let _ = f.write_all(content.as_bytes());
            }
            Err(_) => {
                // File already exists — preserve user edits
            }
        }
    }

    // Write HEARTBEAT.md for autonomous agents
    if let Some(ref hb) = heartbeat_content {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(workspace.join("HEARTBEAT.md"))
        {
            Ok(mut f) => {
                let _ = f.write_all(hb.as_bytes());
            }
            Err(_) => {
                // File already exists — preserve user edits
            }
        }
    }
}

/// Append an assistant response summary to the daily memory log (best-effort, append-only).
/// Caps daily log at 1MB to prevent unbounded growth.
/// When sender_id is present, writes to per-sender memory directory.
pub fn append_daily_memory_log(
    home_dir: &Path,
    agent_name: &str,
    response: &str,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
) {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return;
    }
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let log_path = if let Some(oid) = owner_id.or(sender_id) {
        carrier_types::config::sender_data_dir(home_dir, oid, agent_name, sender_id)
            .join("memory")
            .join(format!("{today}.md"))
    } else {
        // No sender context — write to workspace logs
        home_dir.join("logs").join(format!("{today}.md"))
    };
    // Security: cap total daily log to 1MB
    if let Ok(metadata) = std::fs::metadata(&log_path) {
        if metadata.len() > 1_048_576 {
            return;
        }
    }
    // Truncate long responses for the log (UTF-8 safe)
    let summary = carrier_types::truncate_str(trimmed, 500);
    let timestamp = chrono::Utc::now().format("%H:%M:%S").to_string();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let _ = writeln!(f, "\n## {timestamp}\n{summary}\n");
    }
}
