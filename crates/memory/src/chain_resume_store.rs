//! Chain-resume ledger (断链自动接续) backed by SQLite.
//!
//! Per-`(chain_id, step)` auto-resume attempt budget for chained one-shot
//! cron pipelines. The daemon bumps `attempts` for every self-heal re-fire
//! it issues; any agent/human-created chained job for the same
//! `(chain_id, step)` resets the budget to zero (ground-truth progress).
//! Daemon-issued resume jobs bypass `handle.rs cron_create`, so the reset
//! hook never fires for them — bump and reset stay disjoint by construction.
//!
//! Mirrors `AutomationRuleStore` (`automation_store.rs`): an
//! `Arc<Mutex<Connection>>` plus sync rusqlite bodies. Daemon code calls
//! these synchronously (same precedent as `cron_delivery().purge_expired()`).

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::{CarrierError, CarrierResult};

/// SQLite-backed chain-resume ledger.
#[derive(Clone)]
pub struct ChainResumeStore {
    conn: Arc<Mutex<Connection>>,
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

impl ChainResumeStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Current auto-resume attempt count for `(chain_id, step)`; 0 when no
    /// row exists (fresh chain — never resumed).
    pub fn get(&self, chain_id: &str, step: u32) -> CarrierResult<u32> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.query_row(
            "SELECT attempts FROM chain_resume_state WHERE chain_id=?1 AND step=?2",
            rusqlite::params![chain_id, step],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v.max(0) as u32)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(0),
            other => Err(CarrierError::Memory(other.to_string())),
        })
    }

    /// Increment the attempt budget and return the new count. Creates the
    /// row on first bump.
    pub fn bump(&self, chain_id: &str, step: u32) -> CarrierResult<u32> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.query_row(
            "INSERT INTO chain_resume_state (chain_id, step, attempts, updated_at) \
             VALUES (?1, ?2, 1, ?3) \
             ON CONFLICT(chain_id, step) DO UPDATE SET \
               attempts = attempts + 1, updated_at = ?3 \
             RETURNING attempts",
            rusqlite::params![chain_id, step, now_rfc3339()],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v.max(0) as u32)
        .map_err(|e| CarrierError::Memory(e.to_string()))
    }

    /// Zero the attempt budget for `(chain_id, step)`. Called from the
    /// `cron_create` progress hook: an agent/human scheduling this step is
    /// ground-truth chain progress, so retries get a fresh budget.
    pub fn reset(&self, chain_id: &str, step: u32) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO chain_resume_state (chain_id, step, attempts, updated_at) \
             VALUES (?1, ?2, 0, ?3) \
             ON CONFLICT(chain_id, step) DO UPDATE SET attempts = 0, updated_at = ?3",
            rusqlite::params![chain_id, step, now_rfc3339()],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Drop all rows for a chain — the tail step completed, the chain is
    /// done, no resume budget is ever needed again.
    pub fn clear_chain(&self, chain_id: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM chain_resume_state WHERE chain_id=?1",
            rusqlite::params![chain_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Delete rows not updated since the cutoff (rfc3339). Abandoned
    /// cap-circuited chains must not accumulate forever.
    pub fn purge_stale(&self, cutoff_rfc3339: &str) -> CarrierResult<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "DELETE FROM chain_resume_state WHERE updated_at < ?1",
            rusqlite::params![cutoff_rfc3339],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn test_store() -> ChainResumeStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        ChainResumeStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn bump_increments_and_creates() {
        let store = test_store();
        assert_eq!(store.get("c1", 2).unwrap(), 0); // missing row reads as 0
        assert_eq!(store.bump("c1", 2).unwrap(), 1); // first bump creates at 1
        assert_eq!(store.bump("c1", 2).unwrap(), 2);
        assert_eq!(store.get("c1", 2).unwrap(), 2);
        // Per-step keying: another step of the same chain has its own budget.
        assert_eq!(store.get("c1", 3).unwrap(), 0);
        // Per-chain keying: another chain's step is independent.
        assert_eq!(store.get("c2", 2).unwrap(), 0);
    }

    #[test]
    fn reset_zeroes_budget() {
        let store = test_store();
        store.bump("c1", 2).unwrap();
        store.bump("c1", 2).unwrap();
        store.reset("c1", 2).unwrap();
        assert_eq!(store.get("c1", 2).unwrap(), 0);
        assert_eq!(store.bump("c1", 2).unwrap(), 1); // counts restart from 1
                                                     // reset on a never-seen step just writes a zero row.
        store.reset("c1", 4).unwrap();
        assert_eq!(store.get("c1", 4).unwrap(), 0);
    }

    #[test]
    fn clear_chain_removes_all_steps() {
        let store = test_store();
        store.bump("c1", 1).unwrap();
        store.bump("c1", 2).unwrap();
        store.bump("c1", 3).unwrap();
        store.bump("c2", 1).unwrap();
        store.clear_chain("c1").unwrap();
        assert_eq!(store.get("c1", 1).unwrap(), 0);
        assert_eq!(store.get("c1", 2).unwrap(), 0);
        assert_eq!(store.get("c1", 3).unwrap(), 0);
        assert_eq!(store.get("c2", 1).unwrap(), 1); // other chain untouched
    }

    #[test]
    fn purge_stale_honors_cutoff() {
        let store = test_store();
        store.bump("old", 1).unwrap();
        store.bump("new", 1).unwrap();
        {
            // Age the "old" row past the cutoff directly (store only stamps
            // `now` on write; tests shouldn't fake the clock through it).
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE chain_resume_state SET updated_at='2020-01-01T00:00:00+00:00' \
                 WHERE chain_id='old'",
                [],
            )
            .unwrap();
        }
        let deleted = store.purge_stale("2026-01-01T00:00:00+00:00").unwrap();
        assert_eq!(deleted, 1);
        assert_eq!(store.get("old", 1).unwrap(), 0);
        assert_eq!(store.get("new", 1).unwrap(), 1);
    }
}
