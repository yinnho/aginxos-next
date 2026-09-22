//! Clone lifecycle system — knowledge evolution, version tracking.
//!
//! This crate provides the core logic for clone learning:
//! - **evolution**: Extract knowledge from conversations (pre-filter + analysis)
//! - **version**: Track knowledge file changes via JSONL log
//!
//! The crate has **no LLM dependency**. The kernel calls `build_analysis_prompt()`
//! to get the prompt, executes the LLM call itself, then passes the response to
//! `parse_analysis_response()` and `apply_evolution()`.

pub mod bloat;
pub mod compile;
pub mod evaluate;
pub mod evolution;
pub mod evolution_config;
pub mod feedback;
pub mod health;
pub mod parsers;
pub mod version;
pub mod watcher;
