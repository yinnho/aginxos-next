//! Flow execution outcome / resume types and depth guards.

use std::cell::Cell;
use std::collections::HashMap;

use carrier_runtime::agent_loop::AgentLoopResult;
use serde_json::Value;
use carrier_types::message::TokenUsage;

// Recursion depth of `flow_exec`/`map` sub-flow calls within a task. Limits
// nested sub-flow invocation (mirrors `AGENT_CALL_DEPTH` in tool_runner).
tokio::task_local! {
    pub(crate) static FLOW_DEPTH: Cell<u32>;
}

/// Maximum `flow_exec`/`map` nesting depth.
pub(crate) const MAX_FLOW_DEPTH: u32 = 5;

/// Keywords that cancel a flow when the user replies to a **failure** progress
/// report (as opposed to a `user_input` step, which carries its own
/// `cancel_keywords`). Failure often correlates with the LLM/network being down
/// (e.g. 402 Insufficient Balance), so keyword matching -- not an LLM parse --
/// is the most robust trigger. Case-insensitive substring match (see
/// [`decide_cancel`]).
pub(crate) const FAILURE_CANCEL_KEYWORDS: &[&str] =
    &["取消", "cancel", "放弃", "abort", "算了", "不要了"];

/// Outcome of a `run_flow` invocation. A flow either runs to completion
/// (`Completed`) or suspends at a `user_input` step awaiting the human's reply
/// (`Suspended`).
pub(crate) enum FlowOutcome {
    /// The flow finished. `result.response` is the agent reply; `final_value`
    /// is the final step's structured output (used by `flow_exec` callers to
    /// pass structured results up the chain).
    Completed {
        result: AgentLoopResult,
        final_value: Option<Value>,
    },
    /// The flow suspended at a `user_input` step. `question` is the prompt to
    /// send to the user as the (intermediate) reply; the run is persisted as
    /// `waiting` and resumes on the user's next message.
    Suspended {
        question: String,
        total_usage: TokenUsage,
        iterations: u32,
    },
}

/// Outcome of executing a `map` step. `Done` carries the collected results
/// array; `Suspended` means the map's inline body paused at a `user_input`
/// step, persisting `map_context` so the iteration can resume.
pub(crate) enum MapOutcome {
    Done(Value, TokenUsage, u32),
    Suspended {
        question: String,
        /// The body `user_input` step id that is now waiting.
        body_step_id: String,
        /// Serialized [`MapContext`] to persist in `flow_runs.map_context`.
        map_context_json: String,
        expires_at: Option<String>,
        usage: TokenUsage,
        iterations: u32,
    },
}

/// Outcome of executing one element's inline body. `Done` carries the body's
/// outputs (for cancel detection) + final value (collected); `Suspended` means
/// a body `user_input` paused, carrying the body's outputs so far
/// (`body_completed`) up to the map loop.
pub(crate) enum BodyOutcome {
    Done {
        outputs: HashMap<String, Value>,
        final_value: Option<Value>,
        usage: TokenUsage,
        iterations: u32,
    },
    Suspended {
        question: String,
        step_id: String,
        outputs: HashMap<String, Value>,
        expires_at: Option<String>,
        usage: TokenUsage,
        iterations: u32,
    },
}

/// Resume state for an inline body (one element's body paused at a
/// `user_input` step). `body_completed` are the body steps done before the
/// suspend; the reply becomes the `waiting_step_id` step's `{ decision, text }`.
pub(crate) struct BodyResume {
    pub(crate) body_completed: HashMap<String, Value>,
    pub(crate) waiting_step_id: String,
    pub(crate) user_reply: String,
    pub(crate) cancel_keywords: Vec<String>,
}

/// Map iteration progress persisted when an interactive map's body suspends.
/// Stored as JSON in `flow_runs.map_context`; `waiting_at` holds the body
/// `user_input` step id.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct MapContext {
    pub map_step_id: String,
    pub over: Vec<Value>,
    pub current_index: usize,
    pub collected: Vec<Value>,
    pub body_completed: HashMap<String, Value>,
    #[serde(rename = "as")]
    pub as_name: String,
}

/// State carried into `run_flow` when resuming a suspended flow. `pre_outputs`
/// are the completed steps' snapshots (deserialized from `flow_runs`), and the
/// user's reply becomes the `waiting_step_id` step's output
/// `{ decision, text }`. `map_context` is set when the waiting step is inside
/// an interactive map's body (stage E.2).
pub(crate) struct ResumeState {
    pub run_id: String,
    pub pre_outputs: HashMap<String, Value>,
    pub waiting_step_id: String,
    pub user_reply: String,
    pub cancel_keywords: Vec<String>,
    pub map_context: Option<MapContext>,
}
