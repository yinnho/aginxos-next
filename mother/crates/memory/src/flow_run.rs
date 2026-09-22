//! Persistent store for multi-step flow execution state (`flow_runs` table).
//!
//! Each row records one `run_flow` invocation: the flow, its input, completed
//! step output snapshots, the waiting `user_input` step (if suspended), and
//! status. In stage 2 incremental B this is run history/audit; the suspend/
//! resume columns (`waiting_at`, `map_context`) are exercised once `user_input`
//! lands (stage D).

use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use carrier_types::error::{CarrierError, CarrierResult};

/// A flow run row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowRunRow {
    pub run_id: String,
    pub session_id: String,
    pub agent_id: String,
    pub sender_id: String,
    pub flow_name: String,
    /// JSON: flow input `{user_message, user_id, ...}`.
    pub input: String,
    /// JSON: `{step_id: output_snapshot}`.
    pub completed_steps: String,
    /// Suspended `user_input` step id; `None` when not waiting.
    pub waiting_at: Option<String>,
    /// JSON: map iteration progress when `waiting_at` is inside a map body.
    pub map_context: Option<String>,
    /// `running` | `waiting` | `completed` | `cancelled` | `timed_out` | `failed`.
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    /// Deadline (RFC3339) past which a `waiting` run is reaped as `timed_out`.
    /// `None` when not waiting or no timeout set.
    pub expires_at: Option<String>,
}

/// SQLite-backed flow run store.
#[derive(Clone)]
pub struct FlowRunStore {
    conn: Arc<Mutex<Connection>>,
}

impl FlowRunStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Create a new flow run record (status defaults to whatever the row says,
    /// typically `running`).
    pub fn create(&self, row: &FlowRunRow) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "INSERT INTO flow_runs \
               (run_id, session_id, agent_id, sender_id, flow_name, input, completed_steps, \
                waiting_at, map_context, status, created_at, updated_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                row.run_id,
                row.session_id,
                row.agent_id,
                row.sender_id,
                row.flow_name,
                row.input,
                row.completed_steps,
                row.waiting_at,
                row.map_context,
                row.status,
                row.created_at,
                row.updated_at,
                row.expires_at,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Load a flow run by id.
    pub fn get(&self, run_id: &str) -> CarrierResult<Option<FlowRunRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT run_id, session_id, agent_id, sender_id, flow_name, input, \
                        completed_steps, waiting_at, map_context, status, created_at, updated_at, \
                        expires_at \
                 FROM flow_runs WHERE run_id = ?1",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let res = stmt.query_row(rusqlite::params![run_id], |row| {
            Ok(FlowRunRow {
                run_id: row.get(0)?,
                session_id: row.get(1)?,
                agent_id: row.get(2)?,
                sender_id: row.get(3)?,
                flow_name: row.get(4)?,
                input: row.get(5)?,
                completed_steps: row.get(6)?,
                waiting_at: row.get(7)?,
                map_context: row.get(8)?,
                status: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
                expires_at: row.get(12)?,
            })
        });
        match res {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// List waiting flow runs for a sender+agent (used by `user_input` resume;
    /// included now so the schema/query is validated).
    pub fn list_pending(&self, sender_id: &str, agent_id: &str) -> CarrierResult<Vec<FlowRunRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT run_id, session_id, agent_id, sender_id, flow_name, input, \
                        completed_steps, waiting_at, map_context, status, created_at, updated_at, \
                        expires_at \
                 FROM flow_runs WHERE status = 'waiting' AND sender_id = ?1 AND agent_id = ?2 \
                 ORDER BY updated_at ASC",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![sender_id, agent_id], |row| {
                Ok(FlowRunRow {
                    run_id: row.get(0)?,
                    session_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    sender_id: row.get(3)?,
                    flow_name: row.get(4)?,
                    input: row.get(5)?,
                    completed_steps: row.get(6)?,
                    waiting_at: row.get(7)?,
                    map_context: row.get(8)?,
                    status: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    expires_at: row.get(12)?,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(out)
    }

    /// List `waiting` runs whose `expires_at` deadline has passed (used by the
    /// background expiry tick to mark them `timed_out`).
    pub fn list_expired(&self, now_rfc3339: &str) -> CarrierResult<Vec<FlowRunRow>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn
            .prepare(
                "SELECT run_id, session_id, agent_id, sender_id, flow_name, input, \
                        completed_steps, waiting_at, map_context, status, created_at, updated_at, \
                        expires_at \
                 FROM flow_runs \
                 WHERE status = 'waiting' AND expires_at IS NOT NULL AND expires_at <= ?1",
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![now_rfc3339], |row| {
                Ok(FlowRunRow {
                    run_id: row.get(0)?,
                    session_id: row.get(1)?,
                    agent_id: row.get(2)?,
                    sender_id: row.get(3)?,
                    flow_name: row.get(4)?,
                    input: row.get(5)?,
                    completed_steps: row.get(6)?,
                    waiting_at: row.get(7)?,
                    map_context: row.get(8)?,
                    status: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    expires_at: row.get(12)?,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| CarrierError::Memory(e.to_string()))?);
        }
        Ok(out)
    }

    /// Update status and the completed-steps snapshot. Clears `expires_at` so a
    /// run that has left the `waiting` state is not later reaped by the expiry
    /// tick.
    pub fn update_status(
        &self,
        run_id: &str,
        status: &str,
        completed_steps_json: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "UPDATE flow_runs SET status = ?2, completed_steps = ?3, expires_at = NULL, \
             updated_at = ?4 WHERE run_id = ?1",
            rusqlite::params![run_id, status, completed_steps_json, now_rfc3339()],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Mark a run as waiting at a `user_input` step (stage D). `expires_at` is
    /// the RFC3339 deadline after which the run is reaped as `timed_out`.
    pub fn set_waiting(
        &self,
        run_id: &str,
        step_id: &str,
        map_context: Option<&str>,
        expires_at: Option<&str>,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "UPDATE flow_runs SET status = 'waiting', waiting_at = ?2, map_context = ?3, \
             expires_at = ?4, updated_at = ?5 WHERE run_id = ?1",
            rusqlite::params![run_id, step_id, map_context, expires_at, now_rfc3339()],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }
}

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> FlowRunStore {
        let conn = Connection::open_in_memory().unwrap();
        crate::migration::run_migrations(&conn).unwrap();
        FlowRunStore::new(Arc::new(Mutex::new(conn)))
    }

    fn sample_row(id: &str) -> FlowRunRow {
        let now = now_rfc3339();
        FlowRunRow {
            run_id: id.to_string(),
            session_id: "sess-1".into(),
            agent_id: "agent-1".into(),
            sender_id: "sender-1".into(),
            flow_name: "short-drama".into(),
            input: r#"{"user_message":"hi"}"#.into(),
            completed_steps: "{}".into(),
            waiting_at: None,
            map_context: None,
            status: "running".into(),
            created_at: now.clone(),
            updated_at: now,
            expires_at: None,
        }
    }

    #[test]
    fn create_get_roundtrip() {
        let s = store();
        let row = sample_row("run-1");
        s.create(&row).unwrap();
        let got = s.get("run-1").unwrap().unwrap();
        assert_eq!(got.flow_name, "short-drama");
        assert_eq!(got.status, "running");
        assert_eq!(got.input, r#"{"user_message":"hi"}"#);
    }

    #[test]
    fn get_missing_returns_none() {
        let s = store();
        assert!(s.get("nope").unwrap().is_none());
    }

    #[test]
    fn update_status_persists() {
        let s = store();
        s.create(&sample_row("run-2")).unwrap();
        s.update_status("run-2", "completed", r#"{"draft":"..."}"#)
            .unwrap();
        let got = s.get("run-2").unwrap().unwrap();
        assert_eq!(got.status, "completed");
        assert_eq!(got.completed_steps, r#"{"draft":"..."}"#);
    }

    #[test]
    fn list_pending_filters_waiting() {
        let s = store();
        let mut a = sample_row("run-a");
        a.status = "waiting".into();
        a.waiting_at = Some("review".into());
        let mut b = sample_row("run-b");
        b.status = "running".into();
        s.create(&a).unwrap();
        s.create(&b).unwrap();
        let pending = s.list_pending("sender-1", "agent-1").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].run_id, "run-a");
        assert_eq!(pending[0].waiting_at.as_deref(), Some("review"));
    }

    #[test]
    fn set_waiting_marks_status() {
        let s = store();
        s.create(&sample_row("run-c")).unwrap();
        s.set_waiting("run-c", "review", None, None).unwrap();
        let got = s.get("run-c").unwrap().unwrap();
        assert_eq!(got.status, "waiting");
        assert_eq!(got.waiting_at.as_deref(), Some("review"));
        assert!(got.expires_at.is_none());
    }

    #[test]
    fn set_waiting_records_expires_at() {
        let s = store();
        s.create(&sample_row("run-d")).unwrap();
        let deadline = "2099-01-01T00:00:00+00:00";
        s.set_waiting("run-d", "review", None, Some(deadline))
            .unwrap();
        let got = s.get("run-d").unwrap().unwrap();
        assert_eq!(got.status, "waiting");
        assert_eq!(got.expires_at.as_deref(), Some(deadline));
    }

    #[test]
    fn list_expired_returns_past_deadline_only() {
        let s = store();
        let past = "2000-01-01T00:00:00+00:00";
        let future = "2099-01-01T00:00:00+00:00";
        let mut a = sample_row("run-past");
        a.status = "waiting".into();
        a.waiting_at = Some("review".into());
        a.expires_at = Some(past.into());
        let mut b = sample_row("run-future");
        b.status = "waiting".into();
        b.waiting_at = Some("review".into());
        b.expires_at = Some(future.into());
        s.create(&a).unwrap();
        s.create(&b).unwrap();
        let now = "2026-07-15T00:00:00+00:00";
        let expired = s.list_expired(now).unwrap();
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].run_id, "run-past");
    }

    #[test]
    fn update_status_clears_expires_at() {
        let s = store();
        s.create(&sample_row("run-e")).unwrap();
        s.set_waiting("run-e", "review", None, Some("2099-01-01T00:00:00+00:00"))
            .unwrap();
        s.update_status("run-e", "completed", r#"{"review":"ok"}"#)
            .unwrap();
        let got = s.get("run-e").unwrap().unwrap();
        assert_eq!(got.status, "completed");
        assert!(got.expires_at.is_none());
    }
}
