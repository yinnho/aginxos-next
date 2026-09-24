//! Tool execution context — bundles all environment references needed by `execute_tool`.

use crate::kernel_handle::KernelHandle;
use crate::llm_driver::Brain;
use crate::mcp::McpConnection;
use crate::memory_handle::MemoryHandle;
use crate::process_manager::ProcessManager;
use carrier_types::agent::SubagentConfig;
use carrier_types::config::ExecPolicy;
use carrier_types::tool::PermissionLevel;
use dashmap::DashMap;
use std::path::Path;
use std::sync::Arc;

/// Environment context passed to every tool execution.
///
/// Groups the optional references that `execute_tool` needs beyond the
/// per-call `(tool_use_id, tool_name, input)` triple.
///
/// All fields are `Option<&T>` — inherently `Copy` — so the struct derives `Copy`
/// and can be unpacked in one line at the top of `execute_tool`.
#[derive(Copy, Clone)]
pub struct ToolContext<'a> {
    pub kernel: Option<&'a Arc<dyn KernelHandle>>,
    pub memory: Option<&'a Arc<dyn MemoryHandle>>,
    pub caller_agent_id: Option<&'a str>,
    pub mcp_connections: Option<&'a DashMap<String, McpConnection>>,
    pub allowed_env_vars: Option<&'a [String]>,
    pub workspace_root: Option<&'a Path>,
    pub brain: Option<&'a Arc<dyn Brain>>,
    pub exec_policy: Option<&'a ExecPolicy>,
    pub cli_exec_config: Option<&'a carrier_types::config::CliExecConfig>,
    pub process_manager: Option<&'a ProcessManager>,
    pub sender_id: Option<&'a str>,
    pub owner_id: Option<&'a str>,
    pub home_dir: Option<&'a Path>,
    pub agent_name: Option<&'a str>,
    pub subagent_configs: Option<&'a [SubagentConfig]>,
    pub channel_type: Option<&'a str>,
    pub max_tool_level: PermissionLevel,
    pub is_clone_admin: bool,
    /// Public base URL (e.g. `https://file.yinnho.cn`) for `view_url` on file outputs.
    pub external_url: Option<&'a str>,
    /// Tools elevated for this turn by a shared system flow (`privilege: system`).
    /// Admin-gated tools in this list may run without clone-admin for the turn.
    pub flow_elevated_tools: Option<&'a [String]>,
    /// Shell command allow-patterns from the elevated system flow (`shell_allow`).
    /// When non-empty, elevated `shell_exec` must match at least one pattern.
    pub flow_shell_allow: Option<&'a [String]>,
    /// Tools blocked for this turn by the matched flow's `deny_tools`.
    pub flow_deny_tools: Option<&'a [String]>,
    /// Hard allow-list for this turn when the matched flow declares `tools:`.
    /// `tool_runner` denies calls outside this set.
    /// Frozen at flow-load (see `META_FLOW_ALLOWED_TOOLS`).
    pub flow_allowed_tools: Option<&'a [String]>,
}
