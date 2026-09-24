//! Unit tests for flow pure helpers and types.

use std::collections::HashMap;

use serde_json::Value;
use carrier_types::error::CarrierError;
use carrier_types::flow::{StepDef, StepKind};

use crate::error::KernelError;
use crate::flow::dag::partition_flow_steps;
use crate::flow::report::*;
use crate::flow::template::*;
use crate::flow::types::*;
use crate::flow::FAILURE_CANCEL_KEYWORDS;

fn step(id: &str, deps: &[&str]) -> StepDef {
    StepDef {
        id: id.into(),
        kind: Some(StepKind::Chat),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

#[test]
fn partition_linear() {
    let steps = vec![step("a", &[]), step("b", &["a"]), step("c", &["b"])];
    let layers = partition_flow_steps(&steps).unwrap();
    assert_eq!(layers.len(), 3);
    assert_eq!(layers[0][0].id, "a");
    assert_eq!(layers[2][0].id, "c");
}

#[test]
fn partition_parallel_layer() {
    let steps = vec![step("a", &[]), step("b", &[]), step("c", &["a", "b"])];
    let layers = partition_flow_steps(&steps).unwrap();
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0].len(), 2);
    assert_eq!(layers[1].len(), 1);
}

#[test]
fn partition_detects_cycle() {
    let steps = vec![step("a", &["b"]), step("b", &["a"])];
    assert!(partition_flow_steps(&steps).is_err());
}

#[test]
fn partition_unknown_dep() {
    let steps = vec![step("a", &["missing"])];
    assert!(partition_flow_steps(&steps).is_err());
}

#[test]
fn render_outputs_and_input() {
    let mut outputs = HashMap::new();
    outputs.insert("draft".into(), Value::String("hello".into()));
    let input = serde_json::json!({"user_message": "hi"});
    let r = render_template(
        "{{ outputs.draft }} | {{ input.user_message }}",
        &outputs,
        &input,
    );
    assert_eq!(r, "hello | hi");
}

#[test]
fn render_bare_is_outputs() {
    let mut outputs = HashMap::new();
    outputs.insert("draft".into(), Value::String("hello".into()));
    let input = serde_json::json!({});
    assert_eq!(render_template("{{ draft }}", &outputs, &input), "hello");
}

#[test]
fn render_unresolved_kept() {
    let outputs = HashMap::new();
    let input = serde_json::json!({});
    assert_eq!(
        render_template("{{ outputs.missing }}", &outputs, &input),
        "{{ outputs.missing }}"
    );
}

#[test]
fn render_default_filter_unresolved() {
    // Unresolved path + default -> fallback (single/double/bare quotes).
    let outputs = HashMap::new();
    let input = serde_json::json!({});
    assert_eq!(
        render_template("{{ skipped | default('无') }}", &outputs, &input),
        "无"
    );
    assert_eq!(
        render_template("{{ skipped | default(\"none\") }}", &outputs, &input),
        "none"
    );
    assert_eq!(
        render_template("a-{{ x | default(0) }}-b", &outputs, &input),
        "a-0-b"
    );
}

#[test]
fn render_default_filter_resolved_wins() {
    // When the path resolves, default is ignored.
    let mut outputs = HashMap::new();
    outputs.insert("draft".into(), Value::String("hello".into()));
    let input = serde_json::json!({});
    assert_eq!(
        render_template("{{ draft | default('fallback') }}", &outputs, &input),
        "hello"
    );
}

#[test]
fn render_default_filter_preserves_case() {
    // The fallback value keeps its original case (not lowercased).
    let outputs = HashMap::new();
    let input = serde_json::json!({});
    assert_eq!(
        render_template("{{ x | default('DEFAULTED') }}", &outputs, &input),
        "DEFAULTED"
    );
    // Case-insensitive filter name: DEFAULT(...) works.
    assert_eq!(
        render_template("{{ x | DEFAULT('Up') }}", &outputs, &input),
        "Up"
    );
}

#[test]
fn render_default_filter_nested_path() {
    // Nested path + default (e.g. a skipped step's subfield).
    let mut outputs = HashMap::new();
    outputs.insert("review".into(), serde_json::json!({"decision": "proceed"}));
    let input = serde_json::json!({});
    // Present subfield resolves.
    assert_eq!(
        render_template(
            "{{ review.decision | default('cancel') }}",
            &outputs,
            &input
        ),
        "proceed"
    );
    // Missing subfield -> default.
    assert_eq!(
        render_template("{{ review.note | default('n/a') }}", &outputs, &input),
        "n/a"
    );
}

#[test]
fn when_eq_true() {
    let mut outputs = HashMap::new();
    outputs.insert("review".into(), serde_json::json!({"decision": "revise"}));
    let input = serde_json::json!({});
    assert!(eval_when("review.decision == 'revise'", &outputs, &input));
    assert!(!eval_when("review.decision == 'proceed'", &outputs, &input));
}

#[test]
fn when_missing_lhs_is_false() {
    let outputs = HashMap::new();
    let input = serde_json::json!({});
    // skipped step (no output) -> false (chain skip)
    assert!(!eval_when("review.decision == 'revise'", &outputs, &input));
    assert!(!eval_when("review.decision != 'cancel'", &outputs, &input));
}

#[test]
fn when_review_decision_not_cancel() {
    let mut outputs = HashMap::new();
    let input = serde_json::json!({});
    // proceed -> downstream `when: review.decision != 'cancel'` runs
    outputs.insert("review".into(), serde_json::json!({"decision": "proceed"}));
    assert!(eval_when("review.decision != 'cancel'", &outputs, &input));
    // cancel -> downstream gated step is skipped
    outputs.insert("review".into(), serde_json::json!({"decision": "cancel"}));
    assert!(!eval_when("review.decision != 'cancel'", &outputs, &input));
}

#[test]
fn decide_cancel_matches() {
    let kw = vec!["取消".to_string(), "cancel".to_string(), "算了".to_string()];
    assert!(decide_cancel("算了吧", &kw));
    assert!(decide_cancel("please cancel now", &kw));
    assert!(decide_cancel("取消", &kw));
    assert!(!decide_cancel("继续生成", &kw));
    assert!(!decide_cancel("ok", &kw));
    // empty keywords -> never cancel
    assert!(!decide_cancel("取消", &[]));
    // case-insensitive
    assert!(decide_cancel("CANCEL please", &kw));
}

#[test]
fn select_output_json() {
    let step = StepDef {
        id: "p".into(),
        output: Some("json".into()),
        ..Default::default()
    };
    let outputs = HashMap::new();
    let input = serde_json::json!({});
    let v = select_output(&step, r#"{"a":1}"#, &outputs, &input).unwrap();
    assert_eq!(v["a"], 1);
}

#[test]
fn select_output_json_parse_fail() {
    let step = StepDef {
        id: "p".into(),
        output: Some("json".into()),
        ..Default::default()
    };
    let outputs = HashMap::new();
    let input = serde_json::json!({});
    assert!(select_output(&step, "not json", &outputs, &input).is_err());
}

#[test]
fn render_as_binding_resolves_element_fields() {
    // map injects the current element under the `as` name into a cloned
    // outputs map; bare `{{ shot.prompt }}` then resolves via resolve_path.
    let mut outputs = HashMap::new();
    outputs.insert(
        "shot".into(),
        serde_json::json!({"prompt": "a sunset", "duration": 3}),
    );
    let input = serde_json::json!({});
    assert_eq!(
        render_template("{{ shot.prompt }}", &outputs, &input),
        "a sunset"
    );
    assert_eq!(
        render_template("{{ shot.duration }}", &outputs, &input),
        "3"
    );
}

#[test]
fn partition_handles_flow_exec_and_map_steps() {
    // flow_exec/map steps participate in topological layering like any step.
    let steps = vec![
        StepDef {
            id: "gen".into(),
            kind: Some(StepKind::Chat),
            output: Some("json".into()),
            ..Default::default()
        },
        StepDef {
            id: "batch".into(),
            kind: Some(StepKind::Map),
            over: Some("{{ gen }}".into()),
            as_name: Some("shot".into()),
            flow: Some("shot-image".into()),
            depends_on: vec!["gen".into()],
            ..Default::default()
        },
        StepDef {
            id: "merge".into(),
            kind: Some(StepKind::FlowExec),
            flow: Some("video-merger".into()),
            depends_on: vec!["batch".into()],
            ..Default::default()
        },
    ];
    let layers = partition_flow_steps(&steps).unwrap();
    assert_eq!(layers.len(), 3);
    assert_eq!(layers[0][0].id, "gen");
    assert_eq!(layers[1][0].id, "batch");
    assert_eq!(layers[2][0].id, "merge");
}

#[test]
fn flow_contains_user_input_detects_body() {
    use carrier_types::flow::FlowDef;
    // Batch map (no body) -> no user_input.
    let batch = FlowDef {
        steps: vec![StepDef {
            id: "batch".into(),
            kind: Some(StepKind::Map),
            over: Some("{{ x }}".into()),
            flow: Some("sub".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(!flow_contains_user_input(&batch));

    // Interactive map body with user_input -> detected (recursive).
    let interactive = FlowDef {
        steps: vec![StepDef {
            id: "per_ep".into(),
            kind: Some(StepKind::Map),
            over: Some("{{ eps }}".into()),
            body: Some(vec![
                StepDef {
                    id: "write".into(),
                    kind: Some(StepKind::Chat),
                    ..Default::default()
                },
                StepDef {
                    id: "review".into(),
                    kind: Some(StepKind::UserInput),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(flow_contains_user_input(&interactive));

    // Top-level user_input (no body) -> detected.
    let top = FlowDef {
        steps: vec![StepDef {
            id: "review".into(),
            kind: Some(StepKind::UserInput),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(flow_contains_user_input(&top));
}

#[test]
fn partition_handles_map_with_body() {
    // A map step carrying an inline body still participates in topological
    // layering; the body steps are NOT part of the top-level DAG.
    let steps = vec![
        StepDef {
            id: "eps".into(),
            kind: Some(StepKind::Chat),
            output: Some("json".into()),
            ..Default::default()
        },
        StepDef {
            id: "per_ep".into(),
            kind: Some(StepKind::Map),
            over: Some("{{ eps }}".into()),
            as_name: Some("ep".into()),
            depends_on: vec!["eps".into()],
            body: Some(vec![
                StepDef {
                    id: "write".into(),
                    kind: Some(StepKind::Chat),
                    ..Default::default()
                },
                StepDef {
                    id: "review".into(),
                    kind: Some(StepKind::UserInput),
                    depends_on: vec!["write".into()],
                    ..Default::default()
                },
            ]),
            ..Default::default()
        },
    ];
    let layers = partition_flow_steps(&steps).unwrap();
    assert_eq!(layers.len(), 2);
    assert_eq!(layers[0][0].id, "eps");
    assert_eq!(layers[1][0].id, "per_ep");
    // Body does not leak into the top-level partition.
    assert_eq!(layers[1].len(), 1);
}

#[test]
fn map_context_roundtrip() {
    let mc = MapContext {
        map_step_id: "per_ep".into(),
        over: vec![
            serde_json::json!({"index": 1}),
            serde_json::json!({"index": 2}),
        ],
        current_index: 1,
        collected: vec![serde_json::json!({"decision": "proceed"})],
        body_completed: {
            let mut m = HashMap::new();
            m.insert("write".into(), Value::String("ep1 text".into()));
            m
        },
        as_name: "ep".into(),
    };
    let json = serde_json::to_string(&mc).unwrap();
    // `as` is the serialized key for as_name.
    assert!(json.contains("\"as\":\"ep\""));
    assert!(json.contains("\"current_index\":1"));
    let back: MapContext = serde_json::from_str(&json).unwrap();
    assert_eq!(back.map_step_id, "per_ep");
    assert_eq!(back.current_index, 1);
    assert_eq!(back.over.len(), 2);
    assert_eq!(back.collected.len(), 1);
    assert_eq!(back.as_name, "ep");
    assert_eq!(
        back.body_completed.get("write").and_then(|v| v.as_str()),
        Some("ep1 text")
    );
}

#[test]
fn render_value_renders_string_leaves() {
    // String leaves are templates; object/array structure and non-string
    // leaves (numbers, bools) are preserved.
    let mut outputs = HashMap::new();
    outputs.insert("name".into(), Value::String("晨曦".into()));
    outputs.insert("count".into(), serde_json::json!(3));
    let input = serde_json::json!({"user_message": "hi"});
    let args = serde_json::json!({
        "title": "{{ name }}",
        "raw": "literal text",
        "n": 5,
        "flag": true,
        "nested": ["{{ name }}", 7, {"deep": "{{ name }}"}]
    });
    let rendered = render_value(&args, &outputs, &input);
    assert_eq!(rendered["title"].as_str(), Some("晨曦"));
    assert_eq!(rendered["raw"].as_str(), Some("literal text"));
    assert_eq!(rendered["n"].as_i64(), Some(5));
    assert_eq!(rendered["flag"].as_bool(), Some(true));
    assert_eq!(rendered["nested"][0].as_str(), Some("晨曦"));
    assert_eq!(rendered["nested"][1].as_i64(), Some(7));
    assert_eq!(rendered["nested"][2]["deep"].as_str(), Some("晨曦"));
}

#[test]
fn build_failure_report_lists_all_steps() {
    use carrier_types::flow::FlowDef;
    let flow = FlowDef {
        name: "draft-review".into(),
        steps: vec![
            StepDef {
                id: "draft".into(),
                kind: Some(StepKind::Chat),
                ..Default::default()
            },
            StepDef {
                id: "read".into(),
                kind: Some(StepKind::Tool),
                ..Default::default()
            },
            StepDef {
                id: "publish".into(),
                kind: Some(StepKind::Chat),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    // `draft` completed, `read` failed, `publish` pending.
    let mut outputs = HashMap::new();
    outputs.insert("draft".into(), Value::String("a draft about cats".into()));
    let failed = &flow.steps[1];
    let err = KernelError::Carrier(CarrierError::Internal("file '/tmp/x' not found".into()));
    let report = build_failure_report(&flow, failed, &err, &outputs);

    // Completed step shows id + truncated summary.
    assert!(report.contains("✅ draft"));
    assert!(report.contains("a draft about cats"));
    // Failed step shows id + error.
    assert!(report.contains("❌ read"));
    assert!(report.contains("file '/tmp/x' not found"));
    // Pending step shows id + (未执行).
    assert!(report.contains("⏳ publish"));
    assert!(report.contains("（未执行）"));
    // Retry/cancel hint mentions the failed step id.
    assert!(report.contains("重试"));
    assert!(report.contains("取消"));
    assert!(report.contains("「read」"));
    // Header names the flow.
    assert!(report.contains("draft-review"));
}

#[test]
fn build_failure_report_truncates_long_summary() {
    use carrier_types::flow::FlowDef;
    let flow = FlowDef {
        name: "f".into(),
        steps: vec![
            StepDef {
                id: "big".into(),
                kind: Some(StepKind::Chat),
                ..Default::default()
            },
            StepDef {
                id: "boom".into(),
                kind: Some(StepKind::Tool),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let long = "x".repeat(200);
    let mut outputs = HashMap::new();
    outputs.insert("big".into(), Value::String(long));
    let failed = &flow.steps[1];
    let err = KernelError::Carrier(CarrierError::Internal("boom".into()));
    let report = build_failure_report(&flow, failed, &err, &outputs);
    // Summary capped at 50 chars + ellipsis.
    let big_line = report.lines().find(|l| l.starts_with("✅ big")).unwrap();
    let summary = big_line.strip_prefix("✅ big  ").unwrap();
    assert_eq!(summary.chars().count(), 51); // 50 + …
    assert!(summary.ends_with('…'));
}

#[test]
fn failure_cancel_keywords_match() {
    // decide_cancel against FAILURE_CANCEL_KEYWORDS.
    let kw: Vec<String> = FAILURE_CANCEL_KEYWORDS
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(decide_cancel("取消", &kw));
    assert!(decide_cancel("please cancel now", &kw));
    assert!(decide_cancel("算了吧", &kw));
    assert!(decide_cancel("ABORT mission", &kw));
    // Non-cancel replies do not match (retry intent).
    assert!(!decide_cancel("重试", &kw));
    assert!(!decide_cancel("继续", &kw));
    assert!(!decide_cancel("再试一次", &kw));
    assert!(!decide_cancel("ok retry", &kw));
}
