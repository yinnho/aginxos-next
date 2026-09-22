//! Handler for the EndTurn / StopSequence stop reasons.
//!
//! When the LLM finishes its turn, this handler processes the response:
//! parses directives, handles NO_REPLY/silent, retries empty responses,
//! strips tool call artifacts, persists the session, generates turn
//! summaries, ingests into tree memory, and fires hooks.

use super::*;
use crate::hooks::HookRegistry;
use crate::kernel_handle::KernelHandle;
use crate::llm_driver::Brain;
use crate::text_tool_recovery::strip_tool_call_artifacts;
use carrier_memory::MemorySubstrate;
use tracing::{debug, info, warn};
use carrier_types::error::CarrierError;
use carrier_types::message::{Message, TokenUsage};

/// Maximum full messages to retain in session (6 turns × 2 = 12).
pub(in crate::agent_loop) const MAX_RETAINED_MESSAGES: usize = 12;

/// Action the main loop should take after handling an EndTurn.
pub(in crate::agent_loop) enum EndTurnAction {
    /// The loop should continue (e.g. empty response retry).
    Retry,
    /// The loop should return this result to the caller.
    Complete(AgentLoopResult),
}

/// Count consecutive trailing assistant "[no response]" messages — each is a
/// previous empty-response retry marker. Used to detect sustained gateway
/// silent failures and stop retrying before the session bloats into a loop.
fn count_trailing_retries(messages: &[Message]) -> usize {
    let mut count = 0;
    for m in messages.iter().rev() {
        if matches!(m.role, carrier_types::message::Role::Assistant) {
            if matches!(&m.content, carrier_types::message::MessageContent::Text(t) if t == "[no response]")
            {
                count += 1;
            } else {
                break; // a different assistant message ends the streak
            }
        }
        // Non-assistant messages (user "Please respond" prompts, tool results)
        // between retries don't break the streak.
    }
    count
}

/// Prefix of the corrective user message pushed by the flow `output: report`
/// gate. Counting occurrences in the loop history bounds the retry attempts
/// (same pattern as `count_trailing_retries` for empty responses).
const REPORT_FIX_PREFIX: &str = "[report-invalid]";

/// How many report-gate corrections already happened this turn.
fn count_report_fixes(messages: &[Message]) -> usize {
    messages
        .iter()
        .filter(|m| {
            m.role == carrier_types::message::Role::User
                && matches!(&m.content, carrier_types::message::MessageContent::Text(t) if t.starts_with(REPORT_FIX_PREFIX))
        })
        .count()
}

/// All UUID tokens in `text`, in order of appearance. Uses the uuid crate's
/// authoritative parser (superset: hyphenated 8-4-4-4-12, 32-hex simple,
/// braced, urn forms) rather than a hand-rolled shape check - a fabricated id
/// written in 32-hex no-dash form must not slip past the fact-check below.
fn uuid_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|t| uuid::Uuid::parse_str(t).is_ok())
        .map(str::to_string)
        .collect()
}

/// Evidence fact-check (08-19 白云调图事故): article-writer cited a formatter
/// cron job UUID that was never created (cron_create had failed) and the
/// shape-only gate waved it through. When the report's `evidence` talks about
/// cron jobs (keyword hint) AND cites UUIDs, AT LEAST ONE cited id must exist
/// in the agent's live cron list - if none does, the cron claim is fabricated,
/// rejected through the same corrective-retry path as a shape violation.
///
/// At-least-one (not every-id) semantics: evidence prose routinely carries
/// non-cron UUIDs (session/message ids, chain ids) next to a genuinely created
/// job's id - requiring every token to match would bounce truthful reports
/// (2026-08-20 review). The 08-19 pattern (the ONLY cited job id doesn't
/// exist) is still caught. Known miss: one real + one fabricated id cited
/// together passes - accepted trade-off, recall of true reports first.
///
/// A one-shot job is REMOVED from the list once it fires (record_success),
/// so a truthful citation of an already-fired successor can still trip this -
/// the corrective message tells the agent to rephrase those as completed
/// instead of citing the (now unlisted) id.
async fn verify_evidence_cron_claims(
    kernel: &Arc<dyn KernelHandle>,
    report: &serde_json::Value,
    agent_id: &str,
    owner_id: Option<&str>,
) -> Option<String> {
    const CRON_HINTS: [&str; 5] = ["cron", "job", "任务", "接力", "定时"];
    let evidence_text = match report.get("evidence")? {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let lowered = evidence_text.to_lowercase();
    if !CRON_HINTS.iter().any(|h| lowered.contains(h)) {
        return None;
    }
    let uuids = uuid_tokens(&evidence_text);
    if uuids.is_empty() {
        return None;
    }
    // Fail-open on cron_list errors, but OBSERVABLY: silently swallowing the
    // error would disable the fabrication check exactly when the system is
    // unhealthy. Failing closed would burn the corrective retries on a system
    // error rather than a report problem - so log and skip instead.
    let jobs = match kernel.cron_list(agent_id, owner_id).await {
        Ok(jobs) => jobs,
        Err(e) => {
            warn!(
                error = %e,
                "report gate: cron_list unavailable - evidence cron fact-check skipped (fail-open)"
            );
            return None;
        }
    };
    let any_exists = uuids.iter().any(|u| {
        jobs.iter().any(|j| {
            j.get("id")
                .and_then(|v| v.as_str())
                .is_some_and(|id| id.eq_ignore_ascii_case(u))
        })
    });
    if any_exists {
        return None;
    }
    Some(format!(
        "evidence 声称创建了 cron 任务，但引用的所有 ID（{}）都不在当前 cron 任务列表中\
         --禁止虚报未执行成功的操作；若 cron_create 未成功，如实写 status:blocked 并说明原因；\
         若引用的一次性任务已执行完成（执行后即从列表移除），改述为已完成并去掉其 ID",
        uuids.join("、")
    ))
}

/// Verbatim side-effect marker spans in `text`: `[NOTIFY:…]…[/NOTIFY]` and
/// single-tag `[DELIVER:key|f=v]` (body ends at the first `]` not preceded by a
/// backslash, mirroring the outbound parser).
/// Used by the report gate to carry markers through the human-message swap.
/// Deduplicated, order of first appearance.
fn side_effect_marker_spans(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |span: String| {
        if !span.is_empty() && !out.contains(&span) {
            out.push(span);
        }
    };
    for (open, close) in [("[NOTIFY:", "[/NOTIFY]")] {
        let mut rest = text;
        while let Some(i) = rest.find(open) {
            let after = &rest[i..];
            match after.find(close) {
                Some(e) => {
                    push(after[..e + close.len()].to_string());
                    rest = &after[e + close.len()..];
                }
                None => break, // unterminated — the outbound parser ignores it too
            }
        }
    }
    let mut rest = text;
    while let Some(i) = rest.find("[DELIVER:") {
        let after = &rest[i..];
        let bytes = after.as_bytes();
        let mut j = 0;
        let end = loop {
            if j >= bytes.len() {
                break None;
            }
            if bytes[j] == b'\\' {
                j += 2;
                continue;
            }
            if bytes[j] == b']' {
                break Some(j);
            }
            j += 1;
        };
        match end {
            Some(e) => {
                push(after[..=e].to_string());
                rest = &after[e + 1..];
            }
            None => break,
        }
    }
    out
}

/// Handle a `StopReason::EndTurn | StopReason::StopSequence` response.
///
/// Returns an `EndTurnAction` indicating whether the loop should retry
/// (e.g. for empty responses) or complete with a result.
#[allow(clippy::too_many_arguments)]
pub(in crate::agent_loop) async fn handle_end_turn(
    response: &CompletionResponse,
    session: &mut Session,
    messages: &mut Vec<Message>,
    manifest: &AgentManifest,
    memory: &MemorySubstrate,
    kernel: Option<&Arc<dyn KernelHandle>>,
    memory_handle: Option<&Arc<dyn crate::memory_handle::MemoryHandle>>,
    brain: Option<&Arc<dyn Brain>>,
    hooks: Option<&HookRegistry>,
    on_phase: Option<&PhaseCallback>,
    session_base_len: usize,
    user_message: &str,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
    channel_type: Option<&str>,
    agent_id_str: &str,
    iteration: u32,
    total_usage: TokenUsage,
    any_tools_executed: bool,
) -> Result<EndTurnAction, CarrierError> {
    let text = response.text();

    // Parse reply directives from the streaming response text
    let (cleaned_text_s, parsed_directives_s) = crate::reply_directives::parse_directives(&text);
    let text = strip_tool_call_artifacts(&cleaned_text_s);

    // Intentional silence: `[[silent]]` directive, or whole-text no-reply
    // sentinels (`NO_REPLY`, `[no reply needed]`, `[无需回复]`, …). Same
    // matcher as the outbound delivery safety net
    // (`outbound::is_no_reply_sentinel`) so agent-loop and channel sinks share
    // one contract. Channel-facing response is empty; session keeps a stable
    // marker for prune/audit.
    if parsed_directives_s.silent || crate::outbound::is_no_reply_sentinel(&text) {
        debug!(agent = %manifest.name, "Agent chose NO_REPLY/silent — silent completion");
        // Silent turns previously produced NO INFO-level completion log, so at
        // INFO they were indistinguishable from a hung turn (2026-08-16: five
        // silent weixin-oa event turns were misdiagnosed as hangs). INFO makes
        // every silent completion visible without spamming (one line, only on
        // the sentinel path).
        info!(
            agent = %manifest.name,
            iterations = iteration + 1,
            tokens = total_usage.total(),
            "Streaming agent loop completed silently (no-reply sentinel)"
        );
        // O6: Single-track — sync loop messages before pushing the final response
        super::helpers::sync_loop_messages(messages, session, session_base_len);
        session
            .messages
            .push(Message::assistant("[no reply needed]".to_string()));
        let new_msgs = &session.messages[session_base_len..];
        memory
            .save_session_append_async(
                session.id,
                &session.agent_name,
                new_msgs,
                session.context_window_tokens,
                session.label.as_deref(),
                Some(&session.turn_summaries),
            )
            .await
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        // P1-A: a skipped attempt leaves a trace (nothing happens invisibly).
        memory.session_events_append(
            &session.agent_name,
            &session.id.0.to_string(),
            vec![carrier_memory::SessionEventKind::Silent {
                reason: if parsed_directives_s.silent {
                    "silent directive".to_string()
                } else {
                    "no_reply sentinel".to_string()
                },
            }],
        );
        return Ok(EndTurnAction::Complete(AgentLoopResult {
            response: String::new(),
            total_usage,
            iterations: iteration + 1,
            silent: true,
            directives: carrier_types::message::ReplyDirectives {
                reply_to: parsed_directives_s.reply_to,
                current_thread: parsed_directives_s.current_thread,
                silent: true,
            },
            plan: None,
        }));
    }

    // One-shot retry with sustained-failure protection: if the LLM returns
    // empty text with no tool use, try once more before accepting the empty
    // result. Triggers on first call OR when input_tokens==0 (silent gateway
    // failure — the response looks bogus and input wasn't processed).
    //
    // To avoid session bloat when the gateway is sustained-broken (each retry
    // adds two messages, and if retries keep failing we loop until max_iters),
    // count trailing "[no response]" markers already in the history and stop
    // after MAX_SILENT_RETRIES consecutive silent failures.
    if text.trim().is_empty() && response.tool_calls.is_empty() {
        let is_silent_failure = response.usage.input_tokens == 0;
        let trailing_retries = count_trailing_retries(messages);
        const MAX_SILENT_RETRIES: usize = 2; // 3 total attempts, then give up
        let exhausted = is_silent_failure && trailing_retries >= MAX_SILENT_RETRIES;
        let should_retry = (iteration == 0 || is_silent_failure) && !exhausted;
        if should_retry {
            warn!(
                agent = %manifest.name,
                iteration,
                input_tokens = response.usage.input_tokens,
                output_tokens = response.usage.output_tokens,
                silent_failure = is_silent_failure,
                trailing_retries,
                "Empty response , retrying once"
            );
            // Re-validate messages before retry — the history may have
            // broken tool_use/tool_result pairs that caused the failure.
            if is_silent_failure {
                *messages = crate::session_repair::validate_and_repair(messages);
            }
            messages.push(Message::assistant("[no response]".to_string()));
            messages.push(Message::user("Please provide your response.".to_string()));
            return Ok(EndTurnAction::Retry);
        }
        if exhausted {
            warn!(
                agent = %manifest.name,
                iteration,
                trailing_retries,
                messages_count = messages.len(),
                "Silent gateway failure persisted across {} retries; stopping to avoid session bloat, falling back",
                trailing_retries,
            );
        }
    }

    // Guard against empty response — covers both iteration 0 and post-tool cycles
    let text = if text.trim().is_empty() {
        warn!(
            agent = %manifest.name,
            iteration,
            input_tokens = total_usage.input_tokens,
            output_tokens = total_usage.output_tokens,
            messages_count = messages.len(),
            "Empty response from LLM  — guard activated"
        );
        if any_tools_executed {
            "(已执行操作,但这次没能生成回复文字。请稍后重试,或重新说一下你的需求。)".to_string()
        } else {
            "(模型这次没有返回内容,可能是服务繁忙或上下文过长。请稍后重试,或简化一下你的请求。)"
                .to_string()
        }
    } else {
        text
    };
    let mut final_response = text.clone();

    // Flow `output: report` hard gate: the final message must carry a valid
    // Ralph report (validate_step_report). Chained pipeline steps (writing
    // chain) can no longer end on free-form prose with no quality assertion.
    // On failure the agent gets one corrective retry round (bounded by
    // MAX_REPORT_RETRIES), then the turn is accepted with a warn — the gate
    // must educate, never deadlock the loop.
    if manifest
        .metadata
        .contains_key(carrier_types::flow::META_OUTPUT_REPORT)
    {
        const MAX_REPORT_RETRIES: usize = 2;
        let parsed = carrier_types::flow::extract_json_span(&final_response)
            .map(str::trim)
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
        let mut verdict = parsed
            .as_ref()
            .ok_or_else(|| "final message carries no JSON object".to_string())
            .and_then(carrier_types::flow::validate_step_report);
        // Evidence fact-check: cited cron-job UUIDs must exist. Runs only when
        // the shape verdict already passed (no point fact-checking malformed
        // reports) and a kernel handle is available.
        if verdict.is_ok() {
            if let (Some(kh), Some(report)) = (kernel, parsed.as_ref()) {
                if let Some(reason) =
                    verify_evidence_cron_claims(kh, report, agent_id_str, owner_id).await
                {
                    verdict = Err(reason);
                }
            }
        }
        match verdict {
            Ok(()) => {
                // The report JSON is for the orchestrator; chat sinks get the
                // human-readable `message` field when the agent provided one.
                // Side-effect markers ([NOTIFY]/[DELIVER]) usually live in the
                // prose OUTSIDE the JSON — carry them through the swap
                // verbatim, or their effects (admin notify, rich delivery)
                // silently never run (6b0b3a4 regression, caught 2026-08-17:
                // markers were dropped — the outbound pipeline parses markers
                // from the response text, which no longer contained them).
                let markers = side_effect_marker_spans(&final_response);
                if let Some(human) = parsed
                    .as_ref()
                    .and_then(|v| v.get("message"))
                    .and_then(|m| m.as_str())
                    .filter(|m| !m.trim().is_empty())
                {
                    final_response = if markers.is_empty() {
                        human.to_string()
                    } else {
                        format!("{human}\n\n{}", markers.join("\n"))
                    };
                }
            }
            Err(reason) if count_report_fixes(messages) < MAX_REPORT_RETRIES => {
                warn!(
                    agent = %manifest.name,
                    reason = %reason,
                    fixes = count_report_fixes(messages),
                    "Flow output:report gate rejected final message — corrective retry"
                );
                messages.push(Message::assistant(final_response.clone()));
                messages.push(Message::user(format!(
                    "{REPORT_FIX_PREFIX}你的最终回复未通过 report 校验：{reason}。\n\
                     请修正问题后重新完成本轮任务，最终回复必须以一个 JSON 对象结尾：\n\
                     {{\"status\":\"complete\",\"evidence\":\"...\",\"message\":\"给用户看的简短总结\"}}\n\
                     （若无法完成则给 {{\"status\":\"blocked\",\"blocker\":\"原因\"}}）"
                )));
                return Ok(EndTurnAction::Retry);
            }
            Err(reason) => {
                warn!(
                    agent = %manifest.name,
                    reason = %reason,
                    "Flow output:report gate still invalid after retries; accepting with warn"
                );
            }
        }
    }

    // O6: Single-track — sync loop messages before pushing the final response
    super::helpers::sync_loop_messages(messages, session, session_base_len);
    session.messages.push(Message::assistant(text));

    // Prune NO_REPLY heartbeat turns to save context budget
    crate::session_repair::prune_heartbeat_turns(&mut session.messages, 10);

    // Generate turn summary for this conversation turn.
    // Clamp to the current message count — prune_heartbeat_turns may have
    // removed messages that were included in session_base_len.
    let base = session_base_len.min(session.messages.len());
    let turn_msgs = &session.messages[base..];
    // Condensed turn (intent + outcome) for tree ingestion. None when no
    // summary was generated (brain missing or LLM failed) — the tree then
    // falls back to the raw user message so the turn is never lost.
    let mut tree_turn: Option<(String, String)> = None;
    if let Some(brain_ref) = brain {
        if let Some(mut summary) = super::helpers::generate_turn_summary(turn_msgs, brain_ref).await
        {
            summary.turn_number = session.turn_summaries.len() as u32 + 1;
            info!(
                agent = %manifest.name,
                turn = summary.turn_number,
                intent = %summary.user_intent,
                outcome = %summary.assistant_outcome,
                "Turn summary generated"
            );

            // Extract knowledge from turn summary and write to drawer
            if !summary.key_facts.is_empty() {
                if let Some(mh) = memory_handle {
                    let agent_name = &manifest.name;
                    let oid = owner_id.unwrap_or("");
                    let uid = sender_id.unwrap_or("");
                    super::knowledge::merge_key_facts(mh, agent_name, oid, uid, &summary.key_facts);
                }
            }

            tree_turn = Some((
                summary.user_intent.clone(),
                summary.assistant_outcome.clone(),
            ));
            // P2-3 absorption step 1: the generated turn summary is now also
            // an event — the sessions-row `turn_summaries` blob becomes
            // derivable from the log instead of being the only copy (it is
            // cleared on every compaction today, losing the L0 layer).
            memory.session_events_append(
                &session.agent_name,
                &session.id.0.to_string(),
                vec![carrier_memory::SessionEventKind::TurnSummaryGenerated {
                    turn_number: summary.turn_number,
                    user_intent: summary.user_intent.clone(),
                    assistant_outcome: summary.assistant_outcome.clone(),
                    key_facts: summary.key_facts.clone(),
                }],
            );
            session.turn_summaries.push(summary);
        }
    }

    // Capture new messages BEFORE trim — session_base_len becomes invalid after trim.
    // Also clamp with .min() because prune_heartbeat_turns (line 188) may have removed
    // messages that were counted in session_base_len.
    let new_msgs: Vec<Message> =
        session.messages[session_base_len.min(session.messages.len())..].to_vec();

    // Trim old messages if over retention threshold
    super::helpers::trim_oldest_turns(&mut session.messages, MAX_RETAINED_MESSAGES);

    memory
        .save_session_append_async(
            session.id,
            &session.agent_name,
            &new_msgs,
            session.context_window_tokens,
            session.label.as_deref(),
            Some(&session.turn_summaries),
        )
        .await
        .map_err(|e| CarrierError::Memory(e.to_string()))?;

    // TODO(Phase 13): Tree memory remember will be restored here.

    // Fire-and-forget tree ingestion
    if let Some(mh) = memory_handle {
        // Prefer the condensed turn summary (semantic memory: intent + outcome);
        // fall back to the raw user message when no summary was generated this
        // turn. A single message per turn avoids the canonicaliser's unstable
        // same-timestamp sort, and intent+outcome keeps assistant chatter noise
        // out of the diary.
        let ingest_content = match &tree_turn {
            Some((intent, outcome)) => {
                format!("用户意图：{intent}\n处理结果：{outcome}")
            }
            None => user_message.to_string(),
        };
        let req = carrier_types::memory_tree::IngestRequest {
            owner_id: owner_id.unwrap_or("default").to_string(),
            agent_id: session.agent_name.to_string(),
            source_kind: "chat".to_string(),
            source_id: format!(
                "{}:{}",
                channel_type.unwrap_or("api"),
                sender_id.unwrap_or("unknown")
            ),
            messages: vec![carrier_types::memory_tree::IngestMessage {
                sender: sender_id.unwrap_or("user").to_string(),
                content: ingest_content,
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
            }],
            tags: vec![channel_type.unwrap_or("api").to_string()],
            user_id: sender_id.map(|s| s.to_string()),
        };
        let mh = Arc::clone(mh);
        tokio::spawn(async move {
            if let Err(e) = mh.tree_ingest(req).await {
                tracing::warn!(error = %e, "tree_ingest failed");
            }
        });
    }

    // Notify phase: Done
    if let Some(cb) = on_phase {
        cb(LoopPhase::Done);
    }

    info!(
        agent = %manifest.name,
        iterations = iteration + 1,
        tokens = total_usage.total(),
        "Streaming agent loop completed"
    );

    // Fire AgentLoopEnd hook
    if let Some(hook_reg) = hooks {
        let ctx = crate::hooks::HookContext {
            agent_name: &manifest.name,
            agent_id: agent_id_str,
            event: carrier_types::agent::HookEvent::AgentLoopEnd,
            data: serde_json::json!({
                "iterations": iteration + 1,
                "response_length": final_response.len(),
            }),
        };
        let _ = hook_reg.fire(&ctx);
    }

    Ok(EndTurnAction::Complete(AgentLoopResult {
        response: final_response,
        total_usage,
        iterations: iteration + 1,
        silent: false,
        directives: Default::default(),
        plan: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::{side_effect_marker_spans, uuid_tokens};

    #[test]
    fn uuid_tokens_extracts_shaped_ids_only() {
        // Fabricated-citation shape (08-19 incident): prose + a bare UUID.
        let text = "已创建 formatter job 04d7518f-1234-4abc-b3f4-aabbccddeeff 接力下一步";
        let ids = uuid_tokens(text);
        assert_eq!(
            ids,
            vec!["04d7518f-1234-4abc-b3f4-aabbccddeeff".to_string()]
        );

        // Non-UUID ids (task_id shape, file names) are ignored.
        assert!(uuid_tokens("任务 daily-brief-20260820 已排期, 输出 output/x/大纲.md").is_empty());

        // Wrong group lengths / non-hex are ignored.
        assert!(uuid_tokens("aaaaaaaaaa-1234-4abc-b3f4-aabbccddeeff").is_empty());
        assert!(uuid_tokens("04d7518f-1234-4abc-b3f4-aabbccddegg").is_empty());

        // Uppercase hex is valid UUID shape; multiple ids come back in order.
        let ids = uuid_tokens(
            "AABBCCDD-0011-4052-9FEA-ABCDEF012345 和 04d7518f-1234-4abc-b3f4-aabbccddeeff",
        );
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], "AABBCCDD-0011-4052-9FEA-ABCDEF012345");

        // 32-hex simple form parses via the uuid crate (hand-rolled 8-4-4-4-12
        // shape checks missed it - a fabricated id must not slip through by
        // dropping the dashes). Braced form too (braces are separators, the
        // inner simple token parses).
        let ids = uuid_tokens("job 04d7518f12344abcb3f4aabbccddeeff 已建");
        assert_eq!(ids, vec!["04d7518f12344abcb3f4aabbccddeeff".to_string()]);
        let ids = uuid_tokens("job {04d7518f-1234-4abc-b3f4-aabbccddeeff} 已建");
        assert_eq!(
            ids,
            vec!["04d7518f-1234-4abc-b3f4-aabbccddeeff".to_string()]
        );
    }

    /// Markers must survive the report-gate human-message swap verbatim —
    /// the 6b0b3a4 regression dropped them, so a marker never reached the
    /// outbound pipeline (its effect silently never ran).
    #[test]
    fn side_effect_markers_extracted_verbatim() {
        let text = "产物已就位，正在投递～\n\n[NOTIFY:escalation]新工单[/NOTIFY]\n\n{\"status\":\"complete\",\"message\":\"已进入自动投递流程\"}";
        let spans = side_effect_marker_spans(text);
        assert_eq!(spans, vec!["[NOTIFY:escalation]新工单[/NOTIFY]".to_string()]);

        // Both marker kinds, multiple occurrences, order of appearance,
        // dedup of identical spans.
        let mixed = "[NOTIFY:escalation]投诉[/NOTIFY]\n[DELIVER:yueka|user=o1]\n[NOTIFY:escalation]投诉[/NOTIFY]";
        assert_eq!(side_effect_marker_spans(mixed).len(), 2);

        // Escaped ] inside a DELIVER body doesn't terminate the span early.
        assert_eq!(
            side_effect_marker_spans("[DELIVER:k|text=a\\]b]"),
            vec!["[DELIVER:k|text=a\\]b]".to_string()]
        );

        // No markers → empty (no re-append, human message stays alone).
        assert!(side_effect_marker_spans("普通回复，无标记").is_empty());

        // Unterminated NOTIFY is ignored, same as the outbound parser.
        assert!(side_effect_marker_spans("[NOTIFY:app]no close").is_empty());
    }
}
