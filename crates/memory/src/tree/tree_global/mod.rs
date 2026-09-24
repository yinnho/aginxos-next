//! Global Activity Digest tree.
//!
//! One tree per owner, built end-of-day from source tree summaries so a
//! question like "what did I do in the last 7 days?" can be answered with
//! one summary hop.
//!
//! Level conventions (time-axis aligned):
//!   - L0 = one node per **day**
//!   - L1 = one node per **week** (~7 daily leaves)
//!   - L2 = one node per **month** (~4 weekly nodes)
//!   - L3 = one node per **year** (~12 monthly nodes)

/// Pure hotness formula for topic-tree spawning (reused by aginxMemory).
pub mod hotness;

/// Number of L0 (daily) nodes that seal into one L1 (weekly) node.
pub const WEEKLY_SEAL_THRESHOLD: usize = 7;

/// Literal scope used for the singleton global tree.
pub const GLOBAL_SCOPE: &str = "global";

/// Token budget for global-tree summariser output.
pub const GLOBAL_TOKEN_BUDGET: u32 = 4_000;
