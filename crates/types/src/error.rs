//! Shared error types for the Carrier system.

use thiserror::Error;

/// Top-level error type for the Carrier system.
#[derive(Error, Debug)]
pub enum CarrierError {
    /// The requested agent was not found.
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    /// An agent with this name or ID already exists.
    #[error("Agent already exists: {0}")]
    AgentAlreadyExists(String),

    /// A capability check failed.
    #[error("Capability denied: {0}")]
    CapabilityDenied(String),

    /// A resource quota was exceeded.
    #[error("Resource quota exceeded: {0}")]
    QuotaExceeded(String),

    /// The agent is in an invalid state for the requested operation.
    #[error("Agent is in invalid state '{current}' for operation '{operation}'")]
    InvalidState {
        /// The current state of the agent.
        current: String,
        /// The operation that was attempted.
        operation: String,
    },

    /// The requested session was not found.
    #[error("Session not found: {0}")]
    SessionNotFound(String),

    /// The LLM concurrency limit was reached.
    #[error("LLM concurrency limit reached — too many parallel requests")]
    RateLimited,

    /// A memory substrate error occurred.
    #[error("Memory error: {0}")]
    Memory(String),

    /// A tool execution failed.
    #[error("Tool execution failed: {tool_id} — {reason}")]
    ToolExecution {
        /// The tool that failed.
        tool_id: String,
        /// Why it failed.
        reason: String,
    },

    /// An LLM driver error occurred.
    #[error("LLM driver error: {0}")]
    LlmDriver(String),

    /// A configuration error occurred.
    #[error("Configuration error: {0}")]
    Config(String),

    /// Failed to parse an agent manifest.
    #[error("Manifest parsing error: {0}")]
    ManifestParse(String),

    /// A WASM sandbox error occurred.
    #[error("WASM sandbox error: {0}")]
    Sandbox(String),

    /// A network error occurred.
    #[error("Network error: {0}")]
    Network(String),

    /// A serialization/deserialization error occurred.
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// The agent loop exceeded the maximum iteration count.
    #[error("Max iterations exceeded ({0}). Configure a higher limit in agent.toml under [autonomous] max_iterations")]
    MaxIterationsExceeded(u32),

    /// An I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// An internal error occurred.
    #[error("Internal error: {0}")]
    Internal(String),

    /// The agent loop was aborted by stuck detection (no-progress idle, tool
    /// loop, or declared max_iterations exceeded). Distinct variant so callers
    /// (e.g. `outcome_from_loop_err`) classify structurally instead of
    /// matching on message wording.
    #[error("Agent loop stuck: {0}")]
    LoopStuck(String),

    /// Authentication/authorization denied.
    #[error("Auth denied: {0}")]
    AuthDenied(String),

    /// Metering/cost tracking error.
    #[error("Metering error: {0}")]
    MeteringError(String),

    /// Invalid user input.
    #[error("Invalid input: {0}")]
    InvalidInput(String),

    /// The requested MCP server was not found.
    #[error("MCP server not found: {0}")]
    McpNotFound(String),

    /// A session ownership check failed.
    #[error("Session ownership error: {0}")]
    SessionOwnership(String),
}

/// Alias for Result with CarrierError.
pub type CarrierResult<T> = Result<T, CarrierError>;
