//! Human-readable failure progress reports for recoverable flow errors.

use std::collections::HashMap;

use serde_json::Value;
use carrier_types::flow::{FlowDef, StepDef};

use super::template::value_to_string;
use crate::error::KernelError;

/// Build a human-readable progress report when a step fails at runtime (tool
/// error, LLM error, sub-flow error, ...). Lists completed steps (with a short
/// summary of their output), the failed step (with the error), and pending
/// steps, then prompts the user to retry or cancel. The report doubles as the
/// `Suspended { question }` payload so it flows through the same messaging path
/// as a `user_input` suspend.
pub(crate) fn build_failure_report(
    flow: &FlowDef,
    failed_step: &StepDef,
    err: &KernelError,
    outputs: &HashMap<String, Value>,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("流程「{}」执行中断：\n", flow.name));
    for s in &flow.steps {
        if s.id == failed_step.id {
            lines.push(format!("❌ {}  失败：{}", s.id, err));
        } else if let Some(v) = outputs.get(&s.id) {
            let summary = truncate_summary(&value_to_string(v), 50);
            lines.push(format!("✅ {}  {}", s.id, summary));
        } else {
            lines.push(format!("⏳ {}  （未执行）", s.id));
        }
    }
    lines.push(format!(
        "\n回复「重试」重新执行「{}」，或「取消」终止流程。",
        failed_step.id
    ));
    lines.join("\n")
}

/// Truncate a string to at most `max` characters (by Unicode scalar), appending
/// `…` when truncated. Keeps multi-byte (CJK) output readable.
fn truncate_summary(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}…")
    }
}
