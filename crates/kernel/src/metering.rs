//! Metering engine — tracks LLM token usage and enforces budget alerts.

use chrono::Datelike;
use carrier_memory::usage::{UsageRecord, UsageStore, UsageSummary};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use carrier_types::agent::AgentId;
use carrier_types::config::BudgetConfig;
use carrier_types::error::CarrierResult;

/// A budget threshold that was just crossed.
#[derive(Debug, Clone)]
pub struct BudgetAlert {
    /// Percentage of budget consumed (e.g., 50, 80, 100).
    pub percent: u8,
    /// Total tokens used this month.
    pub used_tokens: u64,
    /// Configured monthly limit.
    pub limit_tokens: u64,
}

/// Tracks which (year, month, threshold%) alerts have already fired.
#[derive(Debug, Clone, Default)]
struct BudgetAlertState {
    fired: HashSet<(i32, u32, u8)>,
}

impl BudgetAlertState {
    fn has_fired(&self, year: i32, month: u32, pct: u8) -> bool {
        self.fired.contains(&(year, month, pct))
    }

    fn mark_fired(&mut self, year: i32, month: u32, pct: u8) {
        self.fired.insert((year, month, pct));
    }
}

/// The metering engine tracks token usage and enforces budget alerts.
pub struct MeteringEngine {
    /// Persistent usage store (SQLite-backed).
    store: Arc<UsageStore>,
    /// Budget configuration for alerting.
    budget: BudgetConfig,
    /// Alert dedup state — prevents the same threshold from firing twice in a month.
    alert_state: Arc<Mutex<BudgetAlertState>>,
}

impl MeteringEngine {
    /// Create a new metering engine with the given usage store and budget config.
    pub fn new(store: Arc<UsageStore>, budget: BudgetConfig) -> Self {
        Self {
            store,
            budget,
            alert_state: Arc::new(Mutex::new(BudgetAlertState::default())),
        }
    }

    /// Record a usage event (persists to SQLite).
    pub fn record(&self, record: &UsageRecord) -> CarrierResult<()> {
        self.store.record(record)
    }

    /// Record usage and check budget thresholds. Returns an alert if a new
    /// threshold was crossed this month.
    pub fn record_and_check(&self, record: &UsageRecord) -> CarrierResult<Option<BudgetAlert>> {
        self.record(record)?;
        Ok(self.check_budget())
    }

    /// Check whether the monthly token budget has crossed any configured
    /// threshold. Returns the first un-fired threshold alert.
    pub fn check_budget(&self) -> Option<BudgetAlert> {
        if self.budget.monthly_token_limit == 0 || self.budget.alert_thresholds.is_empty() {
            return None;
        }

        // Sum tokens from the last 31 days as the "monthly" window
        let daily = match self.store.query_daily_breakdown(31) {
            Ok(d) => d,
            Err(_) => return None,
        };

        let used_tokens: u64 = daily.iter().map(|d| d.tokens).sum();

        let limit = self.budget.monthly_token_limit;
        let pct = ((used_tokens as u128 * 100) / limit as u128).min(255) as u8;

        let now = chrono::Utc::now();
        let year = now.year();
        let month = now.month();

        let mut state = self.alert_state.lock().ok()?;

        // Find the first un-fired threshold that has been crossed
        for &threshold in &self.budget.alert_thresholds {
            if pct >= threshold && !state.has_fired(year, month, threshold) {
                state.mark_fired(year, month, threshold);
                return Some(BudgetAlert {
                    percent: threshold,
                    used_tokens,
                    limit_tokens: limit,
                });
            }
        }

        None
    }

    /// Get a usage summary, optionally filtered by agent.
    pub fn get_summary(&self, agent_id: Option<AgentId>) -> CarrierResult<UsageSummary> {
        self.store.query_summary(agent_id)
    }

    /// Get monthly budget status (used tokens, limit, percentage, fired alerts).
    pub fn get_budget_status(&self) -> BudgetStatus {
        let limit = self.budget.monthly_token_limit;
        let used_tokens = self
            .store
            .query_daily_breakdown(31)
            .map(|d| d.iter().map(|x| x.tokens).sum())
            .unwrap_or(0);

        let pct = if limit > 0 {
            ((used_tokens as u128 * 100) / limit as u128).min(255) as u8
        } else {
            0
        };

        let now = chrono::Utc::now();
        let fired: Vec<u8> = self
            .alert_state
            .lock()
            .map(|s| {
                s.fired
                    .iter()
                    .filter(|(y, m, _)| *y == now.year() && *m == now.month())
                    .map(|(_, _, pct)| *pct)
                    .collect()
            })
            .unwrap_or_default();

        BudgetStatus {
            used_tokens,
            limit_tokens: limit,
            percent: pct,
            fired_thresholds: fired,
            thresholds: self.budget.alert_thresholds.clone(),
            alert_channel: self.budget.alert_channel.clone(),
            alert_recipient: self.budget.alert_recipient.clone(),
        }
    }

    /// Clean up old usage records.
    pub fn cleanup(&self, days: u32) -> CarrierResult<usize> {
        self.store.cleanup_old(days)
    }
}

/// Snapshot of the current budget state (for API responses).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BudgetStatus {
    pub used_tokens: u64,
    pub limit_tokens: u64,
    pub percent: u8,
    pub fired_thresholds: Vec<u8>,
    pub thresholds: Vec<u8>,
    pub alert_channel: Option<String>,
    pub alert_recipient: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrier_memory::MemorySubstrate;

    fn setup(budget: BudgetConfig) -> MeteringEngine {
        let substrate = MemorySubstrate::open_in_memory().unwrap();
        let store = Arc::new(UsageStore::new(substrate.usage_conn()));
        MeteringEngine::new(store, budget)
    }

    #[test]
    fn test_record_and_get_summary() {
        let engine = setup(BudgetConfig::default());
        let agent_id = AgentId::new();

        engine
            .record(&UsageRecord {
                agent_id,
                model: "test-model".to_string(),
                input_tokens: 500,
                output_tokens: 200,
                tool_calls: 3,
            })
            .unwrap();

        let summary = engine.get_summary(Some(agent_id)).unwrap();
        assert_eq!(summary.call_count, 1);
        assert_eq!(summary.total_input_tokens, 500);
    }

    #[test]
    fn test_budget_disabled_when_limit_zero() {
        let engine = setup(BudgetConfig {
            monthly_token_limit: 0,
            alert_thresholds: vec![50, 80, 100],
            ..Default::default()
        });
        assert!(engine.check_budget().is_none());
    }

    #[test]
    fn test_budget_disabled_when_no_thresholds() {
        let engine = setup(BudgetConfig {
            monthly_token_limit: 1000,
            alert_thresholds: vec![],
            ..Default::default()
        });
        assert!(engine.check_budget().is_none());
    }

    #[test]
    fn test_budget_alert_fires_once() {
        let engine = setup(BudgetConfig {
            monthly_token_limit: 10_000,
            alert_thresholds: vec![50],
            ..Default::default()
        });

        let agent_id = AgentId::new();

        // Record 6k tokens (60%) — should fire 50% alert
        let alert = engine
            .record_and_check(&UsageRecord {
                agent_id,
                model: "test".to_string(),
                input_tokens: 4000,
                output_tokens: 2000,
                tool_calls: 1,
            })
            .unwrap();
        assert!(alert.is_some());
        assert_eq!(alert.unwrap().percent, 50);

        // Second record — should NOT fire again (dedup)
        let alert2 = engine
            .record_and_check(&UsageRecord {
                agent_id,
                model: "test".to_string(),
                input_tokens: 1000,
                output_tokens: 500,
                tool_calls: 0,
            })
            .unwrap();
        assert!(alert2.is_none());
    }

    #[test]
    fn test_budget_multiple_thresholds() {
        let engine = setup(BudgetConfig {
            monthly_token_limit: 10_000,
            alert_thresholds: vec![50, 80, 100],
            ..Default::default()
        });

        let agent_id = AgentId::new();

        // Cross 50%
        let a1 = engine
            .record_and_check(&UsageRecord {
                agent_id,
                model: "test".to_string(),
                input_tokens: 6000,
                output_tokens: 0,
                tool_calls: 0,
            })
            .unwrap();
        assert!(a1.is_some());
        assert_eq!(a1.unwrap().percent, 50);

        // Cross 80% (same record already at 60%, need more to reach 80%)
        let a2 = engine
            .record_and_check(&UsageRecord {
                agent_id,
                model: "test".to_string(),
                input_tokens: 3000,
                output_tokens: 0,
                tool_calls: 0,
            })
            .unwrap();
        assert!(a2.is_some());
        assert_eq!(a2.unwrap().percent, 80);

        // Cross 100%
        let a3 = engine
            .record_and_check(&UsageRecord {
                agent_id,
                model: "test".to_string(),
                input_tokens: 2000,
                output_tokens: 0,
                tool_calls: 0,
            })
            .unwrap();
        assert!(a3.is_some());
        assert_eq!(a3.unwrap().percent, 100);
    }

    #[test]
    fn test_budget_status() {
        let engine = setup(BudgetConfig {
            monthly_token_limit: 10_000,
            alert_thresholds: vec![50, 80],
            alert_channel: Some("dingtalk".to_string()),
            alert_recipient: Some("admin".to_string()),
        });

        let status = engine.get_budget_status();
        assert_eq!(status.limit_tokens, 10_000);
        assert_eq!(status.used_tokens, 0);
        assert_eq!(status.percent, 0);
        assert!(status.fired_thresholds.is_empty());
        assert_eq!(status.alert_channel.as_deref(), Some("dingtalk"));
        assert_eq!(status.alert_recipient.as_deref(), Some("admin"));
    }
}
