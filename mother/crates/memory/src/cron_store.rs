//! Persistent cron job store backed by SQLite.
//!
//! Replaces the previous `cron_jobs.json` file. The CronScheduler still uses
//! an in-memory DashMap for hot-path operations; this store handles
//! persistence (load/save/delete) against the central `opencarrier.db`.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::scheduler::{CronAction, CronDelivery, CronJob, CronJobId, CronSchedule};

/// Runtime metadata not stored in CronJob itself.
#[derive(Debug, Serialize, Deserialize)]
pub struct JobMeta {
    pub job: CronJob,
    pub one_shot: bool,
    pub last_status: Option<String>,
    pub consecutive_errors: u32,
    /// In-flight guard: true while this job's fired turn is executing.
    /// `due_jobs` skips (but still pre-advances) a running job so a
    /// slow-running recurring fire is not re-entered concurrently — the
    /// missed interval is skipped, the next scheduled slot fires normally.
    /// Not persisted (transient runtime state).
    #[serde(skip)]
    pub running: std::sync::atomic::AtomicBool,
}

impl JobMeta {
    pub fn new(job: CronJob, one_shot: bool) -> Self {
        Self {
            job,
            one_shot,
            last_status: None,
            consecutive_errors: 0,
            running: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Clone for JobMeta {
    fn clone(&self) -> Self {
        Self {
            job: self.job.clone(),
            one_shot: self.one_shot,
            last_status: self.last_status.clone(),
            consecutive_errors: self.consecutive_errors,
            // Cloning never carries an in-flight state — a clone of a running
            // job is a fresh observation, not a running fire.
            running: std::sync::atomic::AtomicBool::new(
                self.running.load(std::sync::atomic::Ordering::Acquire),
            ),
        }
    }
}

/// SQLite-backed cron job store.
#[derive(Clone)]
pub struct CronJobStore {
    conn: Arc<Mutex<Connection>>,
}

impl CronJobStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Load all persisted jobs from the database.
    pub fn load_all(&self) -> CarrierResult<Vec<JobMeta>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, owner_id, sender_id, name, enabled, schedule, action, delivery, \
                    one_shot, last_status, consecutive_errors, created_at, last_run, next_run, chain \
             FROM cron_jobs"
        ).map_err(|e| CarrierError::Memory(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(RowData {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    owner_id: row.get(2)?,
                    sender_id: row.get(3)?,
                    name: row.get(4)?,
                    enabled: row.get(5)?,
                    schedule_json: row.get(6)?,
                    action_json: row.get(7)?,
                    delivery_json: row.get(8)?,
                    one_shot: row.get(9)?,
                    last_status: row.get(10)?,
                    consecutive_errors: row.get(11)?,
                    created_at: row.get(12)?,
                    last_run: row.get(13)?,
                    next_run: row.get(14)?,
                    chain_json: row.get(15)?,
                })
            })
            .map_err(|e| CarrierError::Memory(e.to_string()))?;

        let mut metas = Vec::new();
        for row in rows {
            let r = row.map_err(|e| CarrierError::Memory(e.to_string()))?;
            if let Some(meta) = row_to_meta(r) {
                metas.push(meta);
            }
        }
        Ok(metas)
    }

    /// Persist all jobs (replaces entire table contents). 事务包住
    /// DELETE+重插——中途崩不掉半张表，也让并发 CLI 的定点写不会插进
    /// 半完成的表重写里。
    pub fn save_all(&self, metas: &[JobMeta]) -> CarrierResult<()> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let tx = conn
            .transaction()
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        tx.execute("DELETE FROM cron_jobs", [])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        for meta in metas {
            Self::insert_meta_tx(&tx, meta)?;
        }
        tx.commit().map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Insert or update a single job.
    pub fn upsert(&self, meta: &JobMeta) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        Self::insert_meta_tx(&conn, meta)
    }

    /// 整行 upsert（冲突时全列覆盖）——结构性变更（新建/重装）用。
    fn insert_meta_tx(conn: &Connection, meta: &JobMeta) -> CarrierResult<()> {
        Self::insert_meta_inner(conn, meta, /* volatile_only */ false)
    }

    /// upsert_volatile 的落库体——冲突时只更新易变列。
    fn insert_meta_volatile(conn: &Connection, meta: &JobMeta) -> CarrierResult<()> {
        Self::insert_meta_inner(conn, meta, /* volatile_only */ true)
    }

    fn insert_meta_inner(conn: &Connection, meta: &JobMeta, volatile_only: bool) -> CarrierResult<()> {
        let schedule_json = serde_json::to_string(&meta.job.schedule)
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let action_json = serde_json::to_string(&meta.job.action)
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let delivery_json = serde_json::to_string(&meta.job.delivery)
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        let chain_json = meta
            .job
            .chain
            .as_ref()
            .and_then(|c| serde_json::to_string(c).ok());
        let created_at = meta.job.created_at.to_rfc3339();
        let last_run = meta.job.last_run.map(|t| t.to_rfc3339());
        let next_run = meta.job.next_run.map(|t| t.to_rfc3339());
        let agent_id = meta.job.agent_id.to_string();
        let id = meta.job.id.to_string();

        let conflict_clause = if volatile_only {
            // 易变列-only：enabled 属于 CLI/主人路径，persist 不越权覆盖。
            "ON CONFLICT(id) DO UPDATE SET \
               last_run=?14, next_run=?15, last_status=?11, consecutive_errors=?12"
        } else {
            "ON CONFLICT(id) DO UPDATE SET \
               agent_id=?2, owner_id=?3, sender_id=?4, name=?5, enabled=?6, schedule=?7, action=?8, \
               delivery=?9, one_shot=?10, last_status=?11, consecutive_errors=?12, \
               last_run=?14, next_run=?15, chain=?16"
        };
        let sql = format!(
            "INSERT INTO cron_jobs (id, agent_id, owner_id, sender_id, name, enabled, schedule, action, delivery, \
                                    one_shot, last_status, consecutive_errors, created_at, last_run, next_run, chain) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
             {conflict_clause}"
        );

        conn.execute(
            &sql,
            rusqlite::params![
                id, agent_id, meta.job.owner_id, meta.job.sender_id, meta.job.name,
                meta.job.enabled as i32, schedule_json, action_json, delivery_json,
                meta.one_shot as i32, meta.last_status, meta.consecutive_errors as i32,
                created_at, last_run, next_run, chain_json,
            ],
        ).map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// Delete a job by ID.
    pub fn delete(&self, id: &str) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute("DELETE FROM cron_jobs WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// 定点改 enabled（CLI 暂停/恢复路径）。只动这一列——不整行 upsert，
    /// 否则会把内存里的陈旧字段盖回 DB，吃掉并发 CLI 的编辑。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "UPDATE cron_jobs SET enabled = ?2 WHERE id = ?1",
            rusqlite::params![id, enabled as i32],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// 定点写一轮执行结果（fire 收尾路径）。只动易变列，**不碰
    /// enabled/name/schedule**——CLI 并发暂停不会被 fire 的写回冲掉。
    /// `enabled_override` 仅用于连续失败自动停用（Some(false)）。
    pub fn record_outcome(
        &self,
        id: &str,
        last_run: DateTime<Utc>,
        next_run: Option<DateTime<Utc>>,
        last_status: &str,
        consecutive_errors: u32,
        enabled_override: Option<bool>,
    ) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        conn.execute(
            "UPDATE cron_jobs SET last_run = ?2, next_run = ?3, last_status = ?4, \
                    consecutive_errors = ?5, \
                    enabled = COALESCE(?6, enabled) \
             WHERE id = ?1",
            rusqlite::params![
                id,
                last_run.to_rfc3339(),
                next_run.map(|t| t.to_rfc3339()),
                last_status,
                consecutive_errors as i32,
                enabled_override.map(|b| b as i32),
            ],
        )
        .map_err(|e| CarrierError::Memory(e.to_string()))?;
        Ok(())
    }

    /// 周期 persist 用：整行插入，冲突时只更新易变列。新 job（内存有、
    /// DB 无）落全量；既有行的 enabled 由 CLI/主人路径独占，persist 不越权。
    pub fn upsert_volatile(&self, meta: &JobMeta) -> CarrierResult<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| CarrierError::Internal(e.to_string()))?;
        Self::insert_meta_volatile(&conn, meta)
    }
}

struct RowData {
    id: String,
    agent_id: String,
    owner_id: Option<String>,
    sender_id: Option<String>,
    name: String,
    enabled: i32,
    schedule_json: String,
    action_json: String,
    delivery_json: String,
    one_shot: i32,
    last_status: Option<String>,
    consecutive_errors: i32,
    created_at: String,
    last_run: Option<String>,
    next_run: Option<String>,
    chain_json: Option<String>,
}

fn row_to_meta(r: RowData) -> Option<JobMeta> {
    let schedule: CronSchedule = serde_json::from_str(&r.schedule_json).ok()?;
    let action: CronAction = serde_json::from_str(&r.action_json).ok()?;
    let delivery: CronDelivery = serde_json::from_str(&r.delivery_json).ok()?;
    let created_at: DateTime<Utc> = r.created_at.parse().ok()?;
    let last_run = r.last_run.and_then(|s| s.parse::<DateTime<Utc>>().ok());
    let next_run = r.next_run.and_then(|s| s.parse::<DateTime<Utc>>().ok());
    let id = CronJobId::from_str(&r.id).ok()?;
    // Parse (not from_string): agent_id is stored as its canonical UUID string
    // and must round-trip losslessly. `from_string` is the deterministic v5
    // hash helper — hashing the UUID string here silently produced a garbage
    // AgentId on every daemon restart, so each DB-loaded cron job died in the
    // orphan-cleanup check at fire time ("agent no longer in registry",
    // 2026-08-16 jiakao publish job: 8f306fe4.. → v5 hash 6aae753e..).
    let agent_id = r.agent_id.parse::<carrier_types::agent::AgentId>().ok()?;

    let job = CronJob {
        id,
        agent_id,
        owner_id: r.owner_id,
        sender_id: r.sender_id,
        name: r.name,
        enabled: r.enabled != 0,
        schedule,
        action,
        delivery,
        chain: r.chain_json.and_then(|s| serde_json::from_str(&s).ok()),
        created_at,
        last_run,
        next_run,
    };

    Some(JobMeta {
        job,
        one_shot: r.one_shot != 0,
        last_status: r.last_status,
        consecutive_errors: r.consecutive_errors as u32,
        running: std::sync::atomic::AtomicBool::new(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (2026-08-16): `row_to_meta` converted the stored agent_id
    /// string with `AgentId::from_string` — the deterministic UUID **v5 hash**
    /// helper — instead of parsing it. Every daemon restart therefore
    /// corrupted all DB-loaded cron jobs' agent_id (8f306fe4.. hashed into
    /// 6aae753e..), and each job then died in the orphan-cleanup check at
    /// fire time ("agent no longer in registry"). agent_id must round-trip
    /// losslessly through save/load.
    #[test]
    fn agent_id_roundtrips_through_sqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE cron_jobs (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                owner_id TEXT,
                sender_id TEXT,
                name TEXT NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                schedule TEXT NOT NULL,
                action TEXT NOT NULL,
                delivery TEXT NOT NULL,
                one_shot INTEGER NOT NULL DEFAULT 0,
                last_status TEXT,
                consecutive_errors INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                last_run TEXT,
                next_run TEXT,
                chain TEXT)",
            [],
        )
        .unwrap();
        let store = CronJobStore::new(Arc::new(Mutex::new(conn)));

        let agent_id = carrier_types::agent::AgentId::new();
        let meta = JobMeta::new(
            CronJob {
                id: CronJobId::new(),
                agent_id,
                owner_id: None,
                sender_id: Some("o-test@im.wechat".to_string()),
                name: "roundtrip".to_string(),
                schedule: CronSchedule::At { at: Utc::now() },
                action: CronAction::AgentTurn {
                    message: "m".to_string(),
                    model_override: None,
                    timeout_secs: Some(300),
                    active_flow: Some("draft-publisher".to_string()),
                    session_label: Some("lbl".to_string()),
                },
                delivery: CronDelivery::None,
                chain: Some(carrier_types::scheduler::ChainMeta {
                    chain_id: "pipeline-20260816-jiakao".to_string(),
                    step: 4,
                    total_steps: 5,
                }),
                enabled: true,
                created_at: Utc::now(),
                next_run: None,
                last_run: None,
            },
            true,
        );
        store.save_all(&[meta]).unwrap();

        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].job.agent_id, agent_id,
            "agent_id must round-trip losslessly (from_string v5-hashed it into a garbage id)"
        );
        assert_eq!(loaded[0].job.sender_id.as_deref(), Some("o-test@im.wechat"));
        // Chain identity (Plan A broken-chain detection) must persist too.
        assert_eq!(
            loaded[0].job.chain,
            Some(carrier_types::scheduler::ChainMeta {
                chain_id: "pipeline-20260816-jiakao".to_string(),
                step: 4,
                total_steps: 5,
            })
        );
    }
}
