//! Job queue store for background tree memory tasks.

use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};

use super::types::{Job, JobKind, JobStatus, NewJob};

/// Job store backed by SQLite.
#[derive(Clone)]
pub struct JobStore {
    conn: Arc<Mutex<Connection>>,
}

/// Lock duration for claimed jobs (5 minutes).
const LOCK_DURATION_MS: i64 = 300_000;

impl JobStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Enqueue a new job. Returns Some(job_id) on success, None if deduped.
    pub fn enqueue(&self, job: &NewJob) -> CarrierResult<Option<String>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        let job_id = format!("job_{}", uuid::Uuid::new_v4().simple());

        // If there's a dedupe_key, check for an active job with the same key
        if let Some(ref dedupe_key) = job.dedupe_key {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) FROM mem_tree_jobs
                     WHERE owner_id = ?1 AND dedupe_key = ?2
                       AND status IN ('ready', 'running')",
                    rusqlite::params![job.owner_id, dedupe_key],
                    |row| {
                        let count: i64 = row.get(0)?;
                        Ok(count > 0)
                    },
                )
                .unwrap_or(false);

            if exists {
                return Ok(None);
            }
        }

        let available_at_ms = job.available_at_ms.unwrap_or(now_ms);
        let max_attempts = job.max_attempts.unwrap_or(5);

        conn.execute(
            "INSERT INTO mem_tree_jobs
             (id, owner_id, kind, payload_json, dedupe_key, status, attempts,
              max_attempts, available_at_ms, locked_until_ms, last_error,
              created_at_ms, started_at_ms, completed_at_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, 'ready', 0, ?6, ?7, NULL, NULL, ?8, NULL, NULL)",
            rusqlite::params![
                job_id,
                job.owner_id,
                job.kind.as_str(),
                job.payload_json,
                job.dedupe_key,
                max_attempts,
                available_at_ms,
                now_ms,
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;

        Ok(Some(job_id))
    }

    /// Claim the next ready job for an owner (or any owner if None).
    pub fn claim_next(&self, owner_id: Option<&str>) -> CarrierResult<Option<Job>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        let locked_until = now_ms + LOCK_DURATION_MS;

        let sql = match owner_id {
            Some(_) => {
                "SELECT id, owner_id, kind, payload_json, dedupe_key, status,
                               attempts, max_attempts, available_at_ms, locked_until_ms,
                               last_error, created_at_ms, started_at_ms, completed_at_ms
                        FROM mem_tree_jobs
                        WHERE owner_id = ?1 AND status = 'ready' AND available_at_ms <= ?2
                        ORDER BY created_at_ms ASC LIMIT 1"
            }
            None => {
                "SELECT id, owner_id, kind, payload_json, dedupe_key, status,
                            attempts, max_attempts, available_at_ms, locked_until_ms,
                            last_error, created_at_ms, started_at_ms, completed_at_ms
                     FROM mem_tree_jobs
                     WHERE status = 'ready' AND available_at_ms <= ?1
                     ORDER BY created_at_ms ASC LIMIT 1"
            }
        };

        let result = match owner_id {
            Some(oid) => conn.query_row(sql, rusqlite::params![oid, now_ms], |row| {
                Self::row_to_job(row)
            }),
            None => conn.query_row(sql, rusqlite::params![now_ms], Self::row_to_job),
        };

        match result {
            Ok(job) => {
                // Mark as running
                conn.execute(
                    "UPDATE mem_tree_jobs SET status = 'running', attempts = attempts + 1,
                     locked_until_ms = ?1, started_at_ms = COALESCE(started_at_ms, ?2)
                     WHERE id = ?3",
                    rusqlite::params![locked_until, now_ms, job.id],
                )
                .map_err(|e| CarrierError::Memory(e.to_string()))?;

                Ok(Some(Job {
                    status: JobStatus::Running,
                    attempts: job.attempts + 1,
                    locked_until_ms: Some(locked_until),
                    started_at_ms: Some(now_ms),
                    ..job
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(CarrierError::Memory(e.to_string())),
        }
    }

    /// Mark a job as done.
    pub fn mark_done(&self, job_id: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();
        conn.execute(
            "UPDATE mem_tree_jobs SET status = 'done', completed_at_ms = ?1, locked_until_ms = NULL
             WHERE id = ?2",
            rusqlite::params![now_ms, job_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Mark a job as failed with an error message.
    pub fn mark_failed(&self, job_id: &str, error: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();

        // Check if we should retry or mark as permanently failed
        let attempts: i32 = conn
            .query_row(
                "SELECT attempts FROM mem_tree_jobs WHERE id = ?1",
                rusqlite::params![job_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let max_attempts: i32 = conn
            .query_row(
                "SELECT max_attempts FROM mem_tree_jobs WHERE id = ?1",
                rusqlite::params![job_id],
                |row| row.get(0),
            )
            .unwrap_or(5);

        if attempts >= max_attempts {
            conn.execute(
                "UPDATE mem_tree_jobs SET status = 'failed', last_error = ?1,
                 completed_at_ms = ?2, locked_until_ms = NULL WHERE id = ?3",
                rusqlite::params![error, now_ms, job_id],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        } else {
            // Re-queue for retry
            conn.execute(
                "UPDATE mem_tree_jobs SET status = 'ready', last_error = ?1,
                 locked_until_ms = NULL, available_at_ms = ?2 WHERE id = ?3",
                rusqlite::params![error, now_ms, job_id],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        }
        Ok(())
    }

    /// Defer a job to be available at a future time.
    pub fn defer(&self, job_id: &str, available_at_ms: i64) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        conn.execute(
            "UPDATE mem_tree_jobs SET status = 'ready', locked_until_ms = NULL,
             available_at_ms = ?1 WHERE id = ?2",
            rusqlite::params![available_at_ms, job_id],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Recover stale locks — jobs that have been running for too long.
    pub fn recover_stale_locks(&self) -> CarrierResult<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();

        let count = conn
            .execute(
                "UPDATE mem_tree_jobs SET status = 'ready', locked_until_ms = NULL
                 WHERE status = 'running' AND locked_until_ms IS NOT NULL AND locked_until_ms < ?1",
                rusqlite::params![now_ms],
            )
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        Ok(count)
    }

    /// Count pending jobs by kind for an owner.
    pub fn count_pending(&self, owner_id: &str, kind: Option<JobKind>) -> CarrierResult<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let count: i64 = match kind {
            Some(k) => conn
                .query_row(
                    "SELECT COUNT(*) FROM mem_tree_jobs
                     WHERE owner_id = ?1 AND kind = ?2 AND status IN ('ready', 'running')",
                    rusqlite::params![owner_id, k.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| CarrierError::Memory(e.to_string()))?,
            None => conn
                .query_row(
                    "SELECT COUNT(*) FROM mem_tree_jobs
                     WHERE owner_id = ?1 AND status IN ('ready', 'running')",
                    rusqlite::params![owner_id],
                    |row| row.get(0),
                )
                .map_err(|e| CarrierError::Memory(e.to_string()))?,
        };

        Ok(count as usize)
    }

    /// Check if a source has already been ingested (dedup for document sources).
    pub fn check_ingested(
        &self,
        owner_id: &str,
        source_kind: &str,
        source_id: &str,
    ) -> CarrierResult<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM mem_tree_ingested_sources
                 WHERE owner_id = ?1 AND source_kind = ?2 AND source_id = ?3",
                rusqlite::params![owner_id, source_kind, source_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        Ok(count > 0)
    }

    /// Mark a source as ingested.
    pub fn mark_ingested(
        &self,
        owner_id: &str,
        source_kind: &str,
        source_id: &str,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        let now_ms = chrono::Utc::now().timestamp_millis();

        conn.execute(
            "INSERT OR IGNORE INTO mem_tree_ingested_sources
             (source_kind, source_id, owner_id, ingested_at_ms)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![source_kind, source_id, owner_id, now_ms],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    // -- Row mapper --------------------------------------------------------

    fn row_to_job(row: &rusqlite::Row) -> rusqlite::Result<Job> {
        let kind_str: String = row.get(2)?;
        let kind = match kind_str.as_str() {
            "extract_chunk" => JobKind::ExtractChunk,
            "append_buffer" => JobKind::AppendBuffer,
            "seal" => JobKind::Seal,
            "topic_route" => JobKind::TopicRoute,
            "digest_daily" => JobKind::DigestDaily,
            "flush_stale" => JobKind::FlushStale,
            _ => JobKind::ExtractChunk,
        };
        let status_str: String = row.get(5)?;
        let status = match status_str.as_str() {
            "ready" => JobStatus::Ready,
            "running" => JobStatus::Running,
            "done" => JobStatus::Done,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            _ => JobStatus::Ready,
        };

        Ok(Job {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            kind,
            payload_json: row.get(3)?,
            dedupe_key: row.get(4)?,
            status,
            attempts: row.get(6)?,
            max_attempts: row.get(7)?,
            available_at_ms: row.get(8)?,
            locked_until_ms: row.get(9)?,
            last_error: row.get(10)?,
            created_at_ms: row.get(11)?,
            started_at_ms: row.get(12)?,
            completed_at_ms: row.get(13)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::run_migrations;

    fn setup() -> JobStore {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        JobStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_enqueue_and_claim() {
        let store = setup();
        let job = NewJob {
            owner_id: "owner_1".to_string(),
            kind: JobKind::Seal,
            payload_json: r#"{"tree_id":"tree_1","level":0}"#.to_string(),
            dedupe_key: Some("seal:tree_1:0".to_string()),
            available_at_ms: None,
            max_attempts: None,
        };

        let job_id = store.enqueue(&job).unwrap().unwrap();
        assert!(!job_id.is_empty());

        let claimed = store.claim_next(Some("owner_1")).unwrap().unwrap();
        assert_eq!(claimed.id, job_id);
        assert_eq!(claimed.status, JobStatus::Running);
    }

    #[test]
    fn test_dedupe() {
        let store = setup();
        let job = NewJob {
            owner_id: "owner_1".to_string(),
            kind: JobKind::Seal,
            payload_json: "{}".to_string(),
            dedupe_key: Some("seal:tree_1:0".to_string()),
            available_at_ms: None,
            max_attempts: None,
        };

        let first = store.enqueue(&job).unwrap();
        assert!(first.is_some());

        // Same dedupe_key should be suppressed
        let second = store.enqueue(&job).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn test_mark_done() {
        let store = setup();
        let job = NewJob {
            owner_id: "owner_1".to_string(),
            kind: JobKind::Seal,
            payload_json: "{}".to_string(),
            dedupe_key: None,
            available_at_ms: None,
            max_attempts: None,
        };

        let job_id = store.enqueue(&job).unwrap().unwrap();
        let _ = store.claim_next(Some("owner_1")).unwrap();
        store.mark_done(&job_id).unwrap();

        // No more ready jobs
        assert!(store.claim_next(Some("owner_1")).unwrap().is_none());
    }

    #[test]
    fn test_mark_failed_retry() {
        let store = setup();
        let job = NewJob {
            owner_id: "owner_1".to_string(),
            kind: JobKind::Seal,
            payload_json: "{}".to_string(),
            dedupe_key: None,
            available_at_ms: None,
            max_attempts: Some(3),
        };

        let job_id = store.enqueue(&job).unwrap().unwrap();
        let _ = store.claim_next(Some("owner_1")).unwrap();
        store.mark_failed(&job_id, "timeout").unwrap();

        // Should be re-queued for retry
        let claimed = store.claim_next(Some("owner_1")).unwrap();
        assert!(claimed.is_some());
        assert_eq!(claimed.unwrap().attempts, 2);
    }

    #[test]
    fn test_owner_isolation() {
        let store = setup();
        let job = NewJob {
            owner_id: "owner_1".to_string(),
            kind: JobKind::Seal,
            payload_json: "{}".to_string(),
            dedupe_key: None,
            available_at_ms: None,
            max_attempts: None,
        };

        store.enqueue(&job).unwrap();

        // Different owner should not see the job
        let claimed = store.claim_next(Some("owner_2")).unwrap();
        assert!(claimed.is_none());
    }

    #[test]
    fn test_check_and_mark_ingested() {
        let store = setup();

        assert!(!store
            .check_ingested("owner_1", "document", "doc_1")
            .unwrap());

        store.mark_ingested("owner_1", "document", "doc_1").unwrap();

        assert!(store
            .check_ingested("owner_1", "document", "doc_1")
            .unwrap());
        assert!(!store
            .check_ingested("owner_2", "document", "doc_1")
            .unwrap());
    }

    #[test]
    fn test_recover_stale_locks() {
        let store = setup();
        let job = NewJob {
            owner_id: "owner_1".to_string(),
            kind: JobKind::Seal,
            payload_json: "{}".to_string(),
            dedupe_key: None,
            available_at_ms: None,
            max_attempts: None,
        };

        let job_id = store.enqueue(&job).unwrap().unwrap();
        let _ = store.claim_next(Some("owner_1")).unwrap();

        // Manually set locked_until to past to simulate stale lock
        {
            let conn = store.conn.lock().unwrap_or_else(|e| e.into_inner());
            conn.execute(
                "UPDATE mem_tree_jobs SET locked_until_ms = 1 WHERE id = ?1",
                rusqlite::params![job_id],
            )
            .unwrap();
        }

        let recovered = store.recover_stale_locks().unwrap();
        assert_eq!(recovered, 1);

        // Job should be claimable again
        let claimed = store.claim_next(Some("owner_1")).unwrap();
        assert!(claimed.is_some());
    }
}
