//! Append-only session event log — the durable facts behind the
//! model-visible surface (dsh session-log model: "model-visible means
//! logged").
//!
//! Phase A (observational bypass, 2026-08): every surface append is ALSO an
//! event here; the sessions DB table stays authoritative until the P1-C
//! authority flip. This consolidates four half-built event-ish stores into
//! one:
//! - the post-hoc JSONL mirror (written after trimming, line-count aligned —
//!   desyncs on every compaction),
//! - the in-memory-only `turn_log` (discarded when the turn ends),
//! - the never-written `audit` `ToolInvoke` variant,
//! - per-turn `usage_events` aggregates.
//!
//! Tool calls/results live in full fidelity INSIDE the assistant/user
//! message events (blocks are not stripped here, unlike the sessions DB
//! table) — so the old "audit ToolInvoke" role is covered without a second
//! write.
//!
//! Storage: one JSONL file per session under
//! `{db_dir}/session-events/{agent}/{session_id}.jsonl` — kernel-owned,
//! independent of workspace/sender routing. Appends happen at the event
//! points and are never diffed against `session.messages`, so trimming or
//! compacting the in-memory session can never desync the log. (A SQLite
//! index table session → path + last_seq for listing/fold-resume is deferred
//! to the P1-C authority flip, where cross-session queries actually appear.)

use dashmap::DashMap;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::message::{Message, MessageContent, Role};

/// One durable session fact. Envelope: per-session monotonic `seq`, epoch-ms
/// `ts_ms`, and a typed `kind`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionEvent {
    pub seq: u64,
    pub ts_ms: i64,
    pub kind: SessionEventKind,
}

/// Durable surface facts (dsh taxonomy: only these reach the model, so only
/// these are logged). Transient per-step injections (status messages, last-run
/// restore, canonical context) are NOT durable surface — they are derived at
/// assembly time and never logged.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SessionEventKind {
    /// A turn opened for this session.
    TurnStart,
    /// A user-role message entered the durable surface (tool_result blocks
    /// included — they ride in user-role messages).
    UserMessage { content: MessageContent },
    /// An assistant message, blocks preserved in full fidelity (text,
    /// tool_use, thinking). The sessions DB table strips these at persist;
    /// the event log is the only place tool history survives.
    AssistantMessage { content: MessageContent },
    /// The turn closed with no user-visible reply (`[[silent]]` / NO_REPLY).
    /// Logged so a skipped attempt leaves a trace (nothing happens
    /// invisibly).
    Silent { reason: String },
    /// Turn envelope close. Absorbs the old in-memory `turn_log` totals.
    TurnEnd {
        iterations: u32,
        tools_called: u32,
        tool_errors: u32,
        outcome: String,
    },
    /// A compaction summarized the first `shadowed_msgs` message events that
    /// predate this event; the summary message replaces them in the folded
    /// surface. The shadowed events STAY in the log (compaction is
    /// non-destructive at the fact level — the original history remains
    /// foldable for audit/rebuild). `shadowed_tokens_est` is the chars/4
    /// estimate of the shadowed text; compaction enforces summary-est <
    /// shadowed-est before emitting ("must be smaller").
    CompactionSummary {
        summary: String,
        shadowed_msgs: u64,
        shadowed_tokens_est: u64,
    },
    /// The L0 turn summary generated for a completed turn. Projection fact:
    /// unlike the sessions-row `turn_summaries` blob (cleared by every
    /// compaction), the evented copy survives — the L0 layer stays
    /// reconstructable from the log. This is step one of absorbing the
    /// derived stores (turn_summaries / kv drawer / tree ingest) into event
    /// consumers.
    TurnSummaryGenerated {
        turn_number: u32,
        user_intent: String,
        assistant_outcome: String,
        key_facts: Vec<String>,
    },
}

/// Map a persisted message batch to surface events.
///
/// `Role::System` messages are skipped: they are transient loop-internal
/// injections (status, corrective nudges), filtered out of history at
/// assembly time — not durable surface.
///
/// Media base64 payloads are redacted (the `url` on image blocks survives):
/// raw bytes are never durable surface — they live in the workspace
/// `input/` files — and writing them into the log would balloon it by
/// megabytes per image.
pub fn message_events(msgs: &[Message]) -> Vec<SessionEventKind> {
    msgs.iter()
        .filter(|m| m.role != Role::System)
        .map(|m| {
            let content = redact_media(&m.content);
            match m.role {
                Role::Assistant => SessionEventKind::AssistantMessage { content },
                _ => SessionEventKind::UserMessage { content },
            }
        })
        .collect()
}

/// Clear base64 media payloads while keeping the block shape and any URL
/// reference (see [`message_events`]).
fn redact_media(content: &MessageContent) -> MessageContent {
    match content {
        MessageContent::Text(t) => MessageContent::Text(t.clone()),
        MessageContent::Blocks(blocks) => MessageContent::Blocks(
            blocks
                .iter()
                .map(|b| match b {
                    carrier_types::message::ContentBlock::Image {
                        media_type,
                        data,
                        url,
                    } => carrier_types::message::ContentBlock::Image {
                        media_type: media_type.clone(),
                        data: if data.is_empty() {
                            String::new()
                        } else {
                            format!("[redacted: {} bytes base64]", data.len())
                        },
                        url: url.clone(),
                    },
                    carrier_types::message::ContentBlock::Audio { media_type, data } => {
                        carrier_types::message::ContentBlock::Audio {
                            media_type: media_type.clone(),
                            data: if data.is_empty() {
                                String::new()
                            } else {
                                format!("[redacted: {} bytes base64]", data.len())
                            },
                        }
                    }
                    other => other.clone(),
                })
                .collect(),
        ),
    }
}

/// Append-only JSONL event log writer, one file per session.
///
/// Concurrency: per-session mutex serializes appends and guards the seq
/// counter; the seq is initialized from the file tail on first write per
/// process, so restarts continue the sequence (no seq reuse after crash).
pub struct SessionEventLog {
    base_dir: PathBuf,
    locks: DashMap<String, Arc<Mutex<()>>>,
    last_seq: DashMap<String, u64>,
}

impl SessionEventLog {
    pub fn new(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            locks: DashMap::new(),
            last_seq: DashMap::new(),
        }
    }

    /// Append `kinds` as sequenced events for `(agent, session)`.
    pub fn append(
        &self,
        agent_id: &str,
        session_id: &str,
        kinds: &[SessionEventKind],
    ) -> CarrierResult<()> {
        if kinds.is_empty() {
            return Ok(());
        }
        let lock = self
            .locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = lock
            .lock()
            .map_err(|e| CarrierError::Internal(format!("session event lock: {e}")))?;

        let dir = self.base_dir.join(sanitize_component(agent_id));
        std::fs::create_dir_all(&dir).map_err(|e| CarrierError::Memory(e.to_string()))?;
        let path = dir.join(format!("{session_id}.jsonl"));

        let mut seq = match self.last_seq.get(session_id) {
            Some(s) => *s,
            None => {
                let from_file = last_seq_in_file(&path).unwrap_or(0);
                self.last_seq.insert(session_id.to_string(), from_file);
                from_file
            }
        };

        let ts_ms = chrono::Utc::now().timestamp_millis();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        let mut buf = String::new();
        for kind in kinds {
            seq += 1;
            let event = SessionEvent {
                seq,
                ts_ms,
                kind: kind.clone(),
            };
            let line = serde_json::to_string(&event)
                .map_err(|e| CarrierError::Serialization(e.to_string()))?;
            buf.push_str(&line);
            buf.push('\n');
        }
        file.write_all(buf.as_bytes())
            .map_err(|e| CarrierError::Memory(e.to_string()))?;
        if let Some(mut s) = self.last_seq.get_mut(session_id) {
            *s = seq;
        } else {
            self.last_seq.insert(session_id.to_string(), seq);
        }
        Ok(())
    }

    /// Read all events for a session (fold input for P1-B).
    pub fn read(&self, agent_id: &str, session_id: &str) -> CarrierResult<Vec<SessionEvent>> {
        let path = self
            .base_dir
            .join(sanitize_component(agent_id))
            .join(format!("{session_id}.jsonl"));
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = std::fs::File::open(&path).map_err(|e| CarrierError::Memory(e.to_string()))?;
        let mut out = Vec::new();
        for line in std::io::BufReader::new(file).lines() {
            let line = line.map_err(|e| CarrierError::Memory(e.to_string()))?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(&line) {
                Ok(ev) => out.push(ev),
                // A torn final line (crash mid-append) must not kill the fold.
                Err(e) => tracing::warn!(error = %e, "session event line unparseable; skipped"),
            }
        }
        Ok(out)
    }
}

/// Fold events back into the message list they produced (the projection half
/// of the "model-visible ⟺ logged" invariant — inverse of [`message_events`]
/// up to media redaction).
///
/// Envelope events (`TurnStart`/`TurnEnd`/`Silent`) are skipped: they
/// describe turns, not surface. Every message the model ever saw therefore
/// reconstructs from the log alone.
pub fn derive_messages(events: &[SessionEvent]) -> Vec<Message> {
    events
        .iter()
        .filter_map(|ev| match &ev.kind {
            SessionEventKind::UserMessage { content } => Some(Message {
                role: Role::User,
                content: content.clone(),
            }),
            SessionEventKind::AssistantMessage { content } => Some(Message {
                role: Role::Assistant,
                content: content.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Rebuild the L0 turn-summary layer from the event log (projection replay —
/// the pollution-cure path: a corrupted or compaction-cleared
/// `turn_summaries` blob regenerates from facts instead of being lost).
///
/// Honors compaction: summaries whose turns were shadowed by a
/// `CompactionSummary` are dropped, mirroring the live path's "clear stale
/// L0 on compaction" rule. `key_facts` are also the recovery source for the
/// kv drawer; intent/outcome for tree re-ingestion.
pub fn rebuild_turn_summaries(events: &[SessionEvent]) -> Vec<carrier_types::message::TurnSummary> {
    // The last compaction shadows the first `shadowed_msgs` message events
    // (and everything older). Summaries generated before that point are
    // stale by the same rule the live path applies.
    let compaction = events.iter().rev().find_map(|ev| match &ev.kind {
        SessionEventKind::CompactionSummary { shadowed_msgs, .. } => Some((ev.seq, *shadowed_msgs)),
        _ => None,
    });
    let mut cutoff_seq = 0u64;
    if let Some((cseq, shadowed)) = compaction {
        let mut msg_count = 0u64;
        for ev in events {
            if matches!(
                ev.kind,
                SessionEventKind::UserMessage { .. } | SessionEventKind::AssistantMessage { .. }
            ) && ev.seq <= cseq
            {
                msg_count += 1;
                if msg_count >= shadowed {
                    cutoff_seq = cseq;
                    break;
                }
            }
        }
    }

    events
        .iter()
        .filter(|ev| ev.seq > cutoff_seq)
        .filter_map(|ev| match &ev.kind {
            SessionEventKind::TurnSummaryGenerated {
                turn_number,
                user_intent,
                assistant_outcome,
                key_facts,
            } => Some(carrier_types::message::TurnSummary {
                turn_number: *turn_number,
                // Replay timestamp: the event's own time — the original
                // generation time is the closest fact we have.
                timestamp: chrono::DateTime::from_timestamp_millis(ev.ts_ms)
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_default(),
                user_intent: user_intent.clone(),
                assistant_outcome: assistant_outcome.clone(),
                // tools_used was never evented (metadata, not content) — replay
                // leaves it empty rather than fabricating.
                tools_used: Vec::new(),
                key_facts: key_facts.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Fold the durable surface with the load-time projection (P1-C authority
/// flip): what the model sees on the next turn.
///
/// Rules:
/// - the LAST `CompactionSummary` wins: the first `shadowed_msgs` message
///   events before it are replaced by the summary user-message; message
///   events after the compaction point are unaffected;
/// - tool blocks render as placeholders (`strip_tool_history` — the same
///   rendering the sessions DB row always applied, now a projection rule
///   instead of a destructive persist-time strip);
/// - envelope events are skipped.
///
/// Older compactions are naturally covered: a later summary shadows a prefix
/// that includes everything the earlier one summarized.
pub fn fold_surface(events: &[SessionEvent]) -> Vec<Message> {
    let (summary, summary_seq, shadowed) = events
        .iter()
        .rev()
        .find_map(|ev| match &ev.kind {
            SessionEventKind::CompactionSummary {
                summary,
                shadowed_msgs,
                ..
            } => Some((summary.clone(), ev.seq, *shadowed_msgs)),
            _ => None,
        })
        .map(|(s, seq, n)| (Some(s), seq, n))
        .unwrap_or((None, 0, 0));

    let mut msg_events_before = 0u64;
    let mut msgs = Vec::new();
    for ev in events {
        let (role, content) = match &ev.kind {
            SessionEventKind::UserMessage { content } => (Role::User, content.clone()),
            SessionEventKind::AssistantMessage { content } => (Role::Assistant, content.clone()),
            _ => continue,
        };
        if ev.seq <= summary_seq {
            msg_events_before += 1;
            if msg_events_before <= shadowed {
                continue; // shadowed by the compaction summary
            }
        }
        msgs.push(Message { role, content });
    }

    let rendered = crate::session::strip_tool_history(&msgs);
    match summary {
        Some(s) if !s.is_empty() => {
            let mut out = Vec::with_capacity(rendered.len() + 1);
            out.push(Message {
                role: Role::User,
                // Same marked shape the live path constructs — re-compaction
                // peels the summary by this prefix (carrier_types::message).
                content: MessageContent::Text(format!(
                    "{}\n{}",
                    carrier_types::message::SESSION_SUMMARY_PREFIX,
                    s
                )),
            });
            out.extend(rendered);
            out
        }
        _ => rendered,
    }
}

/// Read the seq of the last complete line, or 0 for a fresh/absent file.
/// Seeks near the tail instead of reading the whole file.
fn last_seq_in_file(path: &std::path::Path) -> Option<u64> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.seek(SeekFrom::End(0)).ok()?;
    let window = len.min(8192);
    file.seek(SeekFrom::Start(len - window)).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let mut tail = String::new();
    reader.read_to_string(&mut tail).ok()?;
    // Reverse: the very last line may be torn (crash mid-append) — fall back
    // to the previous complete line instead of resetting the seq to 0.
    tail.lines()
        .rev()
        .filter(|l| !l.trim().is_empty())
        .find_map(|l| serde_json::from_str::<SessionEvent>(l).ok())
        .map(|ev| ev.seq)
}

/// Path components must not traverse (`..`, `/`). Agent names are kebab-case
/// by construction; this is defense in depth.
fn sanitize_component(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn append_assigns_monotonic_seq_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let log = SessionEventLog::new(dir.path().to_path_buf());
        log.append(
            "agent-a",
            "sess-1",
            &[
                SessionEventKind::TurnStart,
                SessionEventKind::UserMessage {
                    content: MessageContent::Text("hi".into()),
                },
            ],
        )
        .unwrap();
        log.append(
            "agent-a",
            "sess-1",
            &[SessionEventKind::Silent {
                reason: "test".into(),
            }],
        )
        .unwrap();

        let events = log.read("agent-a", "sess-1").unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!((events[0].seq, events[2].seq), (1, 3));

        // Simulate a restart: a fresh log over the same dir continues the seq.
        drop(log);
        let log2 = SessionEventLog::new(dir.path().to_path_buf());
        log2.append(
            "agent-a",
            "sess-1",
            &[SessionEventKind::TurnEnd {
                iterations: 1,
                tools_called: 0,
                tool_errors: 0,
                outcome: "complete".into(),
            }],
        )
        .unwrap();
        let events = log2.read("agent-a", "sess-1").unwrap();
        assert_eq!(events.last().unwrap().seq, 4);
    }

    #[test]
    fn message_events_skip_system_and_map_roles() {
        let msgs = vec![
            user_msg("hello"),
            Message::system("transient injection"),
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("answer".into()),
            },
        ];
        let events = message_events(&msgs);
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], SessionEventKind::UserMessage { .. }));
        assert!(matches!(
            events[1],
            SessionEventKind::AssistantMessage { .. }
        ));
    }

    #[test]
    fn tool_blocks_survive_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let log = SessionEventLog::new(dir.path().to_path_buf());
        let content = MessageContent::Blocks(vec![carrier_types::message::ContentBlock::ToolUse {
            id: "tu_1".into(),
            name: "file_read".into(),
            input: serde_json::json!({"path": "大纲.md"}),
            provider_metadata: None,
        }]);
        log.append(
            "agent-a",
            "sess-t",
            &[SessionEventKind::AssistantMessage { content }],
        )
        .unwrap();
        let events = log.read("agent-a", "sess-t").unwrap();
        match &events[0].kind {
            SessionEventKind::AssistantMessage { content } => {
                assert!(matches!(content, MessageContent::Blocks(_)));
            }
            other => panic!("expected AssistantMessage, got {other:?}"),
        }
    }

    #[test]
    fn sanitize_blocks_traversal() {
        // '.' and '/' are both outside the allowed set — the result cannot
        // traverse.
        assert_eq!(sanitize_component("../etc/passwd"), "___etc_passwd");
        assert_eq!(sanitize_component("agent-a"), "agent-a");
    }

    #[test]
    fn media_base64_redacted_url_kept() {
        let msgs = vec![Message {
            role: Role::User,
            content: MessageContent::Blocks(vec![carrier_types::message::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "aGk=".repeat(1000),
                url: Some("https://example.test/view.png".into()),
            }]),
        }];
        let events = message_events(&msgs);
        match &events[0] {
            SessionEventKind::UserMessage { content } => {
                let serialized = serde_json::to_string(content).unwrap();
                assert!(
                    !serialized.contains("aGk="),
                    "base64 payload must not enter the event log"
                );
                assert!(serialized.contains("redacted"));
                assert!(serialized.contains("view.png"), "url reference survives");
            }
            other => panic!("expected UserMessage, got {other:?}"),
        }
    }

    fn sequenced(kinds: Vec<SessionEventKind>) -> Vec<SessionEvent> {
        kinds
            .into_iter()
            .enumerate()
            .map(|(i, kind)| SessionEvent {
                seq: (i + 1) as u64,
                ts_ms: 0,
                kind,
            })
            .collect()
    }

    fn text_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            content: MessageContent::Text(text.to_string()),
        }
    }

    #[test]
    fn fold_surface_renders_tool_blocks_as_placeholders() {
        // Same rendering the sessions DB row always applied — now a
        // projection rule instead of a destructive persist-time strip.
        let msgs = vec![
            text_msg("查一下"),
            Message {
                role: Role::Assistant,
                content: MessageContent::Blocks(vec![carrier_types::message::ContentBlock::ToolUse {
                    id: "tu_1".into(),
                    name: "file_read".into(),
                    input: serde_json::json!({"path": "a.md"}),
                    provider_metadata: None,
                }]),
            },
        ];
        let events = sequenced(message_events(&msgs));
        let folded = fold_surface(&events);
        assert_eq!(folded.len(), 2);
        let serialized = serde_json::to_string(&folded[1].content).unwrap();
        assert!(
            serialized.contains("[Called file_read]"),
            "tool_use renders as placeholder: {serialized}"
        );
    }

    #[test]
    fn fold_surface_honors_compaction_summary() {
        // 6 surface messages, compaction shadows the first 4: fold =
        // [summary] + last 2. Envelope events interleave untouched.
        let mut kinds = vec![SessionEventKind::TurnStart];
        kinds.extend(message_events(&[
            text_msg("m1"),
            text_msg("m2"),
            text_msg("m3"),
            text_msg("m4"),
        ]));
        kinds.push(SessionEventKind::CompactionSummary {
            summary: "前情提要：m1 到 m4".into(),
            shadowed_msgs: 4,
            shadowed_tokens_est: 64,
        });
        kinds.extend(message_events(&[text_msg("m5"), text_msg("m6")]));
        kinds.push(SessionEventKind::TurnEnd {
            iterations: 1,
            tools_called: 0,
            tool_errors: 0,
            outcome: "complete".into(),
        });

        let folded = fold_surface(&sequenced(kinds));
        assert_eq!(folded.len(), 3);
        assert!(matches!(&folded[0].content, MessageContent::Text(t) if t.contains("前情提要")));
        assert!(matches!(&folded[1].content, MessageContent::Text(t) if t == "m5"));
        assert!(matches!(&folded[2].content, MessageContent::Text(t) if t == "m6"));
    }

    #[test]
    fn fold_surface_last_compaction_wins() {
        // Two stacked compactions: the later one's shadow prefix covers the
        // earlier summary's entire span.
        let mut kinds = message_events(&[text_msg("m1"), text_msg("m2"), text_msg("m3")]);
        kinds.push(SessionEventKind::CompactionSummary {
            summary: "旧摘要".into(),
            shadowed_msgs: 2,
            shadowed_tokens_est: 64,
        });
        kinds.push(SessionEventKind::CompactionSummary {
            summary: "新摘要（覆盖一切旧摘要）".into(),
            shadowed_msgs: 4, // 3 original messages + the old summary message
            shadowed_tokens_est: 64,
        });
        kinds.extend(message_events(&[text_msg("新消息")]));

        let folded = fold_surface(&sequenced(kinds));
        assert_eq!(folded.len(), 2);
        assert!(matches!(&folded[0].content, MessageContent::Text(t) if t.contains("新摘要")));
        assert!(matches!(&folded[1].content, MessageContent::Text(t) if t == "新消息"));
    }

    #[test]
    fn rebuild_turn_summaries_replays_and_honors_compaction() {
        let mut kinds = vec![SessionEventKind::TurnStart];
        kinds.extend(message_events(&[
            text_msg("m1"),
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("a1".into()),
            },
        ]));
        kinds.push(SessionEventKind::TurnSummaryGenerated {
            turn_number: 1,
            user_intent: "查月票".into(),
            assistant_outcome: "已出票".into(),
            key_facts: vec!["entity:月票M123".into()],
        });
        // Compaction shadows the 2 messages of turn 1.
        kinds.push(SessionEventKind::CompactionSummary {
            summary: "旧事".into(),
            shadowed_msgs: 2,
            shadowed_tokens_est: 64,
        });
        kinds.extend(message_events(&[
            text_msg("m2"),
            Message {
                role: Role::Assistant,
                content: MessageContent::Text("a2".into()),
            },
        ]));
        kinds.push(SessionEventKind::TurnSummaryGenerated {
            turn_number: 2,
            user_intent: "问时刻表".into(),
            assistant_outcome: "已给列表".into(),
            key_facts: vec![],
        });

        let events = sequenced(kinds);
        let rebuilt = rebuild_turn_summaries(&events);
        // The pre-compaction summary is dropped (stale by the live rule);
        // the post-compaction one replays with its key facts.
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].turn_number, 2);
        assert_eq!(rebuilt[0].user_intent, "问时刻表");
        assert!(
            rebuilt[0].tools_used.is_empty(),
            "replay never fabricates tools_used"
        );
    }
}
