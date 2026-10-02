//! Memory substrate for the Carrier Agent Operating System.
//!
//! Provides tree-based hierarchical memory with Obsidian-compatible content storage,
//! plus system infrastructure (agent registry, sessions, cron delivery).

pub mod acp_session;
pub mod chain_resume_store;
pub mod cron_store;
pub mod flow_run;
pub mod migration;
pub mod session;
pub mod session_events;
pub mod ticket_store;
pub mod system_kv;
pub mod tree;
pub mod usage;

mod substrate;
pub use cron_store::CronJobStore;
pub use flow_run::{FlowRunRow, FlowRunStore};
pub use session::SessionStore;
pub use session_events::{
    derive_messages, fold_surface, message_events, rebuild_turn_summaries, SessionEvent,
    SessionEventKind, SessionEventLog,
};
pub use substrate::MemorySubstrate;
