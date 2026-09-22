//! Cron job scheduler engine for the Carrier kernel.
//!
//! Manages scheduled jobs (recurring and one-shot) across all agents.
//! This is separate from `scheduler.rs` which handles agent resource tracking.
//!
//! The scheduler stores jobs in a `DashMap` for concurrent access, persists
//! them to a JSON file on disk, and exposes methods for the kernel tick loop
//! to query due jobs and record outcomes.

use chrono::{Duration, Utc};
use dashmap::DashMap;
use carrier_memory::cron_store::JobMeta;
use carrier_memory::CronJobStore;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};
use carrier_types::agent::AgentId;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::scheduler::{CronJob, CronJobId, CronSchedule};

/// Maximum consecutive errors before a job is auto-disabled.
const MAX_CONSECUTIVE_ERRORS: u32 = 5;

// ---------------------------------------------------------------------------
// CronScheduler
// ---------------------------------------------------------------------------

/// Cron job scheduler — manages scheduled jobs for all agents.
///
/// Thread-safe via `DashMap`. The kernel should call [`due_jobs`] on a
/// regular interval (e.g. every 10-30 seconds) to discover jobs that need
/// to fire, then call [`record_success`] or [`record_failure`] after
/// execution completes.
pub struct CronScheduler {
    /// All tracked jobs, keyed by their unique ID.
    jobs: DashMap<CronJobId, JobMeta>,
    /// Path to the legacy persistence file (kept for fallback).
    persist_path: PathBuf,
    /// SQLite-backed store for persistence (takes priority over JSON).
    db_store: Option<Arc<CronJobStore>>,
    /// Global cap on total jobs across all agents (atomic for hot-reload).
    max_total_jobs: AtomicUsize,
}

impl CronScheduler {
    /// Create a new scheduler.
    pub fn new(home_dir: &Path, max_total_jobs: usize) -> Self {
        Self {
            jobs: DashMap::new(),
            persist_path: home_dir.join("cron_jobs.json"),
            db_store: None,
            max_total_jobs: AtomicUsize::new(max_total_jobs),
        }
    }

    /// Set the DB-backed store. Called after kernel boot once memory is available.
    pub fn set_db_store(&mut self, store: Arc<CronJobStore>) {
        self.db_store = Some(store);
    }

    /// Update the max total jobs limit (for hot-reload).
    pub fn set_max_total_jobs(&self, new_max: usize) {
        self.max_total_jobs.store(new_max, Ordering::Relaxed);
    }

    // -- Persistence --------------------------------------------------------

    /// Load persisted jobs from DB (preferred) or JSON (fallback).
    pub fn load(&self) -> CarrierResult<usize> {
        // Try DB first
        if let Some(ref store) = self.db_store {
            let metas = store.load_all()?;
            let count = metas.len();
            for meta in metas {
                self.jobs.insert(meta.job.id, meta);
            }
            info!(count, "Loaded cron jobs from database");
            return Ok(count);
        }

        // Fallback: JSON file
        if !self.persist_path.exists() {
            return Ok(0);
        }
        let data = std::fs::read_to_string(&self.persist_path)
            .map_err(|e| CarrierError::Internal(format!("Failed to read cron jobs: {e}")))?;
        let metas: Vec<JobMeta> = serde_json::from_str(&data)
            .map_err(|e| CarrierError::Internal(format!("Failed to parse cron jobs: {e}")))?;
        let count = metas.len();
        for meta in metas {
            self.jobs.insert(meta.job.id, meta);
        }
        info!(count, "Loaded cron jobs from disk");
        Ok(count)
    }

    /// Persist all jobs to DB (preferred) or JSON (fallback).
    ///
    /// DB 路径逐行 `upsert_volatile`：新 job 落全量、既有行只更新易变列，
    /// **不整表重写**——CLI 并发 pause/remove 的 DB 编辑不会被周期
    /// persist 冲掉（结构性删除走 remove_job 的定点 delete）。
    pub fn persist(&self) -> CarrierResult<()> {
        let metas: Vec<JobMeta> = self.jobs.iter().map(|r| r.value().clone()).collect();

        // Try DB first
        if let Some(ref store) = self.db_store {
            for meta in &metas {
                store.upsert_volatile(meta)?;
            }
            debug!(count = metas.len(), "Persisted cron jobs to database");
            return Ok(());
        }

        // Fallback: JSON file
        let data = serde_json::to_string_pretty(&metas)
            .map_err(|e| CarrierError::Internal(format!("Failed to serialize cron jobs: {e}")))?;
        let tmp_path = self.persist_path.with_extension("json.tmp");
        std::fs::write(&tmp_path, data.as_bytes()).map_err(|e| {
            CarrierError::Internal(format!("Failed to write cron jobs temp file: {e}"))
        })?;
        std::fs::rename(&tmp_path, &self.persist_path)
            .map_err(|e| CarrierError::Internal(format!("Failed to rename cron jobs file: {e}")))?;
        debug!(count = metas.len(), "Persisted cron jobs to disk");
        Ok(())
    }

    // -- CRUD ---------------------------------------------------------------

    /// Add a new job. Validates fields, computes the initial `next_run`,
    /// and inserts it into the scheduler.
    ///
    /// `one_shot` controls whether the job is removed after a single
    /// successful execution.
    pub fn add_job(&self, mut job: CronJob, one_shot: bool) -> CarrierResult<CronJobId> {
        // Global limit
        let max_jobs = self.max_total_jobs.load(Ordering::Relaxed);
        if self.jobs.len() >= max_jobs {
            return Err(CarrierError::Internal(format!(
                "Global cron job limit reached ({})",
                max_jobs
            )));
        }

        // Per-agent count
        let agent_count = self
            .jobs
            .iter()
            .filter(|r| r.value().job.agent_id == job.agent_id)
            .count();

        // CronJob.validate returns CarrierResult<()> (CarrierError::InvalidInput)
        job.validate(agent_count)?;

        // Compute initial next_run (a past-due `At` job fires immediately once)
        job.next_run = Some(initial_next_run(&job.schedule));

        let id = job.id;
        self.jobs.insert(id, JobMeta::new(job, one_shot));
        Ok(id)
    }

    /// Remove a job by ID. Returns the removed `CronJob`.
    /// DB 直删（定点）——daemon/CLI 共用；周期 persist 不做整表重写，
    /// 删除必须在这里落库，否则 reconcile 会把 DB 残行复活回内存。
    pub fn remove_job(&self, id: CronJobId) -> CarrierResult<CronJob> {
        let removed = self
            .jobs
            .remove(&id)
            .map(|(_, meta)| meta.job)
            .ok_or_else(|| CarrierError::Internal(format!("Cron job {id} not found")))?;
        if let Some(ref store) = self.db_store {
            if let Err(e) = store.delete(&id.to_string()) {
                warn!(job_id = %id, error = %e, "Cron job DB delete failed");
            }
        }
        Ok(removed)
    }

    /// Enable or disable a job. Re-enabling resets errors and recomputes
    /// `next_run`. DB 定点写回（enabled 列 only）——CLI 暂停/恢复即此路径。
    pub fn set_enabled(&self, id: CronJobId, enabled: bool) -> CarrierResult<()> {
        match self.jobs.get_mut(&id) {
            Some(mut meta) => {
                meta.job.enabled = enabled;
                if enabled {
                    meta.consecutive_errors = 0;
                    meta.job.next_run = Some(initial_next_run(&meta.job.schedule));
                }
                if let Some(ref store) = self.db_store {
                    if let Err(e) = store.set_enabled(&id.to_string(), enabled) {
                        warn!(job_id = %id, error = %e, "Cron job DB set_enabled failed");
                    }
                }
                Ok(())
            }
            None => Err(CarrierError::Internal(format!("Cron job {id} not found"))),
        }
    }

    // -- Queries ------------------------------------------------------------

    /// Get a single job by ID.
    #[cfg(test)]
    pub fn get_job(&self, id: CronJobId) -> Option<CronJob> {
        self.jobs.get(&id).map(|r| r.value().job.clone())
    }

    /// Get the full metadata for a job (includes `one_shot`, `last_status`,
    /// `consecutive_errors`).
    pub fn get_meta(&self, id: CronJobId) -> Option<JobMeta> {
        self.jobs.get(&id).map(|r| r.value().clone())
    }

    /// List all jobs for a specific agent.
    pub fn list_jobs(&self, agent_id: AgentId) -> Vec<CronJob> {
        self.jobs
            .iter()
            .filter(|r| r.value().job.agent_id == agent_id)
            .map(|r| r.value().job.clone())
            .collect()
    }

    /// List all jobs across all agents.
    pub fn list_all_jobs(&self) -> Vec<CronJob> {
        self.jobs.iter().map(|r| r.value().job.clone()).collect()
    }

    /// List all jobs with full runtime metadata（one_shot/last_status/
    /// consecutive_errors）——CLI 任务面（CARRIER.md §3.4-3）消费。
    pub fn list_all_metas(&self) -> Vec<JobMeta> {
        self.jobs.iter().map(|r| r.value().clone()).collect()
    }

    /// Remove all cron jobs belonging to a specific agent.
    ///
    /// Used when an agent is deleted so its cron entries don't linger as
    /// orphans pointing at a dead UUID. Returns the number of jobs removed.
    /// DB 定点直删（persist 已不整表重写，删除必须在此落库）。
    pub fn remove_agent_jobs(&self, agent_id: AgentId) -> usize {
        let ids: Vec<CronJobId> = self
            .jobs
            .iter()
            .filter(|r| r.value().job.agent_id == agent_id)
            .map(|r| *r.key())
            .collect();
        let count = ids.len();
        for id in ids {
            self.jobs.remove(&id);
            if let Some(ref store) = self.db_store {
                if let Err(e) = store.delete(&id.to_string()) {
                    warn!(job_id = %id, error = %e, "Cron job DB delete failed");
                }
            }
        }
        if count > 0 {
            info!(agent = %agent_id, count, "Removed cron jobs for deleted agent");
        }
        count
    }

    /// Total number of tracked jobs.
    pub fn total_jobs(&self) -> usize {
        self.jobs.len()
    }

    /// Return jobs whose `next_run` is at or before `now` and are enabled.
    ///
    /// **Important**: This also pre-advances each due job's `next_run` to the
    /// next scheduled time. This prevents the same job from being returned as
    /// "due" on subsequent tick iterations while it's still executing.
    pub fn due_jobs(&self) -> Vec<CronJob> {
        let now = Utc::now();
        let mut due = Vec::new();
        for mut entry in self.jobs.iter_mut() {
            let meta = entry.value_mut();
            if meta.job.enabled && meta.job.next_run.map(|t| t <= now).unwrap_or(false) {
                // Pre-advance next_run so the job won't fire again on the next
                // tick while it's still executing. Use `now` as the base so the
                // next fire time is computed strictly after the current moment.
                let next = Some(compute_next_run_after(&meta.job.schedule, now));
                if meta.running.load(std::sync::atomic::Ordering::Acquire) {
                    // Previous fire still in flight: skip this slot. The
                    // pre-advance above already moved next_run past it — the
                    // interval is skipped, not queued.
                    meta.job.next_run = next;
                    tracing::warn!(
                        job = %meta.job.name,
                        "Cron due slot skipped — previous fire still running"
                    );
                    continue;
                }
                meta.job.next_run = next;
                meta.running
                    .store(true, std::sync::atomic::Ordering::Release);
                due.push(meta.job.clone());
            }
        }
        due
    }

    /// Clear a job's in-flight guard. Called by the fire wrapper on EVERY
    /// outcome (success, failure, timeout, panic, cancellation) so a crashed
    /// fire can never leave a job permanently skipped.
    pub fn clear_running(&self, job_id: CronJobId) {
        if let Some(mut entry) = self.jobs.get_mut(&job_id) {
            entry
                .value_mut()
                .running
                .store(false, std::sync::atomic::Ordering::Release);
        }
    }

    // -- Outcome recording --------------------------------------------------

    /// Record a successful execution for a job.
    ///
    /// Updates `last_run`, resets errors, and either removes the job (if
    /// one-shot) or advances `next_run`. DB 定点写回（易变列）。
    pub fn record_success(&self, id: CronJobId) {
        // We need to check one_shot first, then potentially remove.
        let should_remove = {
            if let Some(mut meta) = self.jobs.get_mut(&id) {
                meta.job.last_run = Some(Utc::now());
                meta.last_status = Some("ok".to_string());
                meta.consecutive_errors = 0;
                // one_shot jobs get removed; recurring jobs keep the next_run
                // already pre-advanced by due_jobs() — no recompute needed.
                meta.one_shot
            } else {
                return;
            }
        };
        if should_remove {
            let _ = self.remove_job(id);
        } else if let Some(ref store) = self.db_store {
            let (last_run, next_run) = match self.jobs.get(&id) {
                Some(meta) => (meta.job.last_run, meta.job.next_run),
                None => return,
            };
            if let Some(last_run) = last_run {
                if let Err(e) = store.record_outcome(
                    &id.to_string(),
                    last_run,
                    next_run,
                    "ok",
                    0,
                    None,
                ) {
                    warn!(job_id = %id, error = %e, "Cron outcome DB write failed");
                }
            }
        }
    }

    /// Record a failed execution for a job.
    ///
    /// Increments the consecutive error counter. If it reaches
    /// [`MAX_CONSECUTIVE_ERRORS`], the job is automatically disabled
    /// （停用也定点落库——重启后仍停）。
    pub fn record_failure(&self, id: CronJobId, error_msg: &str) {
        // one_shot jobs are removed on first completion (success or failure)
        // to prevent duplicate work, especially in pipeline chains.
        let (should_remove, last_run, next_run, errors, auto_disabled, status) = {
            if let Some(mut meta) = self.jobs.get_mut(&id) {
                meta.job.last_run = Some(Utc::now());
                meta.last_status = Some(format!(
                    "error: {}",
                    carrier_types::truncate_str(error_msg, 256)
                ));
                meta.consecutive_errors += 1;
                let mut auto_disabled = false;
                if meta.consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                    warn!(
                        job_id = %id,
                        errors = meta.consecutive_errors,
                        "Auto-disabling cron job after repeated failures"
                    );
                    meta.job.enabled = false;
                    auto_disabled = true;
                } else if !meta.one_shot {
                    meta.job.next_run =
                        Some(compute_next_run_after(&meta.job.schedule, Utc::now()));
                }
                (
                    meta.one_shot,
                    meta.job.last_run,
                    meta.job.next_run,
                    meta.consecutive_errors,
                    auto_disabled,
                    meta.last_status.clone(),
                )
            } else {
                return;
            }
        };
        if should_remove {
            let _ = self.remove_job(id);
            return;
        }
        if let Some(ref store) = self.db_store {
            if let (Some(last_run), Some(status)) = (last_run, status) {
                if let Err(e) = store.record_outcome(
                    &id.to_string(),
                    last_run,
                    next_run,
                    &status,
                    errors,
                    auto_disabled.then_some(false),
                ) {
                    warn!(job_id = %id, error = %e, "Cron outcome DB write failed");
                }
            }
        }
    }

    /// 与 DB 对账（CARRIER.md §3.4-3：CLI 写开关 vs daemon 内存真源的劈叉
    /// 收敛）。daemon 每 tick 调：DB 是结构/开关的协调真源——
    /// - DB 有、内存无 → 采进内存（别的进程建的任务）
    /// - DB 无、内存有 → 从内存摘除（CLI remove 的行）
    /// - enabled 不一致 → 采 DB 值（CLI pause/resume 生效，≤一个 tick）
    ///
    /// 在飞轮（running）不动：暂停语义 = 当前这轮跑完，下一槽不再触发。
    /// 返回 (采进, 摘除, 开关变更) 计数。
    pub fn reconcile_from_db(&self) -> CarrierResult<(usize, usize, usize)> {
        let Some(ref store) = self.db_store else {
            return Ok((0, 0, 0));
        };
        let db_metas = store.load_all()?;

        let mut added = 0usize;
        let mut removed = 0usize;
        let mut toggled = 0usize;

        // DB 无、内存有 → 摘（CLI remove）。
        let db_ids: std::collections::HashSet<CronJobId> =
            db_metas.iter().map(|m| m.job.id).collect();
        let mem_ids: Vec<CronJobId> = self.jobs.iter().map(|r| *r.key()).collect();
        for id in mem_ids {
            if !db_ids.contains(&id) {
                self.jobs.remove(&id);
                removed += 1;
            }
        }

        for db_meta in db_metas {
            match self.jobs.get_mut(&db_meta.job.id) {
                Some(mut mem) => {
                    if mem.value().running.load(Ordering::Acquire) {
                        continue; // 在飞轮下轮再对
                    }
                    if mem.job.enabled != db_meta.job.enabled {
                        mem.job.enabled = db_meta.job.enabled;
                        toggled += 1;
                    }
                }
                None => {
                    self.jobs.insert(db_meta.job.id, db_meta);
                    added += 1;
                }
            }
        }

        if added + removed + toggled > 0 {
            info!(added, removed, toggled, "Cron reconciled from database");
        }
        Ok((added, removed, toggled))
    }
}

// ---------------------------------------------------------------------------
// compute_next_run
// ---------------------------------------------------------------------------

/// Compute the next fire time for a schedule, based on `now`.
///
/// - `At { at }` — returns `at` directly.
/// - `Every { every_secs }` — returns `now + every_secs`.
/// - `Cron { expr, tz }` — parses the cron expression and computes the next
///   matching time. Supports standard 5-field (`min hour dom month dow`) and
///   6-field (`sec min hour dom month dow`) formats by converting to the
///   7-field format required by the `cron` crate.
pub fn compute_next_run(schedule: &CronSchedule) -> chrono::DateTime<Utc> {
    compute_next_run_after(schedule, Utc::now())
}

/// Initial `next_run` for a newly-created or re-enabled job.
///
/// A one-shot `At` job whose time has already passed fires **immediately**
/// (once) instead of being silently dead-on-arrival. Without this, any latency
/// between the agent scheduling "fire at T" and the job being registered makes
/// `at <= now`, and `compute_next_run_after` would sentinel it to +100y (never
/// fire) — so pipeline-style schedules (e.g. "publish article 2 at 10:05")
/// silently never run. Recurring schedules use the normal computation.
///
/// Re-fire safety is unaffected: `due_jobs` pre-advances via
/// `compute_next_run_after` (At-past → +100y sentinel) and `record_success`
/// removes one-shot jobs, so a past-`At` job still fires exactly once.
fn initial_next_run(schedule: &CronSchedule) -> chrono::DateTime<Utc> {
    match schedule {
        CronSchedule::At { at } if *at <= Utc::now() => Utc::now(),
        _ => compute_next_run(schedule),
    }
}

/// Compute the next fire time for a schedule, strictly after `after`.
///
/// Uses `after + 1 second` as the base time so the `cron` crate's
/// inclusive `.after()` always returns a strictly future time. Without
/// this offset, calling `compute_next_run` right after a job fires can
/// return the same minute (or even the same second), causing the
/// scheduler to re-fire immediately.
pub fn compute_next_run_after(
    schedule: &CronSchedule,
    after: chrono::DateTime<Utc>,
) -> chrono::DateTime<Utc> {
    match schedule {
        CronSchedule::At { at } => {
            if *at > after {
                *at
            } else {
                // `at` has passed. With `initial_next_run`, a past-`At` job
                // fires immediately at create/enable time, so reaching here
                // almost always means the job ALREADY fired and `due_jobs` is
                // advancing it past its time — sentinel it to +100y so a
                // one-shot never re-fires. Debug, not warn: this is the normal
                // post-fire advance path now, not a dead-on-arrival job.
                tracing::debug!(?at, "At time passed, sentineled to never re-fire");
                after + Duration::days(36500)
            }
        }
        CronSchedule::Every { every_secs } => after + Duration::seconds(*every_secs as i64),
        CronSchedule::Cron { expr, tz } => {
            // Convert standard 5/6-field cron to 7-field for the `cron` crate.
            // Standard 5-field: min hour dom month dow
            // 6-field:          sec min hour dom month dow
            // cron crate:       sec min hour dom month dow year
            let trimmed = expr.trim();
            let fields: Vec<&str> = trimmed.split_whitespace().collect();
            let seven_field = match fields.len() {
                5 => format!("0 {trimmed} *"),
                6 => format!("{trimmed} *"),
                _ => expr.clone(),
            };

            // Add 1 second so `.after()` (inclusive) skips the current second.
            let base = after + Duration::seconds(1);

            match seven_field.parse::<cron::Schedule>() {
                Ok(sched) => {
                    // Compute the next fire time in the requested timezone so
                    // DST and local offsets are respected, then convert back to
                    // UTC for storage. Default is the server's local timezone
                    // (so a user-friendly "0 8 * * *" means 8am server-local).
                    // Pass `tz: "UTC"` explicitly to opt into UTC scheduling.
                    let next_utc = match tz.as_deref() {
                        Some("UTC") => sched.after(&base).next(),
                        Some(tz_str) if !tz_str.is_empty() => {
                            match tz_str.parse::<chrono_tz::Tz>() {
                                Ok(timezone) => {
                                    let base_local = base.with_timezone(&timezone);
                                    sched
                                        .after(&base_local)
                                        .next()
                                        .map(|dt| dt.with_timezone(&Utc))
                                }
                                Err(_) => {
                                    warn!(
                                        "Invalid timezone '{}' in cron job, falling back to server local",
                                        tz_str
                                    );
                                    let base_local = base.with_timezone(&chrono::Local);
                                    sched
                                        .after(&base_local)
                                        .next()
                                        .map(|dt| dt.with_timezone(&Utc))
                                }
                            }
                        }
                        _ => {
                            // Default: server local timezone
                            let base_local = base.with_timezone(&chrono::Local);
                            sched
                                .after(&base_local)
                                .next()
                                .map(|dt| dt.with_timezone(&Utc))
                        }
                    };
                    next_utc.unwrap_or_else(|| after + Duration::hours(1))
                }
                Err(e) => {
                    warn!("Failed to parse cron expression '{}': {}", expr, e);
                    after + Duration::hours(1)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Timelike};
    use carrier_types::scheduler::{CronAction, CronDelivery};

    /// Build a minimal valid `CronJob` with an `Every` schedule.
    fn make_job(agent_id: AgentId) -> CronJob {
        CronJob {
            id: CronJobId::new(),
            agent_id,
            owner_id: None,
            sender_id: None,
            name: "test-job".into(),
            enabled: true,
            schedule: CronSchedule::Every { every_secs: 3600 },
            action: CronAction::SystemEvent {
                text: "ping".into(),
            },
            delivery: CronDelivery::None,
            chain: None,
            created_at: Utc::now(),
            last_run: None,
            next_run: None,
        }
    }

    /// Create a scheduler backed by a temp directory.
    fn make_scheduler(max_total: usize) -> (CronScheduler, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let sched = CronScheduler::new(tmp.path(), max_total);
        (sched, tmp)
    }

    // -- test_add_job_and_list ----------------------------------------------

    #[test]
    fn test_add_job_and_list() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();
        let job = make_job(agent);

        let id = sched.add_job(job, false).unwrap();

        // Should appear in agent list
        let jobs = sched.list_jobs(agent);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, id);
        assert_eq!(jobs[0].name, "test-job");

        // Should appear in global list
        let all = sched.list_all_jobs();
        assert_eq!(all.len(), 1);

        // get_job should return it
        let fetched = sched.get_job(id).unwrap();
        assert_eq!(fetched.agent_id, agent);

        // next_run should have been computed
        assert!(fetched.next_run.is_some());
        assert_eq!(sched.total_jobs(), 1);
    }

    // -- test_remove_job ----------------------------------------------------

    #[test]
    fn test_remove_job() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();
        let job = make_job(agent);
        let id = sched.add_job(job, false).unwrap();

        let removed = sched.remove_job(id).unwrap();
        assert_eq!(removed.name, "test-job");
        assert_eq!(sched.total_jobs(), 0);

        // Removing again should fail
        assert!(sched.remove_job(id).is_err());
    }

    // -- test_add_job_global_limit ------------------------------------------

    #[test]
    fn test_add_job_global_limit() {
        let (sched, _tmp) = make_scheduler(2);
        let agent = AgentId::new();

        let j1 = make_job(agent);
        let j2 = make_job(agent);
        let j3 = make_job(agent);

        sched.add_job(j1, false).unwrap();
        sched.add_job(j2, false).unwrap();

        // Third should hit global limit
        let err = sched.add_job(j3, false).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("limit"),
            "Expected global limit error, got: {msg}"
        );
    }

    // -- test_add_job_per_agent_limit ---------------------------------------

    #[test]
    fn test_add_job_per_agent_limit() {
        // MAX_JOBS_PER_AGENT = 50 in carrier-types
        let (sched, _tmp) = make_scheduler(1000);
        let agent = AgentId::new();

        for i in 0..50 {
            let mut job = make_job(agent);
            job.name = format!("job-{i}");
            sched.add_job(job, false).unwrap();
        }

        // 51st should be rejected by validate()
        let mut overflow = make_job(agent);
        overflow.name = "overflow".into();
        let err = sched.add_job(overflow, false).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("50"),
            "Expected per-agent limit error, got: {msg}"
        );
    }

    // -- test_record_success_removes_one_shot --------------------------------

    #[test]
    fn test_record_success_removes_one_shot() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();
        let job = make_job(agent);
        let id = sched.add_job(job, true).unwrap(); // one_shot = true

        assert_eq!(sched.total_jobs(), 1);

        sched.record_success(id);

        // One-shot job should have been removed
        assert_eq!(sched.total_jobs(), 0);
        assert!(sched.get_job(id).is_none());
    }

    #[test]
    fn test_record_success_keeps_recurring() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();
        let job = make_job(agent);
        let id = sched.add_job(job, false).unwrap(); // one_shot = false

        sched.record_success(id);

        // Recurring job should still be there
        assert_eq!(sched.total_jobs(), 1);
        let meta = sched.get_meta(id).unwrap();
        assert_eq!(meta.last_status.as_deref(), Some("ok"));
        assert_eq!(meta.consecutive_errors, 0);
        assert!(meta.job.last_run.is_some());
    }

    // -- test_record_failure_auto_disable -----------------------------------

    #[test]
    fn test_record_failure_auto_disable() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();
        let job = make_job(agent);
        let id = sched.add_job(job, false).unwrap();

        // Fail MAX_CONSECUTIVE_ERRORS - 1 times: should still be enabled
        for i in 0..(MAX_CONSECUTIVE_ERRORS - 1) {
            sched.record_failure(id, &format!("error {i}"));
            let meta = sched.get_meta(id).unwrap();
            assert!(
                meta.job.enabled,
                "Job should still be enabled after {} failures",
                i + 1
            );
            assert_eq!(meta.consecutive_errors, i + 1);
        }

        // One more failure should auto-disable
        sched.record_failure(id, "final error");
        let meta = sched.get_meta(id).unwrap();
        assert!(
            !meta.job.enabled,
            "Job should be auto-disabled after {MAX_CONSECUTIVE_ERRORS} failures"
        );
        assert_eq!(meta.consecutive_errors, MAX_CONSECUTIVE_ERRORS);
        assert!(
            meta.last_status.as_ref().unwrap().starts_with("error:"),
            "last_status should record the error"
        );
    }

    // -- test_due_jobs_only_enabled -----------------------------------------

    #[test]
    fn test_due_jobs_only_enabled() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();

        // Job 1: enabled, next_run in the past
        let mut j1 = make_job(agent);
        j1.name = "enabled-due".into();
        let id1 = sched.add_job(j1, false).unwrap();

        // Job 2: disabled
        let mut j2 = make_job(agent);
        j2.name = "disabled-job".into();
        let id2 = sched.add_job(j2, false).unwrap();
        sched.set_enabled(id2, false).unwrap();

        // Force job 1's next_run to the past
        if let Some(mut meta) = sched.jobs.get_mut(&id1) {
            meta.job.next_run = Some(Utc::now() - Duration::seconds(10));
        }

        // Force job 2's next_run to the past too (but it's disabled)
        if let Some(mut meta) = sched.jobs.get_mut(&id2) {
            meta.job.next_run = Some(Utc::now() - Duration::seconds(10));
        }

        let due = sched.due_jobs();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].name, "enabled-due");
    }

    #[test]
    fn test_due_jobs_future_not_included() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();

        let job = make_job(agent);
        sched.add_job(job, false).unwrap();

        // The job was just added with next_run = now + 3600s, so it should
        // not be due yet.
        let due = sched.due_jobs();
        assert!(due.is_empty());
    }

    // -- test_set_enabled ---------------------------------------------------

    #[test]
    fn test_set_enabled() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();

        let job = make_job(agent);
        let id = sched.add_job(job, false).unwrap();

        // Disable
        sched.set_enabled(id, false).unwrap();
        let meta = sched.get_meta(id).unwrap();
        assert!(!meta.job.enabled);

        // Re-enable resets error count
        sched.record_failure(id, "ignored because disabled");
        // Actually the job is disabled so record_failure still updates it.
        // Let's first re-enable to test reset.
        sched.set_enabled(id, true).unwrap();
        let meta = sched.get_meta(id).unwrap();
        assert!(meta.job.enabled);
        assert_eq!(meta.consecutive_errors, 0);
        assert!(meta.job.next_run.is_some());

        // Non-existent ID should fail
        let fake_id = CronJobId::new();
        assert!(sched.set_enabled(fake_id, true).is_err());
    }

    // -- test_persist_and_load ----------------------------------------------

    #[test]
    fn test_persist_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = AgentId::new();

        // Create scheduler, add jobs, persist
        {
            let sched = CronScheduler::new(tmp.path(), 100);
            let mut j1 = make_job(agent);
            j1.name = "persist-a".into();
            let mut j2 = make_job(agent);
            j2.name = "persist-b".into();

            sched.add_job(j1, false).unwrap();
            sched.add_job(j2, true).unwrap(); // one_shot

            sched.persist().unwrap();
        }

        // Create a new scheduler and load from disk
        {
            let sched = CronScheduler::new(tmp.path(), 100);
            let count = sched.load().unwrap();
            assert_eq!(count, 2);
            assert_eq!(sched.total_jobs(), 2);

            let jobs = sched.list_jobs(agent);
            assert_eq!(jobs.len(), 2);

            let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
            assert!(names.contains(&"persist-a"));
            assert!(names.contains(&"persist-b"));

            // Verify one_shot flag was preserved
            let b_id = jobs.iter().find(|j| j.name == "persist-b").unwrap().id;
            let meta = sched.get_meta(b_id).unwrap();
            assert!(meta.one_shot);
        }
    }

    #[test]
    fn test_load_no_file_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let sched = CronScheduler::new(tmp.path(), 100);
        assert_eq!(sched.load().unwrap(), 0);
    }

    // -- compute_next_run ---------------------------------------------------

    #[test]
    fn test_compute_next_run_at() {
        let target = Utc::now() + Duration::hours(2);
        let schedule = CronSchedule::At { at: target };
        let next = compute_next_run(&schedule);
        assert_eq!(next, target);
    }

    #[test]
    fn test_compute_next_run_every() {
        let before = Utc::now();
        let schedule = CronSchedule::Every { every_secs: 300 };
        let next = compute_next_run(&schedule);
        let after = Utc::now();

        // Should be roughly now + 300s
        assert!(next >= before + Duration::seconds(300));
        assert!(next <= after + Duration::seconds(300));
    }

    #[test]
    fn test_compute_next_run_cron_daily() {
        let now = Utc::now();
        let schedule = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            tz: Some("UTC".into()),
        };
        let next = compute_next_run(&schedule);

        // Should be within the next 24 hours (next 09:00 UTC)
        assert!(next > now);
        assert!(next <= now + Duration::hours(24));
        assert_eq!(next.format("%M").to_string(), "00");
        assert_eq!(next.format("%H").to_string(), "09");
    }

    #[test]
    fn test_compute_next_run_cron_with_dow() {
        let now = Utc::now();
        let schedule = CronSchedule::Cron {
            expr: "30 14 * * 1-5".into(),
            tz: Some("UTC".into()),
        };
        let next = compute_next_run(&schedule);

        // Should be within the next 7 days and at 14:30 UTC
        assert!(next > now);
        assert!(next <= now + Duration::days(7));
        assert_eq!(next.format("%H:%M").to_string(), "14:30");
    }

    #[test]
    fn test_compute_next_run_cron_invalid_expr() {
        let now = Utc::now();
        let schedule = CronSchedule::Cron {
            expr: "not a cron".into(),
            tz: None,
        };
        let next = compute_next_run(&schedule);
        // Invalid expression falls back to 1 hour from now
        assert!(next > now + Duration::minutes(59));
        assert!(next <= now + Duration::minutes(61));
    }

    // -- error message truncation in record_failure -------------------------

    #[test]
    fn test_compute_next_run_after_skips_current_second() {
        // A "every 4 hours" cron: next_run should be >= 4 hours from now,
        // not in the same minute (the bug from #55).
        let schedule = CronSchedule::Cron {
            expr: "0 */4 * * *".into(),
            tz: Some("UTC".into()),
        };
        let now = Utc::now();
        let next = compute_next_run_after(&schedule, now);
        // Must be strictly after `now` and at least ~1 hour away
        // (the closest 4-hourly boundary is at least minutes away).
        assert!(next > now, "next_run should be strictly after now");
        let diff = next - now;
        assert!(
            diff.num_minutes() >= 1,
            "Expected next_run at least 1 min away, got {} seconds",
            diff.num_seconds()
        );
    }

    #[test]
    fn test_initial_next_run_past_at_fires_immediately() {
        // A one-shot `At` job whose time already passed must fire immediately
        // (next_run ~= now), not be sentineled to +100y (never fire) — that was
        // the dead-on-arrival bug for pipeline schedules like "publish at 10:05".
        let now = Utc::now();
        let past = CronSchedule::At {
            at: now - Duration::seconds(60),
        };
        let next = initial_next_run(&past);
        assert!(
            next <= Utc::now(),
            "past At job should fire immediately (now-ish), got {next}"
        );

        // A future `At` job keeps its scheduled time.
        let future = CronSchedule::At {
            at: now + Duration::hours(1),
        };
        assert_eq!(initial_next_run(&future), now + Duration::hours(1));
    }

    #[test]
    fn test_record_failure_truncates_long_error() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();
        let job = make_job(agent);
        let id = sched.add_job(job, false).unwrap();

        let long_error = "x".repeat(1000);
        sched.record_failure(id, &long_error);

        let meta = sched.get_meta(id).unwrap();
        let status = meta.last_status.unwrap();
        // "error: " is 7 chars + 256 chars of truncated message = 263 max
        assert!(
            status.len() <= 263,
            "Status should be truncated, got {} chars",
            status.len()
        );
    }

    // -- timezone-aware cron (#473) -----------------------------------------

    #[test]
    fn test_cron_tz_shifts_next_run() {
        // "0 9 * * *" in America/New_York (UTC-5 or UTC-4 depending on DST).
        // The next fire time in UTC should differ from a plain UTC "0 9 * * *".
        let schedule_utc = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            tz: None,
        };
        let schedule_ny = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            tz: Some("America/New_York".into()),
        };
        let now = Utc::now();
        let next_utc = compute_next_run_after(&schedule_utc, now);
        let next_ny = compute_next_run_after(&schedule_ny, now);

        // The New York schedule should fire at 09:00 Eastern, which is 13:00
        // or 14:00 UTC (depending on DST). In either case, it should NOT
        // equal the plain UTC 09:00 result.
        assert_ne!(
            next_utc, next_ny,
            "Timezone-aware schedule should produce a different UTC time"
        );

        // Verify the New York result, when converted to ET, shows hour 09.
        let ny_tz: chrono_tz::Tz = "America/New_York".parse().unwrap();
        let next_ny_local = next_ny.with_timezone(&ny_tz);
        assert_eq!(
            next_ny_local.hour(),
            9,
            "Expected 09:00 in America/New_York, got {:02}:{:02}",
            next_ny_local.hour(),
            next_ny_local.minute()
        );
    }

    #[test]
    fn test_cron_tz_none_defaults_to_server_local() {
        // tz: None should compute fire time in the server's local timezone.
        // We verify by comparing with a manually-computed local conversion.
        let schedule = CronSchedule::Cron {
            expr: "30 12 * * *".into(),
            tz: None,
        };
        let now = Utc::now();
        let next = compute_next_run_after(&schedule, now);

        // Convert next back to Local — should land at 12:30
        let next_local = next.with_timezone(&chrono::Local);
        use chrono::Timelike;
        assert_eq!(next_local.hour(), 12);
        assert_eq!(next_local.minute(), 30);
    }

    #[test]
    fn test_cron_tz_empty_string_defaults_to_server_local() {
        let schedule_empty = CronSchedule::Cron {
            expr: "30 12 * * *".into(),
            tz: Some(String::new()),
        };
        let schedule_none = CronSchedule::Cron {
            expr: "30 12 * * *".into(),
            tz: None,
        };
        let now = Utc::now();
        assert_eq!(
            compute_next_run_after(&schedule_empty, now),
            compute_next_run_after(&schedule_none, now)
        );
    }

    #[test]
    fn test_cron_tz_invalid_falls_back_to_utc() {
        // An invalid timezone string should fall back to UTC, not panic.
        let schedule_bad = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            tz: Some("Not/A_Timezone".into()),
        };
        let schedule_utc = CronSchedule::Cron {
            expr: "0 9 * * *".into(),
            tz: None,
        };
        let now = Utc::now();
        let next_bad = compute_next_run_after(&schedule_bad, now);
        let next_utc = compute_next_run_after(&schedule_utc, now);
        // Invalid tz falls back to UTC computation — same result.
        assert_eq!(next_bad, next_utc);
    }

    #[test]
    fn test_cron_tz_asia_shanghai() {
        // "0 8 * * *" in Asia/Shanghai (UTC+8) should fire at 00:00 UTC.
        let schedule = CronSchedule::Cron {
            expr: "0 8 * * *".into(),
            tz: Some("Asia/Shanghai".into()),
        };
        let now = Utc::now();
        let next = compute_next_run_after(&schedule, now);

        let shanghai_tz: chrono_tz::Tz = "Asia/Shanghai".parse().unwrap();
        let local = next.with_timezone(&shanghai_tz);
        assert_eq!(local.hour(), 8);
        assert_eq!(local.minute(), 0);

        // In UTC, 08:00 Shanghai = 00:00 UTC.
        assert_eq!(next.hour(), 0, "08:00 CST should be 00:00 UTC");
    }

    // -- reassign_agent_jobs (#461) -----------------------------------------

    // -- remove_agent_jobs (#504) -------------------------------------------

    #[test]
    fn test_remove_agent_jobs_basic() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();
        let other = AgentId::new();

        let mut j1 = make_job(agent);
        j1.name = "job-a".into();
        let mut j2 = make_job(agent);
        j2.name = "job-b".into();
        let mut j3 = make_job(other);
        j3.name = "job-other".into();

        sched.add_job(j1, false).unwrap();
        sched.add_job(j2, false).unwrap();
        let id3 = sched.add_job(j3, false).unwrap();

        assert_eq!(sched.total_jobs(), 3);

        let removed = sched.remove_agent_jobs(agent);
        assert_eq!(removed, 2);
        assert_eq!(sched.total_jobs(), 1);

        // The other agent's job should still exist
        assert!(sched.list_jobs(agent).is_empty());
        assert_eq!(sched.list_jobs(other).len(), 1);
        assert!(sched.get_job(id3).is_some());
    }

    #[test]
    fn test_remove_agent_jobs_no_match() {
        let (sched, _tmp) = make_scheduler(100);
        let agent = AgentId::new();

        let job = make_job(agent);
        sched.add_job(job, false).unwrap();

        // Remove for a non-existent agent
        let removed = sched.remove_agent_jobs(AgentId::new());
        assert_eq!(removed, 0);
        assert_eq!(sched.total_jobs(), 1);
    }

    #[test]
    fn test_remove_agent_jobs_persists() {
        let tmp = tempfile::tempdir().unwrap();
        let agent = AgentId::new();
        let other = AgentId::new();

        // Add jobs for two agents, remove one agent's jobs, persist
        {
            let sched = CronScheduler::new(tmp.path(), 100);
            let mut j1 = make_job(agent);
            j1.name = "doomed".into();
            let mut j2 = make_job(other);
            j2.name = "survivor".into();

            sched.add_job(j1, false).unwrap();
            sched.add_job(j2, false).unwrap();

            sched.remove_agent_jobs(agent);
            sched.persist().unwrap();
        }

        // Reload and verify
        {
            let sched = CronScheduler::new(tmp.path(), 100);
            sched.load().unwrap();
            assert_eq!(sched.total_jobs(), 1);
            assert!(sched.list_jobs(agent).is_empty());
            assert_eq!(sched.list_jobs(other).len(), 1);
        }
    }

    /// The in-flight guard: a due slot landing while the previous fire still
    /// runs is SKIPPED (not queued), and clear_running re-arms the job.
    #[test]
    fn due_jobs_skips_running_job_and_clear_rearms() {
        let (sched, _dir) = make_scheduler(10);
        let agent = AgentId::new();
        let id = sched.add_job(make_job(agent), false).unwrap();
        // Force the first slot into the past (Every{3600} starts future).
        {
            let mut entry = sched.jobs.get_mut(&id).unwrap();
            entry.value_mut().job.next_run = Some(Utc::now() - Duration::seconds(60));
        }

        // First collection fires and marks running.
        let due = sched.due_jobs();
        assert_eq!(due.len(), 1, "past-due job fires immediately");
        // While running, force next_run into the past again (simulating a
        // due slot landing mid-fire).
        {
            let mut entry = sched.jobs.get_mut(&id).unwrap();
            entry.value_mut().job.next_run = Some(Utc::now() - Duration::seconds(30));
        }
        let skipped = sched.due_jobs();
        assert!(skipped.is_empty(), "running job's due slot must be skipped");

        // Fire wrapper clears the guard; when the NEXT scheduled slot comes
        // due (forced into the past here), the job fires normally again.
        sched.clear_running(id);
        {
            let mut entry = sched.jobs.get_mut(&id).unwrap();
            entry.value_mut().job.next_run = Some(Utc::now() - Duration::seconds(30));
        }
        let again = sched.due_jobs();
        assert_eq!(again.len(), 1, "cleared job fires on next due slot");
    }
}
