//! Agent runtime and execution environment.
//!
//! Manages the agent execution loop, LLM driver abstraction,
//! tool execution, and WASM sandboxing for untrusted skill/plugin code.

/// Default User-Agent header sent with all outgoing HTTP requests.
/// Some LLM providers (e.g. Moonshot, Qwen) reject requests without one.
pub const USER_AGENT: &str = concat!("opencarrier/", env!("CARGO_PKG_VERSION"));

pub mod a2a;
pub mod agent_loop;
pub mod api_tools;
pub mod apply_patch;
pub mod audit;
pub mod auth_cooldown;
pub mod channel_manager;
pub mod compactor;
pub mod context_budget;
pub mod context_overflow;
pub mod drivers;
pub mod file_view;
pub mod hooks;
pub mod host_functions;
pub mod intent_classifier;
pub mod kernel_handle;
pub mod link_understanding;
pub mod llm_driver;
pub mod llm_driver_impl;
pub mod llm_errors;
pub mod mcp;
pub mod mcp_server;
pub mod media_understanding;
pub mod memory_handle;
pub mod outbound;
pub mod plugin;
pub mod process_manager;
pub mod prompt_builder;
pub mod python_runtime;
pub mod reply_directives;
pub mod sandbox;
pub mod session_repair;
pub mod str_utils;
pub mod subprocess_sandbox;
pub mod text_tool_recovery;
pub mod think_filter;
pub mod tool_context;
pub mod tool_meta;
pub mod tool_runner;
pub mod tools;
pub mod wechat_identity;
pub mod workspace_context;
pub mod workspace_sandbox;
