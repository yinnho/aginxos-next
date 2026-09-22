//! Template rendering, `when` evaluation, and output selection for flow steps.

use std::collections::HashMap;

use serde_json::Value;
use carrier_types::error::CarrierError;
use carrier_types::flow::{FlowDef, StepDef, StepKind, StepOutputMode};

use crate::error::{KernelError, KernelResult};

/// Render a map step's `over` template and parse it as a JSON array. Errors
/// clearly if the template does not resolve to an array.
pub(crate) fn render_over_array(
    step: &StepDef,
    over_tpl: &str,
    outputs: &HashMap<String, Value>,
    input: &Value,
) -> KernelResult<Vec<Value>> {
    let over_str = render_template(over_tpl, outputs, input);
    serde_json::from_str::<Value>(&over_str)
        .map_err(|e| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "map step '{}' `over` did not resolve to a JSON array: {} (got: {})",
                step.id, e, over_str
            )))
        })?
        .as_array()
        .cloned()
        .ok_or_else(|| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "map step '{}' `over` resolved to a non-array",
                step.id
            )))
        })
}

/// True if a flow contains any `user_input` step, including inside interactive
/// map `body` blocks (recursive). Used to reject flow_exec sub-flows that would
/// suspend (no resume stack for sub-flows).
pub(crate) fn flow_contains_user_input(flow: &FlowDef) -> bool {
    flow.steps.iter().any(step_or_body_has_user_input)
}

pub(crate) fn step_or_body_has_user_input(step: &StepDef) -> bool {
    step.kind.as_ref() == Some(&StepKind::UserInput)
        || step
            .body
            .as_ref()
            .is_some_and(|body| body.iter().any(step_or_body_has_user_input))
}

/// Resolve a step's output to a JSON value based on its `output` mode.
pub(crate) fn select_output(
    step: &StepDef,
    final_msg: &str,
    outputs: &HashMap<String, Value>,
    input: &Value,
) -> KernelResult<Value> {
    match step.output_mode() {
        StepOutputMode::Llm => Ok(Value::String(final_msg.to_string())),
        StepOutputMode::Json => serde_json::from_str::<Value>(final_msg).map_err(|e| {
            KernelError::Carrier(CarrierError::Internal(format!(
                "step '{}' output:json parse failed: {}",
                step.id, e
            )))
        }),
        StepOutputMode::Report => {
            // Tolerate markdown code fences around the JSON (models love
            // wrapping structured output in ```json ... ```).
            let body = extract_json_span(final_msg).unwrap_or(final_msg.trim());
            let parsed: Value = serde_json::from_str(body).map_err(|e| {
                KernelError::Carrier(CarrierError::Internal(format!(
                    "step '{}' output:report parse failed (expected a JSON object): {}",
                    step.id, e
                )))
            })?;
            carrier_types::flow::validate_step_report(&parsed).map_err(|reason| {
                KernelError::Carrier(CarrierError::Internal(format!(
                    "step '{}' output:report invalid: {reason}",
                    step.id
                )))
            })?;
            Ok(parsed)
        }
        StepOutputMode::File(path) => {
            let rendered = render_template(&path, outputs, input);
            std::fs::read_to_string(&rendered)
                .map(Value::String)
                .map_err(|e| {
                    KernelError::Carrier(CarrierError::Internal(format!(
                        "step '{}' output:file '{}' missing: {}",
                        step.id, rendered, e
                    )))
                })
        }
    }
}

/// Extract the outermost `{ ... }` span from a fenced or prose-wrapped
/// message. Shared with the runtime's flow-level report gate — lives in
/// `carrier_types::flow` so both crates see one implementation.
fn extract_json_span(msg: &str) -> Option<&str> {
    carrier_types::flow::extract_json_span(msg)
}

/// Render `{{ ... }}` templates. Supports `{{ outputs.id }}`, `{{ outputs.id.field }}`,
/// `{{ input.key }}`, and bare `{{ id }}` (treated as `outputs.id`). Unresolved
/// expressions are left intact. Supports an optional `| default('fallback')`
/// filter: when the path does not resolve, the fallback is used instead of
/// leaving the expression literal (useful for `when:false`-skipped steps).
pub(crate) fn render_template(
    tpl: &str,
    outputs: &HashMap<String, Value>,
    input: &Value,
) -> String {
    let mut out = String::new();
    let mut rest = tpl;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let expr = after[..end].trim();
                let original = &rest[start..start + 2 + end + 2]; // full "{{ ... }}"
                let (path_expr, default_val) = parse_default_filter(expr);
                match resolve_path(path_expr, outputs, input) {
                    Some(v) => out.push_str(&value_to_string(&v)),
                    None => match default_val {
                        Some(d) => out.push_str(&d),
                        None => out.push_str(original),
                    },
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str("{{");
                out.push_str(after);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parse an optional `| default('x')` filter from a template expression.
/// Returns `(path_expr, Some(fallback))` when present, else `(expr, None)`.
/// Accepts single/double-quoted or bare fallback tokens:
/// `x | default('none')`, `x | default("none")`, `x | default(none)`.
pub(crate) fn parse_default_filter(expr: &str) -> (&str, Option<String>) {
    let Some(pos) = expr.find('|') else {
        return (expr, None);
    };
    let left = expr[..pos].trim();
    let right = expr[pos + 1..].trim();
    // Match `default(` case-insensitively, but preserve the fallback value's
    // original case (to_lowercase would corrupt e.g. 'DEFAULTED' -> 'defaulted').
    if !right.to_lowercase().starts_with("default(") {
        return (expr, None);
    }
    let rest = &right["default(".len()..];
    let inner = rest.strip_suffix(')').unwrap_or(rest).trim();
    let val = if inner.len() >= 2
        && ((inner.starts_with('\'') && inner.ends_with('\''))
            || (inner.starts_with('"') && inner.ends_with('"')))
    {
        inner[1..inner.len() - 1].to_string()
    } else {
        inner.to_string()
    };
    (left, Some(val))
}

/// Recursively render `{{ }}` templates inside a JSON value tree: each string
/// leaf is rendered via [`render_template`], object/array structure and
/// non-string leaves are preserved. Used to render a `tool` step's `tool_args`.
pub(crate) fn render_value(v: &Value, outputs: &HashMap<String, Value>, input: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(render_template(s, outputs, input)),
        Value::Object(m) => {
            let rendered: serde_json::Map<String, Value> = m
                .iter()
                .map(|(k, vv)| (k.clone(), render_value(vv, outputs, input)))
                .collect();
            Value::Object(rendered)
        }
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|vv| render_value(vv, outputs, input))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Resolve a dotted path (`outputs.id.field`, `input.key`, or bare `id`) to a
/// JSON value. Bare paths are treated as `outputs.<path>`.
pub(crate) fn resolve_path(
    path: &str,
    outputs: &HashMap<String, Value>,
    input: &Value,
) -> Option<Value> {
    let path = path.trim();
    let (root, rest): (&str, &str) = if let Some(p) = path.strip_prefix("outputs.") {
        ("outputs", p)
    } else if let Some(p) = path.strip_prefix("input.") {
        ("input", p)
    } else {
        ("outputs", path)
    };
    if rest.is_empty() {
        return None;
    }
    let parts: Vec<&str> = rest.split('.').collect();
    let mut cur: Value = if root == "outputs" {
        outputs.get(parts[0])?.clone()
    } else {
        input.get(parts[0])?.clone()
    };
    for f in &parts[1..] {
        cur = cur.get(f)?.clone();
    }
    Some(cur)
}

/// Evaluate a `when` expression: `LHS == 'rhs'` / `LHS != 'rhs'` (a missing LHS
/// -> false, so chains of skips propagate). A bare expression is truthy if it
/// resolves.
pub(crate) fn eval_when(expr: &str, outputs: &HashMap<String, Value>, input: &Value) -> bool {
    let expr = expr.trim();
    if let Some((lhs, rhs)) = expr.split_once("==") {
        let lhs_val = resolve_path(lhs, outputs, input);
        let rhs_str = rhs.trim().trim_matches('\'').trim_matches('"');
        lhs_val
            .map(|v| value_to_string(&v).trim() == rhs_str)
            .unwrap_or(false)
    } else if let Some((lhs, rhs)) = expr.split_once("!=") {
        let lhs_val = resolve_path(lhs, outputs, input);
        let rhs_str = rhs.trim().trim_matches('\'').trim_matches('"');
        lhs_val
            .map(|v| value_to_string(&v).trim() != rhs_str)
            .unwrap_or(false)
    } else {
        resolve_path(expr, outputs, input).is_some()
    }
}

pub(crate) fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        // Convention for tool/JSON step outputs: if the tool returns an object
        // with a standard user-facing string field, use that as channel text so
        // a flow can end on a tool step without a second LLM deliver step.
        // Domain wording belongs in the tool/flow (e.g. scripts), not here.
        Value::Object(map) => {
            for key in ["user_reply", "reply", "message", "text"] {
                if let Some(s) = map.get(key).and_then(|x| x.as_str()) {
                    if !s.trim().is_empty() {
                        return s.to_string();
                    }
                }
            }
            v.to_string()
        }
        other => other.to_string(),
    }
}

/// Decide whether a `user_input` reply cancels the flow: true if the reply
/// case-insensitively contains any of the `cancel_keywords`. Empty keywords
/// => never cancel.
pub(crate) fn decide_cancel(reply: &str, keywords: &[String]) -> bool {
    let reply_lower = reply.to_lowercase();
    keywords
        .iter()
        .any(|kw| !kw.is_empty() && reply_lower.contains(&kw.to_lowercase()))
}
