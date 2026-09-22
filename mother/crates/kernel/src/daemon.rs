//! Daemon background services — watchers, heartbeat, cron tick loop, hub update checks.
//!
//! All methods live on `CarrierKernel` but are organized here for clarity.

use super::handle::SYSTEM_AGENT_ID;
use std::sync::Arc;
use tracing::{debug, info, warn};
use carrier_types::agent::{AgentId, AgentState, ScheduleMode};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::event::*;
use carrier_types::scheduler::CronJob;

use crate::kernel::CarrierKernel;
use carrier_runtime::kernel_handle::KernelHandle;

// ── Cron delivery helper ───────────────────────────────────

/// Filesystem / profile key for outbound side-effects (HTML/cover paths under
/// `workspaces/<key>/senders/...`, content.toml cache).
///
/// Must be the agent **name** (e.g. `ai-writer`), never `AgentId` UUID string.
/// Interactive bridge routes already store names; cron jobs only have UUID and
/// must resolve here — otherwise workspace paths join under a non-existent
/// `workspaces/<uuid>/`.
fn outbound_agent_key(kernel: &CarrierKernel, agent_id: AgentId) -> String {
    match kernel.registry.get(agent_id) {
        Some(entry) => entry.name,
        None => {
            warn!(
                %agent_id,
                "Cron outbound: agent not in registry; falling back to UUID \
                 (workspace paths will likely fail)"
            );
            agent_id.to_string()
        }
    }
}

/// Turn a free-form cron job name into a path/key-safe slug.
///
/// `job.name` may contain CJK, punctuation, spaces, etc. (validate only rejects
/// control chars) so agents can name jobs naturally — e.g. "发布第二篇：OpenAI
/// 硬件". But the name is interpolated into `task_id` (used as a message
/// identity/dedup key AND as the agent's output-path template `output/{tid}/`
/// in prompt_builder.rs) and into the event `type` string `cron.{name}`. Path
/// separators (`/`, `\`), `..`, ASCII `:`, spaces, and other path/identifier-
/// hostile chars would corrupt those, so replace them with `-`. CJK, letters,
/// digits, and emoji are kept (the filesystem sandbox is UTF-8 clean). The
/// original name is still used verbatim for logs/display.
fn slugify(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() {
            continue;
        }
        if matches!(
            c,
            '/' | '\\' | ':' | ' ' | '.' | '<' | '>' | '"' | '|' | '?' | '*'
        ) {
            s.push('-');
        } else {
            s.push(c);
        }
    }
    // Collapse runs of '-' and trim leading/trailing '-'.
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_dash && !out.is_empty() {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "job".to_string()
    } else {
        out
    }
}

/// Fire a single cron job (system event or agent turn), recording success/failure.
///
/// Returns `Err(reason)` on failure paths (already recorded via
/// `record_failure`) so the fire wrapper can audit the outcome.
pub(super) async fn cron_fire_job(kernel: &Arc<CarrierKernel>, job: CronJob) -> Result<(), String> {
    let job_id = job.id;
    let agent_id = job.agent_id;
    let job_name = job.name.clone();

    match &job.action {
        carrier_types::scheduler::CronAction::SystemEvent { text } => {
            tracing::debug!(job = %job_name, "Cron: firing system event");
            let payload_bytes = serde_json::to_vec(&serde_json::json!({
                "type": format!("cron.{}", slugify(&job_name)),
                "text": text,
                "job_id": job_id.to_string(),
            }))
            .unwrap_or_default();
            let event = Event::new(
                SYSTEM_AGENT_ID,
                EventTarget::Broadcast,
                EventPayload::Custom(payload_bytes),
            );
            kernel.publish_event(event).await;
            kernel.cron_scheduler.record_success(job_id);
            Ok(())
        }
        carrier_types::scheduler::CronAction::AgentTurn {
            message,
            timeout_secs,
            active_flow,
            session_label,
            ..
        } => {
            // Orphan-cron cleanup: if the agent vanished via a non-kill path
            // (workspace removed, spawn failed before registering, stale on
            // reload), silently remove the job instead of firing a doomed turn
            // that spams the user with "Agent not found" failures. kill_agent
            // already removes cron jobs (#504); this covers the remaining
            // disappearance paths. SystemEvent jobs don't need an agent, so
            // the check lives here inside the AgentTurn arm only.
            if kernel.registry.get(agent_id).is_none() {
                let _ = kernel.cron_scheduler.remove_job(job_id);
                let _ = kernel.cron_scheduler.persist();
                tracing::info!(
                    job = %job_name,
                    agent = %agent_id,
                    "Removed orphan cron job — agent no longer in registry"
                );
                return Ok(());
            }
            tracing::debug!(job = %job_name, agent = %agent_id, "Cron: firing agent turn");
            // Default to the shared agent-turn backstop (KernelConfig.
            // agent_turn_timeout_secs, default 4h) so cron turns share the same
            // daemon-hang backstop as HTTP /send and channel inbound turns (which
            // get it at the send_message_with_handle_and_blocks chokepoint). This
            // is a BACKSTOP only - the turn itself is governed by progress/stuck
            // detection, not a time budget, so long research crons are not killed
            // at 600s. 0 (or config 0) disables the backstop. Tasks needing a
            // tighter/looser bound set `timeout_secs` explicitly via cron_create.
            let timeout_s = timeout_secs.unwrap_or(kernel.config.agent_turn_timeout_secs);
            let delivery = job.delivery.clone();
            let owner_id = job.owner_id.clone();
            // Generate task_id: {job_name slug}-{YYYYMMDD}. The name is slugified
            // because task_id is used as an identity/dedup key and interpolated
            // into the agent's output-path template (`output/{tid}/`); raw name
            // chars like `/` or `:` would corrupt paths and keys.
            let task_id = format!(
                "{}-{}",
                slugify(&job_name),
                chrono::Local::now().format("%Y%m%d")
            );
            tracing::info!(job = %job_name, task_id = %task_id, "Cron: generated task_id");
            let kh: std::sync::Arc<dyn carrier_runtime::kernel_handle::KernelHandle> = kernel.clone();
            // timeout_s == 0 means "no backstop" - run unbounded (rely on stuck
            // detection + per-LLM-call stall timeout). Otherwise wrap in the
            // wall-clock backstop.
            // Chained-pipeline session isolation (CronAction::AgentTurn
            // `session_label`): pipeline steps run in their own session so
            // user chat mid-chain can't pollute them. sender_id is still
            // passed through — it drives the workspace sender paths and
            // delivery routing, only the session identity is overridden.
            let turn_fut = kernel.send_message_with_handle_and_blocks(
                agent_id,
                message,
                Some(kh),
                None,
                job.sender_id.clone(),
                None,
                job.owner_id.clone(),
                None,
                Some(task_id),
                active_flow.as_deref(),
                session_label.as_deref(),
                // Chained pipeline: expose the chain_id to the prompt builder so
                // the 任务 ID section tells the agent the pipeline identity
                // (output/{chain_id}/) apart from this turn's task_id label.
                job.chain.as_ref().map(|c| c.chain_id.as_str()),
            );
            let outcome = if timeout_s == 0 {
                Some(turn_fut.await)
            } else {
                tokio::time::timeout(std::time::Duration::from_secs(timeout_s), turn_fut)
                    .await
                    .ok()
            };
            match outcome {
                Some(Ok(result)) => {
                    // Chained-pipeline no-op guard (P2-3 follow-up): a cron turn
                    // that "completes" without producing any usable output is a
                    // silent chain-breaker — the step's one-shot job is deleted,
                    // nothing schedules the next step, and no trace remains.
                    // `end_turn` rewrites an empty response (with NO tools run)
                    // to a fixed sentinel, so matching that sentinel is exactly
                    // the "ran zero tools and got nothing" signal — no need to
                    // thread a tools_called counter through AgentLoopResult.
                    // Treat it as a failure (record + admins alert) instead of a
                    // quiet success.
                    if !result.silent && cron_turn_degenerate(&result.response) {
                        let msg = "空转：turn 无实质产出（模型空响应/降智，未执行任何工具）";
                        tracing::warn!(
                            job = %job_name,
                            agent = %agent_id,
                            iterations = result.iterations,
                            "Cron turn produced no usable output — recording failure"
                        );
                        kernel.cron_scheduler.record_failure(job_id, msg);
                        // Auto-resume the chain (silent under budget) before
                        // the diagnostic admins push below.
                        kernel.maybe_resume_chain(&job, job_id, "degenerate").await;
                        let agent_name = outbound_agent_key(kernel, agent_id);
                        let content = carrier_types::content::ContentDescriptor {
                            text: Some(format!(
                                "⚠️ 定时任务「{job_name}」（{agent_name}）空转失败：\
                                 turn 结束但无实质产出——模型空响应或降智，未执行任何工具。\
                                 若这是链式流水线的一步，链条可能已断，请检查。"
                            )),
                            ..Default::default()
                        };
                        let _ = kernel
                            .do_push_message("admins", &content, &agent_id.to_string(), "")
                            .await;
                        return Ok(());
                    }
                    // Broken-chain detection (Plan A): a NON-tail chained step
                    // completed — its successor must now be pending. The step
                    // creates the next step via `cron_create` DURING its turn,
                    // so by the time the turn returns the successor is already
                    // in the scheduler (this job itself is still listed —
                    // one-shot removal happens at record_success — so exclude
                    // it). No successor after a completed step = the chain is
                    // broken: nothing will ever fire step N+1. Alert instead of
                    // failing silently (a human-cancelled chain will also trip
                    // this — the alert says to ignore that case).
                    if let Some(chain) = &job.chain {
                        if !chain.is_tail() {
                            let has_successor = kernel
                                .cron_scheduler
                                .list_jobs(agent_id)
                                .into_iter()
                                .any(|j| {
                                    j.id != job_id
                                        && j.chain
                                            .as_ref()
                                            .is_some_and(|c| c.chain_id == chain.chain_id)
                                });
                            if !has_successor {
                                tracing::warn!(
                                    job = %job_name,
                                    agent = %agent_id,
                                    chain_id = %chain.chain_id,
                                    step = chain.step,
                                    total_steps = chain.total_steps,
                                    "Broken chain: step completed but no successor scheduled"
                                );
                                // Auto-resume: re-fire this breakpoint step
                                // (silent under budget, admins escalation at
                                // cap — handled inside maybe_resume_chain).
                                kernel
                                    .maybe_resume_chain(&job, job_id, "no-successor")
                                    .await;
                            }
                        }
                    }
                    match cron_deliver_response(
                        kernel,
                        agent_id,
                        owner_id.as_deref(),
                        job.sender_id.as_deref(),
                        &result.response,
                        &delivery,
                    )
                    .await
                    {
                        Ok(()) => {
                            tracing::info!(job = %job_name, "Cron job completed successfully");
                            // Tail step succeeded — the chain is complete,
                            // its resume budgets are never needed again.
                            if let Some(c) = &job.chain {
                                if c.is_tail() {
                                    if let Err(e) =
                                        kernel.memory.chain_resume().clear_chain(&c.chain_id)
                                    {
                                        tracing::warn!(
                                            chain_id = %c.chain_id,
                                            "Chain ledger clear failed: {e}"
                                        );
                                    }
                                }
                            }
                            kernel.cron_scheduler.record_success(job_id);
                            Ok(())
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            tracing::warn!(job = %job_name, error = %msg, "Cron job delivery failed");
                            kernel.cron_scheduler.record_failure(job_id, &msg);
                            Err(format!("delivery failed: {msg}"))
                        }
                    }
                }
                Some(Err(e)) => {
                    let err_msg = format!("{e}");
                    tracing::warn!(job = %job_name, error = %err_msg, "Cron job failed");
                    kernel.cron_scheduler.record_failure(job_id, &err_msg);
                    let failure = Err(err_msg.clone());
                    let notice = format!("⚠️ 定时任务「{}」执行失败：{}", job_name, err_msg);
                    if let Err(de) = cron_deliver_response(
                        kernel,
                        agent_id,
                        owner_id.as_deref(),
                        job.sender_id.as_deref(),
                        &notice,
                        &delivery,
                    )
                    .await
                    {
                        tracing::warn!(job = %job_name, error = %de, "Failure-notice delivery failed");
                    }
                    // Chained step: silently re-fire the breakpoint under
                    // budget (resume, not just a notice).
                    kernel.maybe_resume_chain(&job, job_id, "turn-failed").await;
                    failure
                }
                None => {
                    tracing::warn!(job = %job_name, timeout_s, "Cron job timed out");
                    kernel
                        .cron_scheduler
                        .record_failure(job_id, &format!("timed out after {timeout_s}s"));
                    let notice = format!(
                        "⚠️ 定时任务「{}」执行超时（{}秒未完成）",
                        job_name, timeout_s
                    );
                    if let Err(de) = cron_deliver_response(
                        kernel,
                        agent_id,
                        owner_id.as_deref(),
                        job.sender_id.as_deref(),
                        &notice,
                        &delivery,
                    )
                    .await
                    {
                        tracing::warn!(job = %job_name, error = %de, "Timeout-notice delivery failed");
                    }
                    kernel.maybe_resume_chain(&job, job_id, "timeout").await;
                    Err(format!("timed out after {timeout_s}s"))
                }
            }
        }
        // Scheduled fixed-content push (automation Phase 2): no LLM, no
        // session — the payload is the same ContentDescriptor shape as an
        // automation rule's task_payload, delivered via the same
        // do_push_message path (sender_channels routing + admins fan-out).
        carrier_types::scheduler::CronAction::Push {
            channel,
            bot_id,
            payload,
            target,
        } => {
            tracing::info!(job = %job_name, target = %target, channel = %channel,
                "Cron: firing scheduled push (no LLM)");
            let content = match serde_json::from_value::<carrier_types::content::ContentDescriptor>(
                payload.clone(),
            ) {
                Ok(c) => c,
                Err(e) => {
                    // validate_action rejects this shape at creation — a bad
                    // payload here means the job predates a schema drift.
                    let msg = format!("push payload is not ContentDescriptor-shaped: {e}");
                    kernel.cron_scheduler.record_failure(job_id, &msg);
                    return Err(msg);
                }
            };
            // Audience: "followers" expands to the pushable subset (the OA
            // customer-service API only delivers within 48h of the user's last
            // message — 44h leaves margin); everything else ("admins", a raw
            // openid) goes to do_push_message as-is.
            let recipients: Vec<String> = if target == "followers" {
                let since = (chrono::Utc::now() - chrono::Duration::hours(44)).to_rfc3339();
                match kernel.follower_list_pushable(channel, bot_id, &since).await {
                    Ok(list) => list,
                    Err(e) => {
                        let msg = format!("follower audience lookup failed: {e}");
                        kernel.cron_scheduler.record_failure(job_id, &msg);
                        return Err(msg);
                    }
                }
            } else {
                vec![target.clone()]
            };
            if recipients.is_empty() {
                let msg = "push to 'followers': no pushable followers in the 48h window";
                kernel.cron_scheduler.record_failure(job_id, msg);
                return Err(msg.to_string());
            }
            let mut delivered = 0usize;
            let mut failed = 0usize;
            for user in &recipients {
                match kernel
                    .do_push_message(user, &content, &agent_id.to_string(), bot_id)
                    .await
                {
                    Ok(()) => delivered += 1,
                    Err(e) => {
                        failed += 1;
                        tracing::warn!(job = %job_name, target_user = %user, error = %e,
                            "Cron push delivery failed");
                    }
                }
            }
            if delivered == 0 {
                let msg = format!(
                    "push to '{target}': all {} deliveries failed",
                    recipients.len()
                );
                kernel.cron_scheduler.record_failure(job_id, &msg);
                return Err(msg);
            }
            tracing::info!(job = %job_name, delivered, failed,
                "Cron push complete");
            if failed > 0 {
                tracing::warn!(job = %job_name, failed, total = recipients.len(),
                    "Cron push partially delivered (cold followers outside the 48h \
                     window are skipped by the sender_channels gate)");
            }
            kernel.cron_scheduler.record_success(job_id);
            Ok(())
        }
        // weixin-oa FollowerReport/PublishPoll/CommentPull arms stripped (aginx-carrier: iLink-only scope).
        carrier_types::scheduler::CronAction::FollowerReport { .. }
        | carrier_types::scheduler::CronAction::PublishPoll { .. }
        | carrier_types::scheduler::CronAction::CommentPull { .. } => {
            let msg = "weixin-oa cron actions not supported in aginx-carrier (iLink-only)".to_string();
            kernel.cron_scheduler.record_failure(job_id, &msg);
            Err(msg)
        }
    }
}

/// Deliver a cron job's agent response to the configured delivery target.
///
/// - `None`: silent — no notification sent
/// - `LastChannel`: route to the channel the sender (owner_id) most recently
///   used. Buffered for later delivery if the channel doesn't support
///   proactive push or if the send attempt fails.
/// - `Webhook`: HTTP POST to the configured URL.
pub(super) async fn cron_deliver_response(
    kernel: &Arc<CarrierKernel>,
    agent_id: AgentId,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
    response: &str,
    delivery: &carrier_types::scheduler::CronDelivery,
) -> CarrierResult<()> {
    use carrier_types::scheduler::CronDelivery;

    // Empty before markers: keep historical cron behaviour (skip all processing).
    if response.is_empty() {
        return Ok(());
    }

    // Same outbound pipeline as the interactive bridge (DELIVER + no-reply
    // suppress). Cron intentionally skips NOTIFY and WeChat sanitize.
    //
    // Use agent **name** (not UUID) for workspace/content paths — see
    // [`outbound_agent_key`].
    let sender_id = sender_id.or(owner_id).unwrap_or("");
    let (pchannel, pbot, psend_fn) = cron_publish_followup_target(kernel, sender_id);
    let deliver_fn = kernel
        .channel_deliver_fn
        .read()
        .ok()
        .and_then(|g| g.clone());
    let agent_name = outbound_agent_key(kernel, agent_id);
    let content = kernel.resolve_agent_workspace(&agent_name).and_then(|ws| {
        carrier_runtime::outbound::ContentRegistry::global().load(&agent_name, std::path::Path::new(&ws))
    });
    let kh: std::sync::Arc<dyn carrier_runtime::kernel_handle::KernelHandle> = kernel.clone();
    let out = carrier_runtime::outbound::prepare_outbound(
        response,
        carrier_runtime::outbound::OutboundCtx {
            kernel: Some(kh),
            send_fn: psend_fn,
            deliver_fn,
            content: content.as_deref(),
            channel_type: &pchannel,
            bot_id: &pbot,
            sender_id,
            process_notify: false,
            notify_routes: None,
            admin_sender_ids: &[],
            sanitize_wechat: false,
        },
    )
    .await;
    if out.suppress_text_send {
        return Ok(());
    }
    let response = out.cleaned_text.as_str();

    match delivery {
        CronDelivery::None => Ok(()),
        CronDelivery::LastChannel => {
            let sender_id = owner_id.ok_or_else(|| {
                CarrierError::Config(
                    "LastChannel delivery requires owner_id on the cron job".to_string(),
                )
            })?;
            deliver_via_last_channel(kernel, agent_id, sender_id, response).await
        }
        CronDelivery::Admins => {
            // Fan the cron result out to every admin in the agent's workspace
            // (admins.json) via the same privileged path the automation webhook
            // uses. This is delivery, not an agent tool call, so it bypasses the
            // `message_push` tool's Dangerous classification and its ephemeral
            // wechat_identity admin gate — both of which would block a scheduled
            // (async) turn. `do_push_message("admins")` resolves the workspace
            // via registry (id-or-name), routes each admin through sender_channels
            // (prefix fallback), and delivers; ≥1 success returns Ok.
            let content = carrier_types::content::ContentDescriptor {
                text: Some(response.to_string()),
                ..Default::default()
            };
            // `pbot` comes from the job sender's last-channel history and is
            // EMPTY for senderless jobs (no owner_id/sender_id) — downstream
            // push routing (sender_channels 优先 + prefix 兜底) handles the
            // empty bot case. (OpenCarrier's weixin-oa binding fallback was
            // stripped: aginx-carrier is iLink-only.)
            kernel
                .do_push_message("admins", &content, &agent_id.to_string(), &pbot)
                .await
        }
        CronDelivery::Webhook { url } => {
            tracing::debug!(url = %url, "Cron: delivering via webhook");
            carrier_types::ssrf::check_ssrf(url)?;
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| CarrierError::Network(format!("webhook client init failed: {e}")))?;
            let payload = serde_json::json!({
                "agent_id": agent_id.to_string(),
                "response": response,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            });
            let resp = client.post(url).json(&payload).send().await.map_err(|e| {
                tracing::warn!(error = %e, "Cron webhook delivery failed");
                CarrierError::Network(format!("webhook delivery failed: {e}"))
            })?;
            tracing::debug!(status = %resp.status(), "Cron webhook delivered");
            Ok(())
        }
    }
}

/// Did a cron agent turn produce usable output at all?
///
/// `end_turn` rewrites an empty LLM response into fixed sentinel text — and the
/// `模型这次没有返回内容` variant is emitted ONLY when `any_tools_executed ==
/// false` (a "已执行操作" variant is used otherwise). So matching it plus the
/// raw-empty cases detects precisely the turns that ran zero tools and got
/// nothing back (model degeneration / empty responses), without threading a
/// tools_called counter through `AgentLoopResult`. The "tools ran but final
/// text was empty" variant deliberately does NOT match — the step's work may
/// have completed, only the closing prose was lost.
fn cron_turn_degenerate(response: &str) -> bool {
    let t = response.trim();
    t.is_empty() || t == "[no response]" || t.contains("模型这次没有返回内容")
}

/// Max silent auto-resumes per `(chain_id, step)` before the daemon stops
/// re-firing and escalates to workspace admins (decline-cap lesson from the
/// Cumora stall pipeline: a converged failure won't change on the next
/// re-wake — burn-stop at a small number). Note a daemon restart that kills
/// a mid-fire turn consumes one attempt even though the step wasn't at
/// fault; keep that in mind when tuning.
const MAX_AUTO_RESUMES: u32 = 2;

/// Build the self-heal re-fire job for a broken chained step (断链自动接续):
/// a verbatim copy of the breakpoint job (agent/owner/sender/action/chain/
/// delivery) with a fresh id, a `-r{n}` name suffix, an At(+2min) schedule,
/// and a resume note appended to the turn message. The note matters for the
/// "step finished its work but never scheduled the successor" case — the
/// re-fired turn should verify-and-link, not redo the work.
fn build_resume_job(orig: &CronJob, attempts: u32, now: chrono::DateTime<chrono::Utc>) -> CronJob {
    let suffix = format!("-r{attempts}");
    // validate() caps names at 128 chars — truncate the base, keep the
    // attempt suffix intact (it is the forensic marker).
    let max_base = 128usize.saturating_sub(suffix.chars().count());
    let base: String = orig.name.chars().take(max_base).collect();
    let mut action = orig.action.clone();
    if let carrier_types::scheduler::CronAction::AgentTurn { message, .. } = &mut action {
        message.push_str(&format!(
            "\n\n[链自愈重试 第{attempts}次] 这是对上一次中断环节的自动接续。\
             若本步产物已存在且通过校验，不要重做，直接完成「触发下一步」的 cron_create。"
        ));
    }
    CronJob {
        id: carrier_types::scheduler::CronJobId::new(),
        agent_id: orig.agent_id,
        owner_id: orig.owner_id.clone(),
        sender_id: orig.sender_id.clone(),
        name: format!("{base}{suffix}"),
        enabled: true,
        schedule: carrier_types::scheduler::CronSchedule::At {
            at: now + chrono::Duration::minutes(2),
        },
        action,
        delivery: orig.delivery.clone(),
        chain: orig.chain.clone(),
        created_at: now,
        next_run: None,
        last_run: None,
    }
}

/// A chained one-shot whose fire STARTED but never recorded an outcome.
/// `due_jobs` pre-advances a due At schedule to the +100y sentinel, and both
/// `record_success`/`record_failure` remove one-shots — so a live job with a
/// far-future sentinel `next_run` and no in-flight guard is a mid-fire
/// crash/restart leftover (typically a deploy restart killing the turn).
/// Without this predicate's sweep such a job never fires again and never
/// alerts: the chain dies silently.
fn is_stranded(
    job: &CronJob,
    one_shot: bool,
    running: bool,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    one_shot
        && !running
        && job
            .next_run
            .is_some_and(|t| t > now + chrono::Duration::days(365 * 50))
}

/// Best-effort channel target for the cron outbound pipeline.
///
/// Resolves the sender's last known `(channel_type, bot_id)` so DELIVER and
/// the reply text land on the channel the user last talked to us on, plus the
/// channel send fn. When there's no known channel the fields are empty but
/// the pipeline still runs.
fn cron_publish_followup_target(
    kernel: &Arc<CarrierKernel>,
    sender_id: &str,
) -> (
    String,
    String,
    Option<carrier_runtime::plugin::bridge::ChannelSendFn>,
) {
    let last = kernel
        .memory
        .cron_delivery()
        .get_last_channel(sender_id)
        .ok()
        .flatten();
    let send_fn = kernel.channel_send_fn.read().ok().and_then(|g| g.clone());
    match last {
        Some(c) => (c.channel_type, c.bot_id, send_fn),
        None => (String::new(), String::new(), send_fn),
    }
}

/// Deliver a notification to the sender's most recent channel. Attempts a
/// proactive push first; on failure (or for channels that don't support push)
/// the notification is buffered for delivery on the next inbound message.
async fn deliver_via_last_channel(
    kernel: &Arc<CarrierKernel>,
    agent_id: AgentId,
    sender_id: &str,
    response: &str,
) -> Result<(), CarrierError> {
    let store = kernel.memory.cron_delivery();
    let last = match store
        .get_last_channel(sender_id)
        .map_err(|e| CarrierError::Internal(format!("get_last_channel failed: {e}")))?
    {
        Some(c) => c,
        None => {
            // We've never seen this sender — buffer the notification so it
            // delivers when they first send an inbound message.
            store
                .buffer_notification(
                    sender_id,
                    &agent_id.to_string(),
                    response,
                    "cron",
                    carrier_memory::cron_delivery::DEFAULT_TTL_SECS,
                )
                .map_err(|e| CarrierError::Internal(format!("buffer notification failed: {e}")))?;
            tracing::info!(sender = %sender_id, "Cron: buffered (no last channel)");
            return Ok(());
        }
    };

    // Check if the channel supports proactive push; if not, buffer directly.
    let supports = kernel
        .channel_supports_proactive_fn
        .read()
        .ok()
        .and_then(|guard| guard.as_ref().map(|f| f(&last.channel_type)))
        .unwrap_or(false);

    if !supports {
        store
            .buffer_notification(
                sender_id,
                &agent_id.to_string(),
                response,
                "cron",
                carrier_memory::cron_delivery::DEFAULT_TTL_SECS,
            )
            .map_err(|e| CarrierError::Internal(format!("buffer notification failed: {e}")))?;
        tracing::info!(
            sender = %sender_id,
            channel = %last.channel_type,
            "Cron: buffered (channel does not support proactive push)"
        );
        return Ok(());
    }

    // Try proactive push. If it fails, fall back to buffering.
    let send_fn = kernel
        .channel_send_fn
        .read()
        .ok()
        .and_then(|guard| guard.clone());
    let send_fn = match send_fn {
        Some(f) => f,
        None => {
            return Err(CarrierError::Config(
                "channel_send_fn not configured".into(),
            ));
        }
    };

    match send_fn(&last.channel_type, &last.bot_id, sender_id, response) {
        Ok(()) => {
            tracing::info!(
                sender = %sender_id,
                channel = %last.channel_type,
                "Cron: delivered via last channel"
            );
            Ok(())
        }
        Err(e) => {
            tracing::warn!(
                sender = %sender_id,
                channel = %last.channel_type,
                error = %e,
                "Cron: proactive send failed, buffering"
            );
            store
                .buffer_notification(
                    sender_id,
                    &agent_id.to_string(),
                    response,
                    "cron",
                    carrier_memory::cron_delivery::DEFAULT_TTL_SECS,
                )
                .map_err(|e| CarrierError::Internal(format!("buffer notification failed: {e}")))?;
            Ok(())
        }
    }
}

// ── Background daemon methods ──────────────────────────────

impl CarrierKernel {
    /// Reconcile per-clone self-growth cron jobs against config.
    ///
    /// For each clone: if (global `clone_lifecycle.self_growth_enabled` AND the
    /// clone's EVOLUTION.md `self_growth_enabled`), ensure exactly one
    /// `self-growth` cron exists whose message reflects the clone's current OA
    /// binding + create-cadence; otherwise remove any `self-growth` cron. The
    /// message carries `mode=learn` (always) or `mode=create app_id=<wx…>`
    /// (OA-bound clones whose last article draft is older than the cooldown).
    /// Idempotent: when the existing job already matches desired schedule +
    /// message, it's left untouched (no churn, no persist write).
    ///
    /// Called at startup (start_background_agents) and periodically from the
    /// cron tick loop (~every 60s), so config flips and new installs converge
    /// without a restart.
    /// Reseed `knowledge/format-spec.md` into every clone workspace whose
    /// stamped spec version is older than the binary's. Unlike the self-growth
    /// seed (never overwrite — the clone may own it), the format spec is
    /// system-owned: it must track the binary so the clone-creator always
    /// generates against the format the RUNNING parser accepts. The stamped
    /// HTML comment marker distinguishes system-seeded copies (version-
    /// tracked, may overwrite) from clone-authored files (never touched).
    fn reseed_format_spec(&self) {
        let marker = format!(
            "<!-- clone-format-spec {} (system-seeded; do not edit) -->",
            carrier_clone::CLONE_FORMAT_SPEC_VERSION
        );
        let desired = format!("{marker}\n{}", carrier_clone::CLONE_FORMAT_SPEC);
        for entry in self.registry.list() {
            if entry.manifest.clone_source.is_none() {
                continue;
            }
            let Some(ref workspace) = entry.manifest.workspace else {
                continue;
            };
            let spec = workspace.join("knowledge/format-spec.md");
            let current = std::fs::read_to_string(&spec).unwrap_or_default();
            // The lifecycle knowledge-compile layer prepends standard knowledge
            // frontmatter to `knowledge/*.md` files missing it, so the stamp may
            // NOT be at byte 0 — match on containment, not starts_with.
            let seeded_before = current.contains("<!-- clone-format-spec");
            let stale = !current.contains(&marker);
            if (current.is_empty() || (seeded_before && stale))
                && std::fs::write(&spec, &desired).is_ok()
            {
                tracing::debug!(
                    agent = %entry.manifest.name,
                    version = carrier_clone::CLONE_FORMAT_SPEC_VERSION,
                    "Reseeded format spec"
                );
            }
        }
    }

    /// 断链自动接续 — unified resume decision for a broken chained one-shot
    /// step. Under budget → silently re-fire the breakpoint step (the manual
    /// "从断点步建 one-shot 接续" recovery, automated; re-running step N is
    /// the only sound move because the recipe for step N+1 lives in flow
    /// prose the daemon cannot read — the resume note tells an
    /// already-completed step to verify-and-link instead of redoing work).
    /// At budget → remove any leftover job, stop re-firing, escalate to
    /// workspace admins. The alert fires exactly once: every caller has
    /// already removed (or is about to remove) the triggering job, so no
    /// detection path can re-trigger. Silent under budget — provider noise
    /// stays out of the chat (absorb, don't leak).
    async fn maybe_resume_chain(
        &self,
        job: &CronJob,
        job_id: carrier_types::scheduler::CronJobId,
        reason: &str,
    ) {
        let Some(chain) = &job.chain else { return };
        if !self.config.chain_resume_enabled {
            return;
        }
        // Only LLM turns carry chain meta in practice, and only they have
        // step work worth re-firing; the no-LLM actions are excluded by
        // construction, this match is the belt to that suspender.
        if !matches!(job.action, carrier_types::scheduler::CronAction::AgentTurn { .. }) {
            return;
        }
        // Mirror the fire path's orphan guard: a deleted agent's chain
        // cannot resume.
        if self.registry.get(job.agent_id).is_none() {
            tracing::warn!(
                chain_id = %chain.chain_id,
                "Chain resume skipped — agent no longer registered"
            );
            return;
        }

        let attempts = self
            .memory
            .chain_resume()
            .get(&chain.chain_id, chain.step)
            .unwrap_or(0);
        if attempts >= MAX_AUTO_RESUMES {
            // Circuit-break: same breakpoint, budget exhausted. Another
            // re-fire will not change the outcome — hand it to the humans.
            let _ = self.cron_scheduler.remove_job(job_id);
            if let Err(e) = self.cron_scheduler.persist() {
                warn!("Cron persist after chain-resume circuit-break failed: {e}");
            }
            let agent_name = outbound_agent_key(self, job.agent_id);
            let content = carrier_types::content::ContentDescriptor {
                text: Some(format!(
                    "🧯 链自愈熔断：流水线「{chain_id}」第 {step}/{total} 步\
                     （{job_name}，{agent_name}）已自动接续 {attempts} 次仍未推进，停止重试。\
                     请人工检查该步失败原因，并参照该流水线的接续手法从断点重建。",
                    chain_id = chain.chain_id,
                    step = chain.step,
                    total = chain.total_steps,
                    job_name = job.name,
                    agent_name = agent_name,
                )),
                ..Default::default()
            };
            if let Err(e) = self
                .do_push_message("admins", &content, &job.agent_id.to_string(), "")
                .await
            {
                warn!(
                    chain_id = %chain.chain_id,
                    "Chain-resume circuit-break alert delivery failed: {e}"
                );
            }
            return;
        }

        let attempt = self
            .memory
            .chain_resume()
            .bump(&chain.chain_id, chain.step)
            .unwrap_or(attempts + 1);
        let resume = build_resume_job(job, attempt, chrono::Utc::now());
        tracing::warn!(
            chain_id = %chain.chain_id,
            step = chain.step,
            attempt,
            reason,
            resume_job = %resume.name,
            "Chain broken — auto-resuming breakpoint step"
        );
        match self.cron_scheduler.add_job(resume.clone(), true) {
            Ok(_) => {
                // Remove the broken original AFTER the resume exists (never
                // lose both). For the stranded sweep this is essential — a
                // leftover sentinel job would re-trigger the sweep every 60s
                // and burn the whole budget in minutes. For the post-fire
                // sites it is a defensive no-op (record_failure already
                // removed the one-shot; record_success right after finds
                // nothing and no-ops).
                let _ = self.cron_scheduler.remove_job(job_id);
                if let Err(e) = self.cron_scheduler.persist() {
                    warn!("Cron persist after chain resume failed: {e}");
                }
                self.audit_log.record(
                    job.agent_id.to_string(),
                    carrier_runtime::audit::AuditAction::ChainResume,
                    format!(
                        "chain={} step={}/{} attempt={} reason={} resume_job={}",
                        chain.chain_id, chain.step, chain.total_steps, attempt, reason, resume.name
                    ),
                    "ok",
                );
            }
            Err(e) => {
                warn!(chain_id = %chain.chain_id, "Chain resume add_job failed: {e}");
            }
        }
    }

    /// ~60s sweep for stranded chained one-shots (fires that started but
    /// never recorded an outcome because the daemon died mid-turn —
    /// typically a deploy restart). Also runs once at daemon startup so
    /// restart-stranded jobs are caught immediately, before the first tick.
    async fn reconcile_chains(&self) {
        let now = chrono::Utc::now();
        for job in self.cron_scheduler.list_all_jobs() {
            if job.chain.is_none() {
                continue;
            }
            let Some(meta) = self.cron_scheduler.get_meta(job.id) else {
                continue;
            };
            if is_stranded(
                &job,
                meta.one_shot,
                meta.running.load(std::sync::atomic::Ordering::Acquire),
                now,
            ) {
                self.maybe_resume_chain(&job, job.id, "stranded-mid-fire")
                    .await;
            }
        }
    }

    fn reconcile_self_growth(&self) {
        self.reseed_format_spec();
        let global_on = self.config.clone_lifecycle.self_growth_enabled;

        // Master switch off → strip self-growth jobs from every clone.
        if !global_on {
            let mut changed = false;
            for entry in self.registry.list() {
                if entry.manifest.clone_source.is_none() {
                    continue;
                }
                for j in self.cron_scheduler.list_jobs(entry.id) {
                    if j.name == "self-growth" {
                        let _ = self.cron_scheduler.remove_job(j.id);
                        changed = true;
                    }
                }
            }
            if changed {
                let _ = self.cron_scheduler.persist();
            }
            return;
        }

        let mut changed = false;
        for entry in self.registry.list() {
            if entry.manifest.clone_source.is_none() {
                continue;
            }
            let Some(ref workspace) = entry.manifest.workspace else {
                continue;
            };
            let cfg = carrier_lifecycle::evolution_config::read_evolution_config(workspace.as_path());
            let agent_id = entry.id;
            let clone_name = entry.name.clone();

            let existing: Vec<_> = self
                .cron_scheduler
                .list_jobs(agent_id)
                .into_iter()
                .filter(|j| j.name == "self-growth")
                .collect();

            if !cfg.self_growth_enabled {
                if !existing.is_empty() {
                    for j in &existing {
                        let _ = self.cron_scheduler.remove_job(j.id);
                    }
                    changed = true;
                }
                continue;
            }

            // Enabled — compute desired schedule + message.（OA 发布模式已随
            // weixin-oa 渠道退役移除：成长只学习不出稿，需要时随 OA 渠道回归。）
            let interval_secs = cfg.self_growth_interval_hours.saturating_mul(3600).max(60);
            let message = "自主成长。mode=learn".to_string();
            let desired_schedule = carrier_types::scheduler::CronSchedule::Every {
                every_secs: interval_secs,
            };
            let desired_action = carrier_types::scheduler::CronAction::AgentTurn {
                message: message.clone(),
                model_override: None,
                timeout_secs: None,
                active_flow: Some("self-growth".to_string()),
                session_label: Some("self-growth".to_string()),
            };

            // Idempotency: if exactly one existing job matches desired, skip.
            if existing.len() == 1 {
                let j = &existing[0];
                let schedule_matches = matches!(
                    (&j.schedule, &desired_schedule),
                    (
                        carrier_types::scheduler::CronSchedule::Every { every_secs: a },
                        carrier_types::scheduler::CronSchedule::Every { every_secs: b }
                    ) if a == b
                );
                let action_matches = matches!(
                    (&j.action, &desired_action),
                    (
                        carrier_types::scheduler::CronAction::AgentTurn { message: m1, .. },
                        carrier_types::scheduler::CronAction::AgentTurn { message: m2, .. }
                    ) if m1 == m2
                );
                if schedule_matches && action_matches {
                    continue;
                }
            }

            // Remove stale, add fresh.
            for j in &existing {
                let _ = self.cron_scheduler.remove_job(j.id);
            }
            let job = carrier_types::scheduler::CronJob {
                id: carrier_types::scheduler::CronJobId::new(),
                agent_id,
                owner_id: None,
                sender_id: None,
                name: "self-growth".to_string(),
                enabled: true,
                schedule: desired_schedule,
                action: desired_action,
                delivery: carrier_types::scheduler::CronDelivery::None,
                chain: None,
                created_at: chrono::Utc::now(),
                next_run: None,
                last_run: None,
            };
            match self.cron_scheduler.add_job(job, false) {
                Ok(_) => changed = true,
                Err(e) => warn!(agent = %clone_name, error = %e, "self-growth cron add failed"),
            }
        }
        if changed {
            if let Err(e) = self.cron_scheduler.persist() {
                warn!("self-growth reconcile persist failed: {e}");
            }
        }
    }

    /// Start file watchers for clone agents to auto-compile on knowledge changes.
    fn start_clone_watchers(self: &Arc<Self>) {
        if !self.config.clone_lifecycle.evolution_enabled {
            return;
        }

        let agents = self.registry.list();
        let kernel = Arc::clone(self);

        for entry in &agents {
            let Some(ref _cs) = entry.manifest.clone_source else {
                continue;
            };
            let Some(ref workspace) = entry.manifest.workspace else {
                continue;
            };

            let config = carrier_lifecycle::evolution_config::read_evolution_config(workspace.as_path());

            if matches!(
                config.evolution_mode,
                carrier_lifecycle::evolution_config::EvolutionMode::Disabled
            ) {
                continue;
            }

            let driver = match kernel.resolve_driver(&entry.manifest) {
                Ok(d) => d,
                Err(e) => {
                    warn!(agent = %entry.name, error = %e, "No LLM driver for watcher");
                    continue;
                }
            };
            let rt_handle = tokio::runtime::Handle::current();

            let llm_call: Arc<carrier_lifecycle::watcher::LlmCallback> = Arc::new(
                move |sys: &str, user: &str, max_tokens: u32| -> anyhow::Result<String> {
                    let request = carrier_runtime::llm_driver::CompletionRequest {
                        model: String::new(),
                        messages: vec![carrier_types::message::Message {
                            role: carrier_types::message::Role::User,
                            content: carrier_types::message::MessageContent::Text(user.to_string()),
                        }],
                        tools: vec![],
                        max_tokens,
                        temperature: 0.3,
                        system: Some(sys.to_string()),
                        thinking: None,
                        extra: Default::default(),
                    };
                    // IMPORTANT: Do NOT use `rt_handle.block_on()` here.
                    // The watcher callback runs on a notify crate thread, and
                    // block_on() can deadlock if all tokio worker threads are busy.
                    // Instead, spawn the async work and wait via oneshot channel.
                    let (tx, rx) = std::sync::mpsc::channel();
                    let driver = driver.clone();
                    rt_handle.spawn(async move {
                        let result = tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            driver.complete(request),
                        )
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!("knowledge watcher LLM call timed out after 60s")
                        })
                        .and_then(|r| r.map(|r| r.text()).map_err(|e| anyhow::anyhow!("{e}")));
                        let _ = tx.send(result);
                    });
                    rx.recv_timeout(std::time::Duration::from_secs(65))
                        .map_err(|_| {
                            anyhow::anyhow!(
                                "knowledge watcher LLM call channel closed or timed out"
                            )
                        })?
                },
            );

            match carrier_lifecycle::watcher::spawn_watcher(workspace.clone(), config, llm_call, None) {
                Ok(handle) => {
                    info!(agent = %entry.name, "Started knowledge file watcher");
                    if let Ok(mut handles) = kernel.runtime.watcher_handles.lock() {
                        handles.push(handle);
                    }
                }
                Err(e) => {
                    warn!(agent = %entry.name, error = %e, "Failed to start file watcher");
                }
            }
        }
    }

    /// Iterates the agent registry and starts background tasks for agents with
    /// `Continuous`, `Periodic`, or `Proactive` schedules.
    pub fn start_background_agents(self: &Arc<Self>) {
        let agents = self.registry.list();
        let mut bg_agents: Vec<(carrier_types::agent::AgentId, String, ScheduleMode)> = Vec::new();

        for entry in &agents {
            if matches!(entry.manifest.schedule, ScheduleMode::Reactive) {
                continue;
            }
            bg_agents.push((
                entry.id,
                entry.name.clone(),
                entry.manifest.schedule.clone(),
            ));
        }

        if !bg_agents.is_empty() {
            let count = bg_agents.len();
            let kernel = Arc::clone(self);
            tokio::spawn(async move {
                for (i, (id, name, schedule)) in bg_agents.into_iter().enumerate() {
                    kernel.start_background_for_agent(id, &name, &schedule);
                    if i > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    }
                }
                info!("Started {count} background agent loop(s) (staggered)");
            });
        }

        self.start_heartbeat_monitor();

        // Periodic usage data cleanup (every 24 hours, retain 90 days)
        {
            let kernel = Arc::clone(self);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
                interval.tick().await;
                loop {
                    interval.tick().await;
                    if kernel.runtime.supervisor.is_shutting_down() {
                        break;
                    }
                    match kernel.metering.cleanup(90) {
                        Ok(removed) if removed > 0 => {
                            info!("Metering cleanup: removed {removed} old usage records");
                        }
                        Err(e) => {
                            warn!("Metering cleanup failed: {e}");
                        }
                        _ => {}
                    }
                }
            });
        }

        // Connect to configured + extension MCP servers
        let has_mcp = self
            .plugins
            .effective_mcp_servers
            .read()
            .map(|s| !s.is_empty())
            .unwrap_or(false);
        if has_mcp {
            let kernel = Arc::clone(self);
            tokio::spawn(async move {
                kernel.connect_mcp_servers().await;
                kernel.build_toolset_registry();
            });
        }

        self.start_clone_watchers();
        self.reconcile_self_growth();

        // Cron scheduler tick loop — fires due jobs every 15 seconds
        {
            let kernel = Arc::clone(self);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                let mut persist_counter = 0u32;
                let mut reconcile_counter = 0u32;
                interval.tick().await;
                // Catch restart-stranded chained one-shots immediately — a
                // deploy restart that killed a mid-fire turn leaves a job
                // whose +100y sentinel next_run would otherwise never fire
                // and never alert.
                kernel.reconcile_chains().await;
                loop {
                    interval.tick().await;
                    if kernel.runtime.supervisor.is_shutting_down() {
                        let _ = kernel.cron_scheduler.persist();
                        break;
                    }

                    // CLI（CARRIER.md §3.4-3 cron pause/resume/remove）与
                    // 其他进程直写 cron 表；enabled/存在性以 DB 为协调真源，
                    // 每 tick 对账采进内存。fire 收尾走定点写回，不会冲掉。
                    if let Err(e) = kernel.cron_scheduler.reconcile_from_db() {
                        tracing::warn!("Cron reconcile from DB failed: {e}");
                    }

                    let due = kernel.cron_scheduler.due_jobs();
                    for job in due {
                        let job_id = job.id;
                        let job_name = job.name.clone();
                        let agent_id = job.agent_id;
                        // Chain forensics in the audit trail: plain job name
                        // alone can't tie a fire back to its pipeline step.
                        let chain_note = job
                            .chain
                            .as_ref()
                            .map(|c| {
                                format!(" chain={} step={}/{}", c.chain_id, c.step, c.total_steps)
                            })
                            .unwrap_or_default();
                        let k = Arc::clone(&kernel);
                        // Detached spawn (no tick barrier): one slow job no
                        // longer stalls every other agent's crons. Re-entry
                        // safety comes from the per-job in-flight guard set
                        // in due_jobs — a due slot that lands while the
                        // previous fire still runs is skipped, not queued.
                        // The wrapper clears the guard on EVERY outcome
                        // (including panic) so a crashed fire can never
                        // leave a job permanently skipped, and records the
                        // fire in the audit chain (cron forensics).
                        tokio::spawn(async move {
                            let outcome = cron_fire_job(&k, job).await;
                            k.cron_scheduler.clear_running(job_id);
                            let (status, detail) = match &outcome {
                                Ok(()) => ("ok", format!("job={job_name}{chain_note}")),
                                Err(e) => {
                                    ("error", format!("job={job_name}{chain_note} error={e}"))
                                }
                            };
                            k.audit_log.record(
                                agent_id.to_string(),
                                carrier_runtime::audit::AuditAction::CronFire,
                                detail,
                                status,
                            );
                        });
                    }

                    persist_counter += 1;
                    if persist_counter >= 20 {
                        persist_counter = 0;
                        if let Err(e) = kernel.cron_scheduler.persist() {
                            tracing::warn!("Cron persist failed: {e}");
                        }
                        // Periodically purge expired pending notifications.
                        match kernel.memory.cron_delivery().purge_expired() {
                            Ok(0) => {}
                            Ok(n) => {
                                tracing::debug!(deleted = n, "Purged expired pending notifications")
                            }
                            Err(e) => tracing::warn!("Purge expired notifications failed: {e}"),
                        }
                        // Abandoned cap-circuited chains must not accumulate.
                        let cutoff = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
                        match kernel.memory.chain_resume().purge_stale(&cutoff) {
                            Ok(0) => {}
                            Ok(n) => {
                                tracing::debug!(
                                    deleted = n,
                                    "Purged stale chain-resume ledger rows"
                                )
                            }
                            Err(e) => tracing::warn!("Chain-resume ledger purge failed: {e}"),
                        }
                    }

                    // Reconcile self-growth crons every ~60s so config flips
                    // (EVOLUTION.md self_growth_enabled) and new installs converge
                    // without a restart. Cheap: iterates clones, reads small
                    // frontmatter files, idempotent add/remove by name.
                    reconcile_counter += 1;
                    if reconcile_counter >= 4 {
                        reconcile_counter = 0;
                        kernel.reconcile_self_growth();
                        // Stranded chained one-shots (mid-fire crash/restart).
                        kernel.reconcile_chains().await;
                    }
                }
            });
            if self.cron_scheduler.total_jobs() > 0 {
                info!(
                    "Cron scheduler active with {} job(s)",
                    self.cron_scheduler.total_jobs()
                );
            }
        }

        // Flow run expiry tick - reaps `waiting` flow runs whose `user_input`
        // deadline has passed, marking them `timed_out`. Mirrors the cron loop.
        {
            let kernel = Arc::clone(self);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                interval.tick().await; // discard immediate first tick
                loop {
                    interval.tick().await;
                    if kernel.runtime.supervisor.is_shutting_down() {
                        break;
                    }
                    let now = chrono::Utc::now().to_rfc3339();
                    match kernel.memory.flow_runs().list_expired(&now) {
                        Ok(rows) => {
                            for r in rows {
                                let completed = r.completed_steps.clone();
                                match kernel.memory.flow_runs().update_status(
                                    &r.run_id,
                                    "timed_out",
                                    &completed,
                                ) {
                                    Ok(()) => info!(
                                        run_id = %r.run_id,
                                        flow = %r.flow_name,
                                        "flow_run timed out (user_input deadline passed)"
                                    ),
                                    Err(e) => warn!(
                                        run_id = %r.run_id,
                                        error = %e,
                                        "flow_run timeout mark failed"
                                    ),
                                }
                            }
                        }
                        Err(e) => warn!(error = %e, "list_expired flow_runs failed"),
                    }
                }
            });
        }

        // Discover configured external A2A agents
        if let Some(ref a2a_config) = self.config.a2a {
            if a2a_config.enabled && !a2a_config.external_agents.is_empty() {
                let kernel = Arc::clone(self);
                let agents = a2a_config.external_agents.clone();
                tokio::spawn(async move {
                    let discovered = carrier_runtime::a2a::discover_external_agents(&agents).await;
                    if let Ok(mut store) = kernel.a2a.a2a_external_agents.lock() {
                        *store = discovered
                            .into_iter()
                            .map(|(url, card)| (url, card, std::time::Instant::now()))
                            .collect();
                    }
                });
            }
        }
    }

    /// Periodically checks running agents and publishes events for unresponsive ones.
    fn start_heartbeat_monitor(self: &Arc<Self>) {
        use crate::heartbeat::{check_agents, is_quiet_hours, HeartbeatConfig, RecoveryTracker};

        let kernel = Arc::clone(self);
        let config = HeartbeatConfig::default();
        let interval_secs = config.check_interval_secs;
        let recovery_tracker = RecoveryTracker::new();

        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(config.check_interval_secs));

            loop {
                interval.tick().await;

                if kernel.runtime.supervisor.is_shutting_down() {
                    info!("Heartbeat monitor stopping (shutdown)");
                    break;
                }

                let statuses = check_agents(&kernel.registry, &config);
                for status in &statuses {
                    if let Some(entry) = kernel.registry.get(status.agent_id) {
                        if let Some(ref auto_cfg) = entry.manifest.autonomous {
                            if let Some(ref qh) = auto_cfg.quiet_hours {
                                if is_quiet_hours(qh) {
                                    continue;
                                }
                            }
                        }
                    }

                    if status.state == AgentState::Crashed {
                        let failures = recovery_tracker.failure_count(status.agent_id);

                        if failures >= config.max_recovery_attempts {
                            if let Some(entry) = kernel.registry.get(status.agent_id) {
                                if entry.state == AgentState::Crashed {
                                    let _ = kernel
                                        .registry
                                        .set_state(status.agent_id, AgentState::Terminated);
                                    warn!(
                                        agent = %status.name,
                                        attempts = failures,
                                        "Agent exhausted all recovery attempts — marked Terminated. Manual restart required."
                                    );
                                    let event = Event::new(
                                        status.agent_id,
                                        EventTarget::System,
                                        EventPayload::System(SystemEvent::HealthCheckFailed {
                                            agent_id: status.agent_id,
                                            unresponsive_secs: status.inactive_secs as u64,
                                        }),
                                    );
                                    kernel.coordination.event_bus.publish(event).await;
                                }
                            }
                            continue;
                        }

                        if !recovery_tracker
                            .can_attempt(status.agent_id, config.recovery_cooldown_secs)
                        {
                            debug!(
                                agent = %status.name,
                                "Recovery cooldown active, skipping"
                            );
                            continue;
                        }

                        let attempt = recovery_tracker.record_attempt(status.agent_id);
                        info!(
                            agent = %status.name,
                            attempt = attempt,
                            max = config.max_recovery_attempts,
                            "Auto-recovering crashed agent (attempt {}/{})",
                            attempt,
                            config.max_recovery_attempts
                        );
                        let _ = kernel
                            .registry
                            .set_state(status.agent_id, AgentState::Running);

                        let event = Event::new(
                            status.agent_id,
                            EventTarget::System,
                            EventPayload::System(SystemEvent::HealthCheckFailed {
                                agent_id: status.agent_id,
                                unresponsive_secs: 0,
                            }),
                        );
                        kernel.coordination.event_bus.publish(event).await;
                        continue;
                    }

                    if status.state == AgentState::Running
                        && !status.unresponsive
                        && recovery_tracker.failure_count(status.agent_id) > 0
                    {
                        info!(
                            agent = %status.name,
                            "Agent recovered successfully — resetting recovery tracker"
                        );
                        recovery_tracker.reset(status.agent_id);
                    }

                    if status.unresponsive && status.state == AgentState::Running {
                        let _ = kernel
                            .registry
                            .set_state(status.agent_id, AgentState::Crashed);
                        warn!(
                            agent = %status.name,
                            inactive_secs = status.inactive_secs,
                            "Unresponsive Running agent marked as Crashed for recovery"
                        );

                        let event = Event::new(
                            status.agent_id,
                            EventTarget::System,
                            EventPayload::System(SystemEvent::HealthCheckFailed {
                                agent_id: status.agent_id,
                                unresponsive_secs: status.inactive_secs as u64,
                            }),
                        );
                        kernel.coordination.event_bus.publish(event).await;
                    }
                }
            }
        });

        info!("Heartbeat monitor started (interval: {}s)", interval_secs);
    }

    /// Start the background loop for a single agent.
    pub fn start_background_for_agent(
        self: &Arc<Self>,
        agent_id: AgentId,
        name: &str,
        schedule: &ScheduleMode,
    ) {
        let kernel = Arc::clone(self);
        self.runtime
            .background
            .start_agent(agent_id, name, schedule, move |aid, msg| {
                let k = Arc::clone(&kernel);
                tokio::spawn(async move {
                    // Background ticks are agent-autonomous (no user/sender); give
                    // them an explicit `task:autonomous` label so the session is
                    // traceable instead of falling back to an unlabeled orphan.
                    let handle: Option<std::sync::Arc<dyn carrier_runtime::kernel_handle::KernelHandle>> =
                        k.coordination
                            .self_handle
                            .get()
                            .and_then(|w| w.upgrade())
                            .map(|a| a as std::sync::Arc<dyn carrier_runtime::kernel_handle::KernelHandle>);
                    match k
                        .send_message_with_handle(
                            aid,
                            &msg,
                            handle,
                            None,
                            None,
                            None,
                            None,
                            Some("autonomous".to_string()),
                            None,
                        )
                        .await
                    {
                        Ok(_) => {}
                        Err(e) => {
                            warn!(agent_id = %aid, error = %e, "Background tick failed");
                        }
                    }
                })
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{build_resume_job, cron_turn_degenerate, is_stranded, slugify, MAX_AUTO_RESUMES};
    use crate::registry::AgentRegistry;
    use chrono::Utc;
    use std::collections::HashMap;
    use carrier_types::agent::*;

    fn entry_with(id: AgentId, name: &str) -> AgentEntry {
        AgentEntry {
            id,
            name: name.to_string(),
            manifest: AgentManifest {
                name: name.to_string(),
                display_name: String::new(),
                version: "0.1.0".to_string(),
                description: "test".to_string(),
                author: "test".to_string(),
                module: "test".to_string(),
                schedule: ScheduleMode::default(),
                model: ModelConfig::default(),
                resources: ResourceQuota::default(),
                priority: Priority::default(),
                capabilities: ManifestCapabilities::default(),
                profile: None,
                tools: HashMap::new(),
                flows: vec![],
                mcp_servers: vec![],
                max_tool_level: carrier_types::tool::PermissionLevel::Write,
                intent_classifier_enabled: None,
                default_flow: None,
                metadata: HashMap::new(),
                tags: vec![],
                autonomous: None,
                workspace: Some(std::path::PathBuf::from(format!("/tmp/workspaces/{name}"))),
                generate_identity_files: true,
                exec_policy: None,
                cli_exec: None,
                tool_allowlist: vec![],
                tool_blocklist: vec![],
                clone_source: None,
                knowledge_files: vec![],
                plugins: vec![],
                subagents: vec![],
            },
            state: AgentState::Created,
            mode: AgentMode::default(),
            created_at: Utc::now(),
            last_active: Utc::now(),
            parent: None,
            children: vec![],
            session_id: SessionId::new(),
            tags: vec![],
            identity: Default::default(),
            onboarding_completed: false,
            onboarding_completed_at: None,
        }
    }

    /// Contract: cron must resolve AgentId → name before workspace path joins.
    /// Interactive routes store names; UUID must never become the workspaces/ segment.
    #[test]
    fn outbound_agent_key_contract_name_not_uuid() {
        let id = AgentId::new();
        let reg = AgentRegistry::new();
        reg.register(entry_with(id, "ai-writer")).unwrap();
        // Same lookup outbound_agent_key uses via kernel.registry.get
        let name = reg.get(id).map(|e| e.name).expect("registered");
        assert_eq!(name, "ai-writer");
        assert_ne!(name, id.to_string());
        // Profile path segment must match interactive bridge (agent name)
        let profile_via_name = carrier_types::config::sender_data_dir(
            std::path::Path::new("/home/u/.aginx/carrier"),
            "sender@im.wechat",
            &name,
            Some("sender@im.wechat"),
        );
        let profile_via_uuid = carrier_types::config::sender_data_dir(
            std::path::Path::new("/home/u/.aginx/carrier"),
            "sender@im.wechat",
            &id.to_string(),
            Some("sender@im.wechat"),
        );
        assert!(profile_via_name.ends_with("workspaces/ai-writer/senders/sender@im.wechat"));
        assert!(!profile_via_uuid
            .to_string_lossy()
            .contains("workspaces/ai-writer/"));
    }

    /// Chained-pipeline no-op guard: the degenerate-response contract.
    ///
    /// `end_turn` emits the `模型这次没有返回内容` sentinel ONLY for turns that
    /// ran zero tools and got an empty response back — matching it (plus raw
    /// empty / `[no response]`) detects silent chain-breakers. The
    /// tools-ran-but-no-closing-prose variant must NOT match (work may be done).
    #[test]
    fn cron_turn_degenerate_matches_empty_and_sentinel_only() {
        // Degenerate: empty / whitespace / retry marker / zero-tools sentinel
        assert!(cron_turn_degenerate(""));
        assert!(cron_turn_degenerate("   \n\t "));
        assert!(cron_turn_degenerate("[no response]"));
        assert!(cron_turn_degenerate(
            "(模型这次没有返回内容,可能是服务繁忙或上下文过长。请稍后重试,或简化一下你的请求。)"
        ));
        // Healthy: real prose, even short
        assert!(!cron_turn_degenerate(
            "✅ Step 4 排版完成，正文.html 已落盘。"
        ));
        assert!(!cron_turn_degenerate("无新知。"));
        // Tools ran, closing prose lost — NOT a chain-breaker
        assert!(!cron_turn_degenerate(
            "(已执行操作,但这次没能生成回复文字。请稍后重试,或重新说一下你的需求。)"
        ));
    }

    fn chained_agent_turn_job(name: &str, message: &str) -> carrier_types::scheduler::CronJob {
        carrier_types::scheduler::CronJob {
            id: carrier_types::scheduler::CronJobId::new(),
            agent_id: AgentId::new(),
            owner_id: Some("o-owner".to_string()),
            sender_id: Some("s-sender".to_string()),
            name: name.to_string(),
            enabled: true,
            schedule: carrier_types::scheduler::CronSchedule::At {
                at: Utc::now() + chrono::Duration::minutes(2),
            },
            action: carrier_types::scheduler::CronAction::AgentTurn {
                message: message.to_string(),
                model_override: None,
                timeout_secs: Some(600),
                active_flow: Some("outline-writer".to_string()),
                session_label: Some("pipeline:test-chain".to_string()),
            },
            delivery: carrier_types::scheduler::CronDelivery::None,
            chain: Some(carrier_types::scheduler::ChainMeta {
                chain_id: "test-chain".to_string(),
                step: 2,
                total_steps: 5,
            }),
            created_at: Utc::now(),
            last_run: None,
            next_run: None,
        }
    }

    /// The resume job is the manual "从断点步建 one-shot 接续" move, automated:
    /// verbatim identity + attempt suffix + At(+2min) + self-heal note.
    #[test]
    fn build_resume_job_copies_breakpoint_with_note_and_suffix() {
        let orig =
            chained_agent_turn_job("article-writer-test-chain", "写正文。流水线ID = test-chain");
        let now = Utc::now();
        let resume = build_resume_job(&orig, 1, now);

        assert_ne!(resume.id, orig.id);
        assert_eq!(resume.name, "article-writer-test-chain-r1");
        assert!(resume.name.chars().count() <= 128);
        // Identity and chain wiring copied verbatim.
        assert_eq!(resume.agent_id, orig.agent_id);
        assert_eq!(resume.owner_id, orig.owner_id);
        assert_eq!(resume.sender_id, orig.sender_id);
        assert_eq!(resume.delivery, orig.delivery);
        assert_eq!(resume.chain, orig.chain);
        // At schedule ~2 minutes out.
        match resume.schedule {
            carrier_types::scheduler::CronSchedule::At { at } => {
                let delta = (at - now).num_seconds();
                assert!(
                    (110..=130).contains(&delta),
                    "At should be ~+2min, got {delta}s"
                );
            }
            other => panic!("resume schedule must be At, got {other:?}"),
        }
        // Message = original + self-heal note (verify-and-link, don't redo).
        match &resume.action {
            carrier_types::scheduler::CronAction::AgentTurn {
                message,
                active_flow,
                session_label,
                ..
            } => {
                assert!(message.starts_with("写正文。流水线ID = test-chain"));
                assert!(message.contains("链自愈重试 第1次"));
                assert!(message.contains("不要重做"));
                assert_eq!(active_flow.as_deref(), Some("outline-writer"));
                assert_eq!(session_label.as_deref(), Some("pipeline:test-chain"));
            }
            other => panic!("resume action must stay AgentTurn, got {other:?}"),
        }
    }

    /// Long base names must truncate (never exceed 128) while keeping the
    /// attempt suffix intact.
    #[test]
    fn build_resume_job_truncates_long_names() {
        let long_name: String = "步".repeat(130);
        let orig = chained_agent_turn_job(&long_name, "x");
        let resume = build_resume_job(&orig, 2, Utc::now());
        assert!(resume.name.chars().count() <= 128);
        assert!(resume.name.ends_with("-r2"));
    }

    /// Stranded = one-shot + far-future sentinel next_run + not running.
    /// Real schedules (even far-out At) and in-flight fires never match.
    #[test]
    fn is_stranded_matches_only_sentinel_one_shots() {
        let now = Utc::now();
        let sentinel = now + chrono::Duration::days(365 * 100);
        let mut job = chained_agent_turn_job("j", "x");
        job.next_run = Some(sentinel);
        assert!(is_stranded(&job, true, false, now)); // the exact signature
        assert!(!is_stranded(&job, true, true, now)); // still firing
        assert!(!is_stranded(&job, false, false, now)); // recurring job
        job.next_run = Some(now + chrono::Duration::minutes(2)); // real pending At
        assert!(!is_stranded(&job, true, false, now));
        job.next_run = None;
        assert!(!is_stranded(&job, true, false, now));
        // Cap sanity: the circuit-break budget the predicate feeds into.
        assert_eq!(MAX_AUTO_RESUMES, 2);
    }

    #[test]
    fn registry_resolve_accepts_uuid_or_name_for_workspace() {
        let id = AgentId::new();
        let reg = AgentRegistry::new();
        reg.register(entry_with(id, "ai-writer")).unwrap();
        let by_name = reg.resolve("ai-writer").unwrap();
        let by_uuid = reg.resolve(&id.to_string()).unwrap();
        assert_eq!(by_name.1.name, "ai-writer");
        assert_eq!(by_uuid.1.name, "ai-writer");
        assert_eq!(by_name.1.manifest.workspace, by_uuid.1.manifest.workspace);
    }

    #[test]
    fn slugify_keeps_cjk_and_strips_path_chars() {
        // The motivating case: a naturally-named Chinese job must become a safe
        // task_id / event-type segment. ASCII path-hostile chars (here the
        // space) collapse to `-`; CJK and full-width punctuation (`：`, `（）`)
        // are kept — they're UTF-8-safe in paths and harmless in event types.
        assert_eq!(
            slugify("发布第二篇：OpenAI 硬件（2026）"),
            "发布第二篇：OpenAI-硬件（2026）"
        );
    }

    #[test]
    fn slugify_neutralizes_traversal_and_separators() {
        // `/`, `\`, `..`, ASCII `:` — none may survive into a path template or
        // an event-type string.
        assert_eq!(slugify("a/../../etc"), "a-etc");
        assert_eq!(slugify("x\\y"), "x-y");
        assert_eq!(slugify("a:b"), "a-b");
        assert_eq!(slugify("v1.2"), "v1-2");
    }

    #[test]
    fn slugify_ascii_passthrough() {
        // An already-safe ASCII name (the historical whitelist form) is a no-op.
        assert_eq!(slugify("daily-report"), "daily-report");
        assert_eq!(slugify("job_42"), "job_42");
    }

    #[test]
    fn slugify_collapses_and_trims_dashes() {
        assert_eq!(slugify("a   b"), "a-b"); // spaces collapse
        assert_eq!(slugify("--weird--"), "weird"); // leading/trailing trimmed
        assert_eq!(slugify("   "), "job"); // all-hostile -> fallback
    }

}
