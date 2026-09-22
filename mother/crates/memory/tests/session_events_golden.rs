//! P1-B invariant tests: "model-visible ⟺ logged".
//!
//! The golden fixture is hand-written JSONL in the exact wire format the
//! event log persists — any format drift (enum rename, field change) breaks
//! this test before it breaks a fold in production. `derive_messages` must
//! reconstruct the message surface from the log alone.

use carrier_memory::{derive_messages, SessionEvent};
use carrier_types::message::{ContentBlock, Message, MessageContent, Role};

/// MessageContent/ContentBlock have no PartialEq — compare via the
/// serialized form instead.
fn same(a: &MessageContent, b: &MessageContent) -> bool {
    serde_json::to_value(a).unwrap() == serde_json::to_value(b).unwrap()
}

fn parse_golden() -> Vec<SessionEvent> {
    let raw = include_str!("fixtures/golden_session.jsonl");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("golden fixture line must parse"))
        .collect()
}

#[test]
fn golden_file_parses_and_sequences() {
    let events = parse_golden();
    assert_eq!(events.len(), 11);
    for (i, ev) in events.iter().enumerate() {
        assert_eq!(ev.seq, (i + 1) as u64, "seq must be dense and monotonic");
    }
}

#[test]
fn derive_reconstructs_surface_from_log_alone() {
    let events = parse_golden();
    let msgs = derive_messages(&events);

    // 11 events - 5 envelope events (2 TurnStart, 2 TurnEnd, 1 Silent)
    assert_eq!(msgs.len(), 6);

    // Turn 1: user asks, assistant calls a tool, tool result rides in a
    // user-role message, assistant answers.
    assert_eq!(msgs[0].role, Role::User);
    assert!(same(
        &msgs[0].content,
        &MessageContent::Text("帮我查下月票".to_string())
    ));

    assert_eq!(msgs[1].role, Role::Assistant);
    match &msgs[1].content {
        MessageContent::Blocks(blocks) => match &blocks[0] {
            ContentBlock::ToolUse {
                id, name, input, ..
            } => {
                assert_eq!(id, "tu_1");
                assert_eq!(name, "bus_query_order");
                assert_eq!(input["order_no"], "M123");
            }
            other => panic!("expected ToolUse block, got {other:?}"),
        },
        other => panic!("expected Blocks, got {other:?}"),
    }

    assert_eq!(msgs[2].role, Role::User);
    match &msgs[2].content {
        MessageContent::Blocks(blocks) => match &blocks[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                tool_name,
                content,
                is_error,
            } => {
                assert_eq!(tool_use_id, "tu_1");
                assert_eq!(tool_name, "bus_query_order");
                assert!(content.contains("M123"));
                assert!(!is_error);
            }
            other => panic!("expected ToolResult block, got {other:?}"),
        },
        other => panic!("expected Blocks, got {other:?}"),
    }

    assert_eq!(msgs[3].role, Role::Assistant);
    assert!(matches!(&msgs[3].content, MessageContent::Text(t) if t.contains("M123")));

    // Turn 2: user thanks, assistant goes silent — the [[silent]] assistant
    // message IS surface (it stays in history), the Silent envelope event is
    // not a message.
    assert_eq!(msgs[4].role, Role::User);
    assert_eq!(msgs[5].role, Role::Assistant);
    assert!(matches!(&msgs[5].content, MessageContent::Text(t) if t == "[[silent]]"));
}

/// The invariant, round-trip form: for any surface batch, logging then
/// deriving yields the batch back (System messages excluded by contract —
/// they are transient injections, media redacted by contract).
#[test]
fn message_events_derive_roundtrip() {
    let batch = vec![
        Message {
            role: Role::User,
            content: MessageContent::Text("查时刻表".to_string()),
        },
        Message::system("transient status injection — must not survive"),
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("8:30 / 9:15 / 10:00".to_string()),
        },
    ];
    let kinds = carrier_memory::message_events(&batch);
    let events: Vec<SessionEvent> = kinds
        .into_iter()
        .enumerate()
        .map(|(i, kind)| SessionEvent {
            seq: (i + 1) as u64,
            ts_ms: 0,
            kind,
        })
        .collect();
    let derived = derive_messages(&events);
    assert_eq!(derived.len(), 2);
    assert!(same(&derived[0].content, &batch[0].content));
    assert!(same(&derived[1].content, &batch[2].content));
}

/// Integration form: what save_session_append_async logs is exactly what the
/// turn produced (substrate-level, the path production takes).
#[tokio::test]
async fn substrate_save_appends_events_matching_batches() {
    let sub = carrier_memory::MemorySubstrate::open_in_memory().unwrap();
    let session = sub
        .create_session_with_label("gold-agent".to_string(), Some("user:test"))
        .unwrap();
    let sid = session.id;

    let batch1 = vec![Message {
        role: Role::User,
        content: MessageContent::Text("第一轮".to_string()),
    }];
    let batch2 = vec![
        Message::system("status — not surface"),
        Message {
            role: Role::Assistant,
            content: MessageContent::Text("第一轮回复".to_string()),
        },
        Message {
            role: Role::User,
            content: MessageContent::Text("第二轮".to_string()),
        },
    ];

    sub.save_session_append_async(sid, "gold-agent", &batch1, 128_000, Some("user:test"), None)
        .await
        .unwrap();
    sub.save_session_append_async(sid, "gold-agent", &batch2, 128_000, Some("user:test"), None)
        .await
        .unwrap();

    let events = sub
        .session_events_read("gold-agent", &sid.0.to_string())
        .unwrap();
    let derived = derive_messages(&events);
    // Batches concatenated, System messages dropped: 3 surface messages.
    assert_eq!(derived.len(), 3);
    assert!(same(&derived[0].content, &batch1[0].content));
    assert!(same(&derived[1].content, &batch2[1].content));
    assert!(same(&derived[2].content, &batch2[2].content));
}
