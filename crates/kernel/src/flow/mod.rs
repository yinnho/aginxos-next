//! Multi-step flow DAG executor (`run_flow`).
//!
//! Executes a [`carrier_types::flow::FlowDef`] with non-empty `steps` as a topologically
//! ordered DAG. Split into focused submodules; the public surface is re-exported
//! from [`crate::flow_runner`] for existing call sites.

mod dag;
mod map;
mod report;
mod run;
mod steps;
mod subflow;
mod template;
mod types;

#[cfg(test)]
mod tests;

pub(crate) use types::{
    FlowOutcome, MapContext, MapOutcome, ResumeState, FAILURE_CANCEL_KEYWORDS, FLOW_DEPTH,
    MAX_FLOW_DEPTH,
};
