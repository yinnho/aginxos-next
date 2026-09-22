//! LoopContext — bundles all mutable state and references for the agent loop.
//!
//! The agent loop has many moving parts (session, messages, tools, MCP connections,
//! hooks, etc.) that are passed through the iteration cycle. Rather than threading
//! 20+ parameters through every function, we bundle them here so each phase function
//! takes `&mut LoopContext`.

use std::path::Path;
use std::sync::Arc;

use crate::context_budget::ContextBudget;
use crate::kernel_handle::KernelHandle;
use crate::llm_driver::{Brain, LlmDriver, StreamEvent};
use crate::mcp::McpConnection;
use carrier_memory::session::Session;
use carrier_memory::MemorySubstrate;
use carrier_types::agent::AgentManifest;
use carrier_types::message::Message;
use carrier_types::tool::ToolDefinition;

use super::state::LoopState;
use super::{PhaseCallback, TaskPlan};

/// Bundles all mutable state and shared references for a single agent loop execution.
///
/// Created once during setup, passed through each phase, and consumed during teardown.
pub(super) struct LoopContext<'a> {
    // ---- Agent identity ----
    pub manifest: &'a AgentManifest,
    pub user_message: &'a str,
    pub agent_id_str: String,

    // ---- Session & memory ----
    pub session: &'a mut Session,
    pub messages: Vec<Message>,
    pub session_base_len: usize,
    pub memory: &'a MemorySubstrate,
    pub memory_handle: Option<Arc<dyn crate::memory_handle::MemoryHandle>>,

    // ---- LLM ----
    pub driver: Arc<dyn LlmDriver>,
    pub brain: Option<Arc<dyn Brain>>,
    pub system_prompt: String,
    pub stream_tx: Option<tokio::sync::mpsc::Sender<StreamEvent>>,
    pub llm_concurrency_limit: Option<Arc<tokio::sync::Semaphore>>,

    // ---- Tools ----
    pub tools_owned: Vec<ToolDefinition>,
    pub discovered_tool_names: std::collections::HashSet<String>,
    pub loaded_flows: std::collections::HashSet<String>,
    /// Shell allow-patterns granted by flows loaded mid-turn via `flow_load`.
    /// Unioned with the active flow's `shell_allow` so a just-loaded flow's
    /// `scripts/` are runnable (historically `flow_load` injected the body but
    /// left the turn's shell gate frozen to the active/classified flow).
    pub loaded_flow_shell_allow: Vec<String>,
    /// Declared tools of ELEVATING flows (`shell_exec`/`process_start` +
    /// non-empty `shell_allow`) loaded mid-turn via `flow_load`. Unioned into
    /// the turn's `flow_elevated_tools` (level + admin-gate bypass) and the
    /// flow `tools:` hard sandbox, mirroring what loading the flow as
    /// active_flow would stamp. An explicit `flow_load` is sanctioned intent —
    /// unlike the default_flow fallback cage, which never grants authority.
    /// Commands stay scoped: elevation requires the flow's `shell_allow`, and
    /// the pattern gate runs on every shell_exec.
    pub loaded_flow_elevated_tools: Vec<String>,

    // ---- Kernel & external ----
    pub kernel: Option<Arc<dyn KernelHandle>>,
    pub mcp_connections: Option<&'a dashmap::DashMap<String, McpConnection>>,
    pub workspace_root: Option<&'a Path>,
    pub process_manager: Option<&'a crate::process_manager::ProcessManager>,
    pub context_budget: ContextBudget,

    // ---- Callbacks ----
    pub on_phase: Option<&'a PhaseCallback>,
    pub hooks: Option<&'a crate::hooks::HookRegistry>,

    // ---- Routing ----
    pub sender_id: Option<&'a str>,
    pub owner_id: Option<&'a str>,
    pub channel_type: Option<&'a str>,

    // ---- Config ----
    pub hand_allowed_env: Vec<String>,
    pub context_window_tokens: usize,

    // ---- Loop state ----
    pub state: LoopState,
    pub detected_plan: Option<TaskPlan>,
}

impl<'a> LoopContext<'a> {
    /// Get the current tools slice (borrows from tools_owned).
    /// Must be called fresh each time since tools_owned may have been modified.
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools_owned
    }

    /// Persist the last run summary to cross-session storage via kv_set.
    pub fn persist_last_run(&self, outcome: super::state::RunOutcome) {
        let last_run = self.state.to_last_run(outcome);
        if let Some(mh) = &self.memory_handle {
            let agent_key = format!("loop_state:{}", self.manifest.name);
            if let Ok(val) = serde_json::to_value(&last_run) {
                if let Err(e) = mh.kv_set(
                    &self.manifest.name,
                    self.owner_id.unwrap_or(""),
                    self.sender_id.unwrap_or(""),
                    &agent_key,
                    val,
                ) {
                    tracing::warn!("Failed to persist last run summary: {e}");
                }
            }
        }
    }

    /// Append the turn envelope events to the session event log
    /// (P1-A observational bypass — see `carrier_memory::session_events`).
    /// `TurnStart` at loop open; `TurnEnd` at every exit (natural, stuck,
    /// plan-break), absorbing the old in-memory `turn_log` totals.
    pub fn log_event(&self, kind: carrier_memory::SessionEventKind) {
        self.memory.session_events_append(
            &self.session.agent_name,
            &self.session.id.0.to_string(),
            vec![kind],
        );
    }

    /// `TurnEnd` payload from loop state. `tools_called`/`tool_errors` are
    /// summed from `turn_log` (partial by design — entries are only written
    /// on notable iterations), while `iterations` is authoritative.
    pub fn turn_end_event(&self, outcome: &str) -> carrier_memory::SessionEventKind {
        let tools_called: u32 = self
            .state
            .turn_log
            .iter()
            .map(|e| e.tools_called.len() as u32)
            .sum();
        let tool_errors: u32 = self.state.turn_log.iter().map(|e| e.tool_errors).sum();
        carrier_memory::SessionEventKind::TurnEnd {
            iterations: self.state.iteration,
            tools_called,
            tool_errors,
            outcome: outcome.to_string(),
        }
    }
}
