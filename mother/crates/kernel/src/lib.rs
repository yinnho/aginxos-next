//! Core kernel for the Carrier Agent Operating System.
//!
//! The kernel manages agent lifecycles, memory, permissions, scheduling,
//! and inter-agent communication.

pub mod aginx_net;
pub mod background;
pub mod brain;
pub mod capabilities;
pub mod clone_marker;
pub mod config;
pub mod config_reload;
pub mod cron;
pub mod daemon;
pub mod dotenv;
pub mod error;
pub mod event_bus;
pub mod flow;
pub mod flow_runner;
pub mod handle;
pub mod heartbeat;
pub mod kernel;
pub mod management;
pub mod mcp_conn;
pub mod mcp_docker;
pub mod mcp_registry;
pub mod messaging;
pub mod metering;
pub mod plugins;
pub mod prompt_sources;
pub mod registry;
pub mod scheduler;
pub mod sender_gate;
pub mod sessions;
pub mod supervisor;
pub mod workspace;
pub use kernel::CarrierKernel;
pub use carrier_runtime::kernel_handle::KernelHandle;
