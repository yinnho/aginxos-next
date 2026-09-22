//! Flow definition types and frontmatter parsing.
//!
//! A flow is the capability unit (replaces the legacy "skill"). It has two forms:
//! - **single-step** (`steps` empty/absent): body injected into the system prompt,
//!   the LLM runs freely in `run_agent_loop` (equivalent to the legacy skill).
//! - **multi-step** (`steps` non-empty): a DAG executed by `run_flow`.
//!
//! This module is the single authoritative parser for flow frontmatter. It lives
//! in `types` (not `kernel`) so both `kernel` (classification) and `runtime`
//! (`run_flow` execution) can share it without violating the `kernel -> runtime`
//! dependency direction.

use serde_json::{Map, Value};

/// How a single step is executed. `AgentLoop`, `Chat`, `UserInput`,
/// `FlowExec`, and `Map` are executed by `run_flow`; `Tool` is parsed but not
/// yet executed (rejected as unsupported); other kinds are preserved as
/// `Unknown` so later stages can add execution without touching the parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    AgentLoop,
    Chat,
    Tool,
    /// Suspend the flow and ask the human a question; resume on their next
    /// message (stage D). The user's reply becomes this step's output
    /// `{ decision, text }`.
    UserInput,
    /// Invoke another flow by name (stage E.1). `with` becomes the sub-flow's
    /// `input`; output is the sub-flow's final value.
    FlowExec,
    /// Iterate a dynamic array (`over`), running a sub-flow per element (stage
    /// E.1, serial batch). Output is the collected results array.
    Map,
    /// A recognized-but-not-yet-executed kind (e.g. `delegate`). `run_flow`
    /// rejects these until later stages.
    Unknown(String),
}

impl StepKind {
    pub fn parse(s: &str) -> Self {
        match s.trim() {
            "agent_loop" => Self::AgentLoop,
            "chat" => Self::Chat,
            "tool" => Self::Tool,
            "user_input" => Self::UserInput,
            "flow_exec" => Self::FlowExec,
            "map" => Self::Map,
            other => Self::Unknown(other.to_string()),
        }
    }

    /// True if `run_flow` can currently execute this kind. All of
    /// `AgentLoop`/`Chat`/`UserInput`/`FlowExec`/`Map`/`Tool` are executed;
    /// other kinds are preserved as `Unknown` so later stages can add
    /// execution without touching the parser.
    pub fn is_executable(&self) -> bool {
        matches!(
            self,
            Self::AgentLoop
                | Self::Chat
                | Self::UserInput
                | Self::FlowExec
                | Self::Map
                | Self::Tool
        )
    }
}

/// How a step's output is captured (see refactor doc §4.5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StepOutputMode {
    /// LLM final message text (default).
    #[default]
    Llm,
    /// Read a file's content at step completion. Path may be templated.
    File(String),
    /// Parse the LLM final message as JSON.
    Json,
    /// Parse the LLM final message as a JSON structured report and enforce
    /// the Ralph constraint matrix (see [`validate_step_report`]). The step
    /// FAILS with a precise field-level error when the report is malformed —
    /// `on_failure` can route the repair.
    Report,
}

impl StepOutputMode {
    /// Parse a raw `output:` frontmatter value.
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if s == "json" {
            Self::Json
        } else if s == "report" {
            Self::Report
        } else if let Some(p) = s.strip_prefix("file:") {
            Self::File(p.trim().to_string())
        } else {
            Self::Llm // "llm" or anything unrecognized -> default
        }
    }
}

/// Hard cap on a step report's serialized size — the handoff between
/// orchestration rounds must stay bounded (dsh tool-ralph `maxHandoffChars`).
pub const REPORT_HANDOFF_MAX_CHARS: usize = 16384;

/// Extract the outermost `{ ... }` span from a fenced or prose-wrapped
/// message (models love wrapping structured output in ```json ... ```).
/// `None` when the text has no `{`/`}` span at all.
pub fn extract_json_span(msg: &str) -> Option<&str> {
    let start = msg.find('{')?;
    let end = msg.rfind('}')?;
    if end > start {
        Some(&msg[start..=end])
    } else {
        None
    }
}

/// Validate a structured step report against the Ralph per-status constraint
/// matrix (dsh tool-ralph `validateReport`). Constraints live on FIELDS, not
/// free text — an invalid combination is rejected with a precise, field-level
/// reason the caller can repair on:
///
/// - `status: continue` → `next_steps` non-empty (every entry non-blank),
///   `blocker` absent/blank;
/// - `status: complete` → `evidence` non-blank (string or array of non-blank
///   entries), `next_steps` AND `blocker` absent/blank;
/// - `status: blocked`  → `blocker` non-blank, `next_steps` absent/blank.
///
/// Every present string field must be "normalized" (non-blank after trim).
/// The whole report must serialize under [`REPORT_HANDOFF_MAX_CHARS`].
pub fn validate_step_report(value: &Value) -> Result<(), String> {
    let obj = value.as_object().ok_or("report must be a JSON object")?;
    let serialized_len = serde_json::to_string(value)
        .map_err(|e| e.to_string())?
        .len();
    if serialized_len > REPORT_HANDOFF_MAX_CHARS {
        return Err(format!(
            "report is {serialized_len} chars, over the {REPORT_HANDOFF_MAX_CHARS} handoff cap"
        ));
    }

    fn blank(v: &Value) -> bool {
        match v {
            Value::String(s) => s.trim().is_empty(),
            Value::Null => true,
            Value::Array(a) => a.is_empty() || a.iter().all(blank),
            _ => false,
        }
    }
    fn nonblank_strings<'a>(v: &'a Value, field: &str) -> Result<Vec<&'a str>, String> {
        match v {
            Value::Array(items) => {
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let s = item
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .ok_or_else(|| format_err(field))?;
                    out.push(s);
                }
                if out.is_empty() {
                    return Err(format_err(field));
                }
                Ok(out)
            }
            Value::String(s) if !s.trim().is_empty() => Ok(vec![s.as_str()]),
            _ => Err(format_err(field)),
        }
    }
    fn format_err(field: &str) -> String {
        format!("field '{field}' must be a non-empty string or array of non-empty strings")
    }

    let status = obj
        .get("status")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| matches!(*s, "continue" | "complete" | "blocked"))
        .ok_or_else(|| {
            "field 'status' must be exactly one of \"continue\" | \"complete\" | \"blocked\""
                .to_string()
        })?
        .to_string();

    let has_blocker = obj.get("blocker").is_some_and(|v| !blank(v));
    let has_next = obj.get("next_steps").is_some_and(|v| !blank(v));

    match status.as_str() {
        "continue" => {
            let next = obj
                .get("next_steps")
                .ok_or("status 'continue' requires a non-empty 'next_steps' array")?;
            nonblank_strings(next, "next_steps")?;
            if has_blocker {
                return Err("status 'continue' must NOT carry a 'blocker'".to_string());
            }
        }
        "complete" => {
            let evidence = obj
                .get("evidence")
                .ok_or("status 'complete' requires non-empty 'evidence'")?;
            nonblank_strings(evidence, "evidence")?;
            if has_next {
                return Err("status 'complete' must NOT carry 'next_steps'".to_string());
            }
            if has_blocker {
                return Err("status 'complete' must NOT carry a 'blocker'".to_string());
            }
        }
        "blocked" => {
            let blocker = obj
                .get("blocker")
                .ok_or("status 'blocked' requires a non-empty 'blocker' string")?;
            nonblank_strings(blocker, "blocker")?;
            if has_next {
                return Err("status 'blocked' must NOT carry 'next_steps'".to_string());
            }
        }
        _ => unreachable!("status filtered above"),
    }
    Ok(())
}

/// A single step in a multi-step flow DAG.
#[derive(Debug, Clone, Default)]
pub struct StepDef {
    /// Step identifier (required, unique within a flow).
    pub id: String,
    /// Execution kind. `None` means `kind:` was absent (invalid).
    pub kind: Option<StepKind>,
    /// IDs of steps that must complete before this one (DAG edges).
    pub depends_on: Vec<String>,
    /// Condition expression evaluated before execution, e.g.
    /// `"review.decision == 'revise'"`. `false` -> step is skipped.
    pub when: Option<String>,
    /// Step to jump to on failure (graceful degradation).
    pub on_failure: Option<String>,
    /// Raw `output:` value; resolved to [`StepOutputMode`] at execution time.
    pub output: Option<String>,
    /// Step-specific instruction prompt (may contain templates).
    pub prompt: Option<String>,
    /// Task text for `chat` steps.
    pub task: Option<String>,
    /// Tool name for `tool` steps.
    pub tool_name: Option<String>,
    /// Tool arguments for `tool` steps (template strings allowed in values).
    pub tool_args: Value,
    /// Parameters passed to the step (template strings as values).
    pub with: Map<String, Value>,
    /// Cancel keywords for `user_input` steps (case-insensitive substring
    /// match against the user's reply -> `decision = "cancel"`).
    pub cancel_keywords: Vec<String>,
    /// Per-step timeout for `user_input` steps, in hours. `None` => the
    /// kernel config default (`user_input_timeout_secs`).
    pub timeout_hours: Option<f64>,
    /// Sub-flow name for `flow_exec` steps and `map` step bodies (stage E.1).
    pub flow: Option<String>,
    /// Template resolving to a JSON array, iterated by `map` steps.
    pub over: Option<String>,
    /// Element binding name in `map` step templates (defaults to `"item"`).
    pub as_name: Option<String>,
    /// Inline steps body for interactive `map` steps (stage E.2). When set, the
    /// map iterates `over` running this step list per element (may contain
    /// `user_input`, suspending via `map_context`); when `None`, the map uses
    /// `flow`/`with` (batch form, stage E.1).
    pub body: Option<Vec<StepDef>>,
    /// Concurrency for batch `map` steps (stage E.1): up to this many sub-flows
    /// run at once. `None`/`1` => serial (default). Ignored (must be 1) for
    /// interactive maps (`body` set), which can suspend per element.
    pub parallel: Option<u32>,
}

impl StepDef {
    /// Resolved output mode (defaults to [`StepOutputMode::Llm`]).
    pub fn output_mode(&self) -> StepOutputMode {
        self.output
            .as_deref()
            .map(StepOutputMode::parse)
            .unwrap_or_default()
    }
}

/// Privilege tier for a flow. `System` grants turn-scoped elevation only when
/// the flow is loaded from the shared `~/.aginx/carrier/flows` directory (not a
/// private workspace overlay). See `docs/OFFICE-SYSTEM-FLOWS.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlowPrivilege {
    /// Default: no elevation; tools still filtered by agent `max_tool_level`.
    #[default]
    Agent,
    /// Platform system capability: when loaded from shared flows dir, inject
    /// declared tools and raise `max_tool_level` for this turn only.
    System,
}

impl FlowPrivilege {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" => Self::System,
            _ => Self::Agent,
        }
    }
}

/// Metadata keys written onto `AgentManifest.metadata` for turn-scoped system
/// flow elevation (not persisted to agent.toml).
pub const META_FLOW_ELEVATED_TOOLS: &str = "flow_elevated_tools";
pub const META_FLOW_SHELL_ALLOW: &str = "flow_shell_allow";
/// Tools stripped for this turn (and blocked at execute / text-recovery).
pub const META_FLOW_DENY_TOOLS: &str = "flow_deny_tools";
/// Hard tool allow-list for this turn (flow `tools:` sandbox): when a matched
/// flow declares a non-empty `tools:` set, the agent may only call tools in
/// that set ∪ its legit base toolset (core + api + subagent).
/// `tool_runner` denies calls outside it. Stamped as
/// the assembled tool names at flow-load time (frozen — discovery cannot
/// widen it by surfacing out-of-set tools).
pub const META_FLOW_ALLOWED_TOOLS: &str = "flow_allowed_tools";
/// Set when a flow or subagent declared `max_iterations` (not the AutonomousConfig default).
pub const META_MAX_ITERATIONS_DECLARED: &str = "max_iterations_declared";
/// Set when the matched flow's top-level `output:` is `report`: the agent
/// turn's FINAL message must carry a valid Ralph report (validated by
/// `validate_step_report` in end_turn) — a hard gate for chained pipeline
/// steps whose quality otherwise rides on agent goodwill.
pub const META_OUTPUT_REPORT: &str = "flow_output_report";

/// Author-attached golden examples for a single `shell_allow` pattern — used
/// only by [`validate_shell_allow`] at install/load time to catch patterns that
/// are too narrow (typo'd script path → the command can never match) or too
/// broad (would authorize a dangerous command). Never consulted at runtime.
///
/// In the flow frontmatter a check is written as a `shell_allow` map entry:
/// ```yaml
/// shell_allow:
///   - pattern: python3 output/scripts/*
///     match: [python3 output/scripts/run.py]
///     not_match: [rm -rf /]
/// ```
/// (`match` maps to [`Self::matches`]; `not_match` maps to [`Self::not_matches`].)
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShellAllowCheck {
    pub pattern: String,
    pub matches: Vec<String>,
    pub not_matches: Vec<String>,
}

/// A parsed flow definition.
#[derive(Debug, Clone, Default)]
pub struct FlowDef {
    pub name: String,
    pub description: String,
    pub max_iterations: Option<u32>,
    pub tools: Vec<String>,
    /// Instruction body (markdown after frontmatter).
    pub body: String,
    /// Steps. Empty => single-step flow.
    pub steps: Vec<StepDef>,
    /// Which step's output is the flow's final result. Defaults to the last
    /// executed (non-skipped) step.
    pub final_step: Option<String>,
    /// `false` => not selectable by `classify_flow` (pure atomic). Defaults to true.
    pub entry: Option<bool>,
    /// Top-level `output` for single-step flows.
    pub output: Option<String>,
    /// Privilege tier (`agent` default, or `system` for shared platform flows).
    pub privilege: FlowPrivilege,
    /// Shell command allow-patterns for elevated `shell_exec` (glob `*`).
    /// Example: `python3 output/scripts/*`
    pub shell_allow: Vec<String>,
    /// Author-attached golden examples for `shell_allow` patterns (declaration-
    /// time verification only — never used at runtime). See [`validate_shell_allow`].
    pub shell_allow_checks: Vec<ShellAllowCheck>,
    /// Tools forbidden for this turn even if they are core/always-on
    /// (e.g. `image_generate` on a template-based poster flow).
    pub deny_tools: Vec<String>,
}

impl FlowDef {
    /// True if this is a multi-step (DAG) flow.
    pub fn is_multi_step(&self) -> bool {
        !self.steps.is_empty()
    }

    /// Turn-scoped tool elevation for this flow.
    ///
    /// A flow that declares both `shell_exec` (or `process_start`) and a
    /// non-empty `shell_allow` elevates the turn — clone-local skills can run
    /// allowlisted commands without permanent agent shell access. Used both at
    /// flow match time (kernel `apply_flow_elevation`) and when the agent
    /// explicitly `flow_load`s a flow mid-turn (runtime grants the same
    /// turn-scoped authority for the loaded flow's declared tools).
    pub fn elevates(&self) -> bool {
        !self.shell_allow.is_empty()
            && self
                .tools
                .iter()
                .any(|t| t == "shell_exec" || t == "process_start")
    }

    /// Highest permission level among declared tools (for turn elevation).
    pub fn required_max_tool_level(&self) -> crate::tool::PermissionLevel {
        self.tools
            .iter()
            .map(|t| crate::tool::PermissionLevel::for_tool(t))
            .max()
            .unwrap_or(crate::tool::PermissionLevel::Write)
    }
}

/// Parse a flow `.md` file's content into a [`FlowDef`].
///
/// Frontmatter is YAML-like (delimited by `---`); the body is everything after
/// the closing `---`. Unknown frontmatter keys are ignored. The `steps:` block
/// is parsed as a nested list-of-maps (a constrained YAML subset).
pub fn parse_flow_def(content: &str) -> FlowDef {
    let content = content.trim();
    let (frontmatter, body) = split_frontmatter(content);

    let mut def = FlowDef {
        body: body.to_string(),
        ..Default::default()
    };
    if frontmatter.is_empty() {
        return def;
    }

    let lines: Vec<&str> = frontmatter.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        // Only top-level (indent 0) keys are handled here; nested content under
        // `steps:` is consumed wholesale by `parse_steps_block`.
        let indent = line.len() - line.trim_start().len();
        if indent != 0 {
            i += 1;
            continue;
        }
        let trimmed = line.trim();

        if let Some(val) = trimmed.strip_prefix("name:") {
            def.name = unquote(val);
            i += 1;
        } else if let Some(val) = trimmed.strip_prefix("description:") {
            def.description = unquote(val);
            i += 1;
        } else if let Some(val) = trimmed.strip_prefix("max_iterations:") {
            def.max_iterations = unquote(val).parse().ok();
            i += 1;
        } else if let Some(val) = trimmed.strip_prefix("final:") {
            let s = unquote(val);
            def.final_step = (!s.is_empty()).then_some(s);
            i += 1;
        } else if let Some(val) = trimmed.strip_prefix("entry:") {
            def.entry = match unquote(val).as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            };
            i += 1;
        } else if let Some(val) = trimmed.strip_prefix("output:") {
            let s = unquote(val);
            def.output = (!s.is_empty()).then_some(s);
            i += 1;
        } else if let Some(val) = trimmed.strip_prefix("privilege:") {
            def.privilege = FlowPrivilege::parse(&unquote(val));
            i += 1;
        } else if trimmed == "tools:" || trimmed.starts_with("tools:") {
            let (list, consumed) = parse_top_array(&lines, i, "tools");
            def.tools = list;
            i += consumed.max(1);
        } else if trimmed == "shell_allow:" || trimmed.starts_with("shell_allow:") {
            let (list, checks, consumed) = parse_shell_allow(&lines, i);
            def.shell_allow = list;
            def.shell_allow_checks = checks;
            i += consumed.max(1);
        } else if trimmed == "deny_tools:" || trimmed.starts_with("deny_tools:") {
            let (list, consumed) = parse_top_array(&lines, i, "deny_tools");
            def.deny_tools = list;
            i += consumed.max(1);
        } else if trimmed == "steps:" || trimmed.starts_with("steps:") {
            let inline = trimmed.strip_prefix("steps:").unwrap_or("").trim();
            if inline == "[]" {
                def.steps = Vec::new();
                i += 1;
            } else if inline.is_empty() {
                let (steps, consumed) = parse_steps_block(&lines, i);
                def.steps = steps;
                i += consumed.max(1);
            } else {
                // Inline steps (e.g. `steps: [...]`) are not supported; ignore.
                i += 1;
            }
        } else {
            // Unknown top-level key (e.g. `version:`) - skip.
            i += 1;
        }
    }

    def
}

/// Split `---\n<fm>\n---\n<body>` into (frontmatter, body). If there is no
/// frontmatter, returns ("", content).
fn split_frontmatter(content: &str) -> (&str, &str) {
    let rest = match content.strip_prefix("---") {
        Some(r) => r,
        None => return ("", content),
    };
    match rest.find("---") {
        Some(end) => (&rest[..end], rest[end + 3..].trim()),
        None => ("", content),
    }
}

/// Parse a top-level array field (inline `[a, b]` or block `  - a` form).
/// `key` is the field name without colon (e.g. `"tools"`, `"shell_allow"`).
/// Returns (values, lines_consumed_including_the_key_line).
fn parse_top_array(lines: &[&str], key_idx: usize, key: &str) -> (Vec<String>, usize) {
    let prefix = format!("{key}:");
    let inline = lines[key_idx]
        .trim()
        .strip_prefix(&prefix)
        .unwrap_or("")
        .trim();
    if !inline.is_empty() {
        // inline form (also handles `[]`)
        return (parse_inline_list(inline), 1);
    }
    // block form: collect subsequent `  - x` lines at indent > 0
    let mut out = Vec::new();
    let mut j = key_idx + 1;
    while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() {
            j += 1;
            continue;
        }
        let indent = l.len() - l.trim_start().len();
        if indent == 0 {
            break;
        }
        let t = l.trim_start();
        if let Some(item) = t.strip_prefix('-') {
            let v = unquote(item.trim());
            if !v.is_empty() {
                out.push(v);
            }
            j += 1;
        } else {
            // a non-list indented line ends the block
            break;
        }
    }
    (out, j - key_idx)
}

/// Parse the `shell_allow:` field, which accepts two entry forms:
///   - a plain string: `- python3 output/scripts/*` (backward-compatible)
///   - a map with golden examples:
///       - pattern: python3 output/scripts/*
///         match: [python3 output/scripts/run.py]
///         not_match: [rm -rf /]
///
/// Inline `[...]` form supports plain strings only.
///
/// Returns (patterns, checks, lines_consumed_including_the_key_line).
fn parse_shell_allow(lines: &[&str], key_idx: usize) -> (Vec<String>, Vec<ShellAllowCheck>, usize) {
    let mut patterns = Vec::new();
    let mut checks = Vec::new();

    // Inline form (`shell_allow: [a, b]` or `shell_allow: a`) — plain strings only.
    let inline = lines[key_idx]
        .trim()
        .strip_prefix("shell_allow:")
        .unwrap_or("")
        .trim();
    if !inline.is_empty() {
        for p in parse_inline_list(inline) {
            if !p.is_empty() {
                patterns.push(p);
            }
        }
        return (patterns, checks, 1);
    }

    // Block form: subsequent `  - x` lines at indent > 0.
    let mut j = key_idx + 1;
    while j < lines.len() {
        let l = lines[j];
        if l.trim().is_empty() {
            j += 1;
            continue;
        }
        let indent = l.len() - l.trim_start().len();
        if indent == 0 {
            break;
        }
        let t = l.trim_start();
        let Some(item) = t.strip_prefix('-') else {
            break;
        };
        let item = item.trim();
        if item.starts_with("pattern:") {
            let pattern = unquote(item.strip_prefix("pattern:").unwrap_or("").trim());
            let mut check = ShellAllowCheck {
                pattern: pattern.clone(),
                ..Default::default()
            };
            j += 1;
            // Consume nested `match:` / `not_match:` keys at deeper indent.
            while j < lines.len() {
                let l2 = lines[j];
                if l2.trim().is_empty() {
                    j += 1;
                    continue;
                }
                let indent2 = l2.len() - l2.trim_start().len();
                if indent2 <= indent {
                    break;
                }
                let t2 = l2.trim_start();
                if let Some(v) = t2.strip_prefix("match:") {
                    check.matches.extend(parse_inline_list(v));
                } else if let Some(v) = t2.strip_prefix("not_match:") {
                    check.not_matches.extend(parse_inline_list(v));
                }
                j += 1;
            }
            if !pattern.is_empty() {
                patterns.push(pattern);
                checks.push(check);
            }
        } else {
            let v = unquote(item);
            if !v.is_empty() {
                patterns.push(v);
            }
            j += 1;
        }
    }

    (patterns, checks, j - key_idx)
}

/// Match a shell command against flow `shell_allow` glob patterns.
///
/// Patterns support a single `*` wildcard (e.g. `python3 output/scripts/*`).
/// Matching is against the full command string (trimmed). Empty patterns deny all.
pub fn command_matches_shell_allow(command: &str, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let cmd = command.trim();
    if cmd.is_empty() {
        return false;
    }
    patterns
        .iter()
        .any(|p| shell_allow_glob_match(p.trim(), cmd))
}

/// Base command names that must never appear as a `shell_allow` pattern's base
/// command — there is no legitimate vetted-script use for these (unlike
/// `python3`/`ffmpeg`, which legitimately run flow scripts). A trailing `*`
/// means prefix-match (e.g. `mkfs*` catches `mkfs.ext4`). Checked at install
/// time by [`validate_shell_allow`].
pub const FORBIDDEN_SHELL_ALLOW_BASES: &[&str] = &[
    "sudo",
    "rm",
    "mkfs*",
    "dd",
    "osascript",
    "powershell",
    "pwsh",
];

/// Validate a flow's `shell_allow` declarations at install/load time. Returns
/// structured Chinese errors (what's wrong -> fix hint) so an agent can
/// self-repair in one round.
///
/// Two families:
///   1. Structural lint (unconditional): a pattern that is `*` (matches
///      everything = total bypass) or whose base command is a forbidden binary.
///   2. Golden-sample checks (only when the author attached `match`/`not_match`
///      examples): the referenced pattern must exist; every `match` example must
///      match (else the pattern is too narrow — a typo'd script path that can
///      never authorize the intended command); every `not_match` example must
///      NOT match (else the pattern is too broad and would authorize a
///      dangerous command).
///
/// Uses [`command_matches_shell_allow`] (the raw glob, no workspace
/// normalization) because a golden sample asserts the pattern's *intrinsic*
/// boundary, independent of any runtime path resolution.
pub fn validate_shell_allow(def: &FlowDef) -> Vec<String> {
    let mut errors = Vec::new();

    for pattern in &def.shell_allow {
        let p = pattern.trim();
        if p == "*" {
            errors.push(format!(
                "shell_allow 条目 '{pattern}' 是 '*' —— 匹配一切命令，等于完全绕过 shell 安全边界。\
                 修复：改成具体脚本路径 glob，例如 'python3 flows/<name>/scripts/*'"
            ));
            continue;
        }
        let base = p.split_whitespace().next().unwrap_or("");
        let base = base.rsplit('/').next().unwrap_or(base);
        let forbidden = FORBIDDEN_SHELL_ALLOW_BASES.iter().any(|b| {
            if let Some(prefix) = b.strip_suffix('*') {
                base.starts_with(prefix)
            } else {
                *b == base
            }
        });
        if forbidden {
            errors.push(format!(
                "shell_allow 条目 '{pattern}' 的基准命令 '{base}' 是危险命令，不允许声明在 shell_allow 里。\
                 修复：改用安全的脚本/工具路径"
            ));
        }
    }

    for check in &def.shell_allow_checks {
        let pattern = check.pattern.trim().to_string();
        if !def.shell_allow.iter().any(|p| p.trim() == pattern) {
            errors.push(format!(
                "shell_allow 校验引用了不存在的 pattern '{pattern}' —— 它必须同时出现在 shell_allow 列表里。\
                 修复：要么把该 pattern 加进 shell_allow，要么删除这条校验"
            ));
            continue;
        }
        let single = [pattern.clone()];
        for ex in &check.matches {
            if !command_matches_shell_allow(ex, &single) {
                errors.push(format!(
                    "shell_allow pattern '{pattern}' 的 match 示例 '{ex}' 竟然不匹配 —— pattern 过窄或脚本路径 typo，\
                     该命令将永远匹配不上。修复：修正 pattern 或示例"
                ));
            }
        }
        for ex in &check.not_matches {
            if command_matches_shell_allow(ex, &single) {
                errors.push(format!(
                    "shell_allow pattern '{pattern}' 的 not_match 示例 '{ex}' 竟然匹配 —— pattern 过宽，\
                     会放行危险命令。修复：收紧 pattern（更具体的目录/脚本名）"
                ));
            }
        }
    }

    errors
}

/// True if `command` contains a `..` path segment (`../`, `/../`, `/..`, or a
/// lone `..` token) - a traversal component that could escape a pattern's
/// directory prefix. Does NOT match `..` inside a filename like `foo..bar`.
/// Strips leading `VAR=value` env assignments first (same rule as
/// `normalize_command_for_match`) so `PYTHONPATH=../etc python3 ...` is caught.
fn command_has_dotdot_segment(command: &str) -> bool {
    let mut tokens: Vec<&str> = command.split_whitespace().collect();
    while let Some(first) = tokens.first() {
        if let Some(eq) = first.find('=') {
            let name = &first[..eq];
            let valid_name = !name.is_empty()
                && name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if valid_name {
                tokens.remove(0);
                continue;
            }
        }
        break;
    }
    tokens.iter().any(|tok| {
        *tok == ".." || tok.starts_with("../") || tok.contains("/../") || tok.ends_with("/..")
    })
}

/// Match a shell command against flow `shell_allow` patterns, also trying a
/// workspace-relative-ized form of the command.
///
/// Flow authors write workspace-relative patterns (e.g. `python3 flows/foo/*`),
/// but agents often discover and use absolute workspace paths
/// (`python3 /home/.../ws/flows/foo/scripts/x.py`). Stripping the
/// `workspace_root` prefix lets the same relative pattern match the absolute
/// command pointing at the identical script. Security-neutral: the same glob
/// match runs — the actual executed command is unchanged, and relative-pattern
/// traversal risk (already present) is not widened.
/// Strip a leading `cd <DIR> && <REST>` prefix, returning `(REST, cd_dir)`.
///
/// Agents often run flow scripts via `cd <workspace_root> && python3 flows/...`
/// because `shell_exec`'s default cwd is the sender_data_dir, not the
/// workspace root — so a relative `python3 flows/...` can't find the script.
/// This canonical agent pattern is safe ONLY when `<DIR>` is inside the
/// workspace and `<REST>` carries no further chaining metacharacters. The
/// workspace check is done here; the "REST has no extra metacharacters" check
/// is enforced by `subprocess_sandbox::is_safe_cd_and_chain` (the metacharacter
/// gate), and `<REST>` must still match a `shell_allow` pattern (the caller
/// runs the existing match tiers against the returned `REST`).
///
/// Returns `Some((rest, cd_dir))` when:
/// - the command is exactly `cd <DIR> && <REST>` (single `&&`, `cd` is the
///   first token),
/// - `<DIR>` resolves (relative to `workspace_root` if given) to a path that
///   canonicalizes inside `workspace_root`,
/// - `<REST>` is non-empty.
///
/// Otherwise `None` — the caller falls through to the normal match tiers
/// (which will reject an out-of-workspace `cd` or a non-matching REST).
///
/// `cd_dir` is the resolved absolute directory the execution layer should use
/// as `current_dir`; the match layer only consumes `rest`.
pub fn strip_cd_prefix(
    command: &str,
    workspace_root: Option<&std::path::Path>,
) -> Option<(String, std::path::PathBuf)> {
    let trimmed = command.trim();
    // First token must be `cd`. Split off "cd <rest>".
    let after_cd = trimmed.strip_prefix("cd")?;
    // `cd` must be a whole word, not e.g. "cddir".
    let next_char = after_cd.chars().next()?;
    if !next_char.is_whitespace() {
        return None;
    }
    let after_cd = after_cd.trim_start();

    // Split on the first " && " (single occurrence expected). Find the literal
    // "&&" surrounded by whitespace so "cd a&&b" (no spaces) is NOT accepted —
    // that form is unusual for agents and risks mis-parsing.
    let amp = after_cd.find(" && ")?;
    let dir_str = after_cd[..amp].trim();
    let rest = after_cd[amp + 4..].trim();
    if dir_str.is_empty() || rest.is_empty() {
        return None;
    }
    // Reject any further "&&" in REST — only a single cd&& chain is allowed.
    if rest.contains("&&") || rest.contains("||") {
        return None;
    }

    // Strip surrounding quotes from the dir (single or double).
    let dir_str = strip_one_quote_pair(dir_str);

    // Resolve the cd target. Relative dirs resolve against workspace_root;
    // without workspace_root we can only accept absolute paths that the caller
    // will have to trust (the metachar gate still guards REST).
    let resolved = if let Some(ws) = workspace_root {
        let p = if std::path::Path::new(&dir_str).is_absolute() {
            std::path::PathBuf::from(&dir_str)
        } else {
            ws.join(dir_str)
        };
        // Canonicalize both to compare real paths (symlinks, .., etc.). If the
        // dir doesn't exist yet, fall back to lexical normalize + starts_with.
        match p.canonicalize() {
            Ok(canon) => {
                let ws_canon = ws.canonicalize().ok()?;
                if !canon.starts_with(&ws_canon) {
                    return None;
                }
                canon
            }
            Err(_) => {
                // Path doesn't exist (e.g. typo) — don't allow. cd to a
                // nonexistent dir would fail at exec anyway; safer to reject
                // here so the agent gets a clear "not in shell_allow".
                return None;
            }
        }
    } else {
        // No workspace_root: can't verify containment. Return the raw path so
        // the exec layer can try it, but the match layer (no workspace strip)
        // will still require REST to match a pattern literally. This path is
        // only reached for non-flow / non-workspace contexts.
        std::path::PathBuf::from(&dir_str)
    };

    Some((rest.to_string(), resolved))
}

/// Strip one layer of matching surrounding quotes from a token.
fn strip_one_quote_pair(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (q, rest) = (bytes[0], &s[1..s.len() - 1]);
        if (q == b'"' || q == b'\'') && s.ends_with(q as char) {
            return rest;
        }
    }
    s
}

pub fn command_matches_flow_shell_allow(
    command: &str,
    patterns: &[String],
    workspace_root: Option<&std::path::Path>,
) -> bool {
    // `cd <DIR> && <REST>`: agents use this to reach flow scripts from the
    // sender_data_dir cwd. Strip the cd prefix (verifying <DIR> is inside the
    // workspace) and match the REST against the patterns via the normal tiers
    // below. See `strip_cd_prefix` for the safety rationale.
    if let Some((rest, _)) = strip_cd_prefix(command, workspace_root) {
        if command_matches_flow_shell_allow(&rest, patterns, workspace_root) {
            return true;
        }
        // Fall through: a cd&& command whose REST doesn't match must NOT be
        // allowed by the raw/workspace tiers against the full `cd ... && ...`
        // string either — those would also fail. Return false directly to
        // avoid the dotdot/normalize tiers re-parsing the `&&` as a path.
        return false;
    }

    // A `..` path segment can traverse out of a pattern's directory prefix.
    // The raw + workspace-strip tiers below match on starts_with(prefix), which
    // a `../../../` suffix defeats (escapes the allowed dir yet still
    // "matches"). For `..`-bearing commands skip them and rely on the normalize
    // tier, which lexically collapses `..`: escapes lose the prefix (denied);
    // legit `../../flows/...` collapses back under it (allowed).
    let has_dotdot = command_has_dotdot_segment(command);
    if !has_dotdot && command_matches_shell_allow(command, patterns) {
        return true;
    }
    if let Some(ws) = workspace_root.and_then(|p| p.to_str()) {
        if !ws.is_empty() {
            let prefix = format!("{ws}/");
            // Remove the workspace_root prefix so a relative pattern
            // (e.g. `python3 flows/foo/*`) matches the absolute command
            // (e.g. `python3 {ws}/flows/foo/scripts/x.py`). Only the first
            // occurrence is replaced — the command text before and after the
            // path is preserved.
            let rel = command.replacen(&prefix, "", 1);
            if !has_dotdot && rel != command && command_matches_shell_allow(&rel, patterns) {
                return true;
            }
            // Agent may be running from sender_data_dir cwd
            // (workspace_root/senders/{owner}/). After stripping workspace_root/,
            // the rel looks like `python3 senders/{owner}/.flows/foo/scripts/x.py`.
            // Strip senders/{owner}/ too so `.flows/foo/*` matches.
            if let Some(sender_idx) = rel.find("/senders/") {
                let before = &rel[..sender_idx + 1]; // text before /senders/ (e.g. "python3 ")
                let after_sender = &rel[sender_idx + 9..]; // after /senders/
                if let Some(rest) = after_sender.split_once('/').map(|x| x.1) {
                    let without_sender = format!("{before}{rest}");
                    if !has_dotdot && command_matches_shell_allow(&without_sender, patterns) {
                        return true;
                    }
                }
            }
        }
    }
    // Agent may run flow scripts via a relative `../../flows/...` path from the
    // sender_data_dir cwd (workspace_root/senders/{owner}/ -> `../../` reaches
    // workspace_root). Lexically collapse `..` components so
    // `python3 ../../flows/foo/scripts/x.py` matches a
    // `python3 flows/foo/scripts/*` pattern. Also strips leading `VAR=value`
    // env assignments the agent may prepend (e.g. `PYTHONPATH=../../flows/...
    // python3 ...`). Security-neutral: the same glob match runs against the
    // canonicalized form - `..`/env-prefix manipulation that escapes the
    // pattern's directory collapses to a path that no longer shares the
    // pattern's prefix, so it is still denied.
    let normalized = normalize_command_for_match(command);
    if normalized != command && command_matches_shell_allow(&normalized, patterns) {
        return true;
    }
    false
}

/// Normalize a shell command for `shell_allow` matching. Strips leading
/// `VAR=value` env assignments (`PYTHONPATH=... ` etc.), then lexically
/// collapses `..` path components in each remaining token
/// (`../../flows/foo/x.py` -> `flows/foo/x.py`; `a/b/../c` -> `a/c`).
/// Tokens without `/` are left unchanged. Pure lexical normalization (no
/// filesystem access) used only to match a relative command against a relative
/// `shell_allow` pattern.
fn normalize_command_for_match(command: &str) -> String {
    let mut tokens: Vec<&str> = command.split_whitespace().collect();
    // Strip leading env-var assignments (VAR=value) - the agent may prepend
    // PYTHONPATH=... before the actual command.
    while let Some(first) = tokens.first() {
        if let Some(eq) = first.find('=') {
            let name = &first[..eq];
            let valid_name = !name.is_empty()
                && name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if valid_name {
                tokens.remove(0);
                continue;
            }
        }
        break;
    }
    tokens
        .iter()
        .map(|tok| {
            if !tok.contains('/') {
                return tok.to_string();
            }
            let mut stack: Vec<&str> = Vec::new();
            for part in tok.split('/') {
                match part {
                    ".." => {
                        stack.pop();
                    }
                    "." | "" => {}
                    other => stack.push(other),
                }
            }
            stack.join("/")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_allow_glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == text;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 2 {
        let (prefix, suffix) = (parts[0], parts[1]);
        return text.starts_with(prefix)
            && text.ends_with(suffix)
            && text.len() >= prefix.len() + suffix.len();
    }
    // Multi-star: sequential scan
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if i == 0 {
            if !rest.starts_with(part) {
                return false;
            }
            rest = &rest[part.len()..];
        } else if i == parts.len() - 1 {
            if !rest.ends_with(part) {
                return false;
            }
        } else if let Some(pos) = rest.find(part) {
            rest = &rest[pos + part.len()..];
        } else {
            return false;
        }
    }
    true
}

/// Parse the nested `steps:` block into step definitions.
/// `start` is the index of the `steps:` line.
fn parse_steps_block(lines: &[&str], start: usize) -> (Vec<StepDef>, usize) {
    // Gather block lines (indent > 0) following `steps:`.
    let mut block: Vec<(usize, String)> = Vec::new();
    let mut j = start + 1;
    while j < lines.len() {
        let raw = lines[j];
        if raw.trim().is_empty() {
            j += 1;
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        if indent == 0 {
            break;
        }
        block.push((indent, raw.trim_start().to_string()));
        j += 1;
    }
    let consumed = j - start;
    if block.is_empty() {
        return (Vec::new(), consumed);
    }

    (parse_step_list(&block), consumed)
}

/// Parse a list of step definitions from a block of `(indent, text)` lines.
/// Self-contained: computes its own `item_indent`/`field_indent` from the
/// block, so it can be called recursively for a nested `body:` step list.
fn parse_step_list(block: &[(usize, String)]) -> Vec<StepDef> {
    if block.is_empty() {
        return Vec::new();
    }

    // Items begin with `- ` at the minimum indent among dash-lines.
    let item_indent = block
        .iter()
        .filter(|(_, t)| t.starts_with('-'))
        .map(|(ind, _)| *ind)
        .min()
        .unwrap_or(2);
    let field_indent = item_indent + 2; // fields align after "- "

    let mut steps: Vec<StepDef> = Vec::new();
    // When collecting a block field (depends_on list / with map / body step
    // list), holds its name.
    let mut pending: Option<String> = None;
    // Buffered lines for a nested `body:` step list (indent > field_indent).
    let mut body_buf: Vec<(usize, String)> = Vec::new();

    /// Flush a completed `body:` sub-block onto the current step.
    fn flush_body(steps: &mut [StepDef], body_buf: &mut Vec<(usize, String)>) {
        if let Some(s) = steps.last_mut() {
            if !body_buf.is_empty() {
                s.body = Some(parse_step_list(body_buf));
            }
        }
        body_buf.clear();
    }

    for (indent, text) in block {
        let t = text.as_str();
        let is_dash = t.starts_with('-');

        // While collecting a `body:` step list, absorb deeper-indented lines
        // until we drop back to field_indent or shallower.
        if pending.as_deref() == Some("body") {
            if *indent > field_indent {
                body_buf.push((*indent, t.to_string()));
                continue;
            }
            // Left the body block: flush it, then process this line normally.
            flush_body(&mut steps, &mut body_buf);
            pending = None;
        }

        // New step item: `  - id: draft`
        if *indent == item_indent && is_dash {
            pending = None;
            let mut s = StepDef::default();
            let first = t.strip_prefix('-').unwrap_or(t).trim();
            if !first.is_empty() {
                pending = apply_step_field(&mut s, first);
            }
            steps.push(s);
            continue;
        }

        let Some(s) = steps.last_mut() else { continue };

        // Continuation of a pending block field.
        match pending.as_deref() {
            Some("depends_on") if is_dash => {
                let v = unquote(t.strip_prefix('-').unwrap_or(t).trim());
                if !v.is_empty() {
                    s.depends_on.push(v);
                }
                continue;
            }
            Some("cancel_keywords") if is_dash => {
                let v = unquote(t.strip_prefix('-').unwrap_or(t).trim());
                if !v.is_empty() {
                    s.cancel_keywords.push(v);
                }
                continue;
            }
            Some("with") if !is_dash && *indent > field_indent => {
                let (k, v) = split_kv(t);
                if !k.is_empty() {
                    s.with.insert(k, Value::String(unquote(&v)));
                }
                continue;
            }
            _ => {}
        }

        // A field line at field_indent (ends any pending block).
        if !is_dash && *indent == field_indent {
            pending = apply_step_field(s, t);
        }
        // Anything else (deeper non-matching content) is ignored.
    }

    // Flush a `body:` block left open at end of input.
    if pending.as_deref() == Some("body") {
        flush_body(&mut steps, &mut body_buf);
    }

    steps
}

/// Apply a single `key: value` field to a step. Returns `Some(field_name)` when
/// the value is empty and the field opens a block (depends_on / with) that the
/// caller should collect from subsequent lines.
fn apply_step_field(s: &mut StepDef, text: &str) -> Option<String> {
    let (k, v) = split_kv(text);
    let v = unquote(&v);
    match k.as_str() {
        "id" => s.id = v,
        "kind" => s.kind = Some(StepKind::parse(&v)),
        "depends_on" => {
            if v.is_empty() {
                return Some("depends_on".into());
            }
            s.depends_on = parse_inline_list(&v);
        }
        "when" => s.when = (!v.is_empty()).then_some(v),
        "on_failure" => s.on_failure = (!v.is_empty()).then_some(v),
        "output" => s.output = (!v.is_empty()).then_some(v),
        "prompt" => s.prompt = (!v.is_empty()).then_some(v),
        "task" => s.task = (!v.is_empty()).then_some(v),
        "tool" | "tool_name" => s.tool_name = (!v.is_empty()).then_some(v),
        "tool_args" => {
            if !v.is_empty() {
                s.tool_args = parse_value(&v);
            }
        }
        "with" => {
            if v.is_empty() {
                return Some("with".into());
            }
            s.with = parse_inline_map(&v);
        }
        "cancel_keywords" => {
            if v.is_empty() {
                return Some("cancel_keywords".into());
            }
            s.cancel_keywords = parse_inline_list(&v);
        }
        "timeout_hours" => s.timeout_hours = (!v.is_empty()).then(|| v.parse().ok()).flatten(),
        "flow" => s.flow = (!v.is_empty()).then_some(v),
        "over" => s.over = (!v.is_empty()).then_some(v),
        "as" => s.as_name = (!v.is_empty()).then_some(v),
        "parallel" => s.parallel = (!v.is_empty()).then(|| v.parse().ok()).flatten(),
        // Block form (`body:` on its own line) opens a nested step list
        // collected by `parse_step_list`; inline form is unsupported.
        "body" if v.is_empty() => return Some("body".into()),
        _ => {}
    }
    None
}

/// Split `key: value` (value may be empty). Trims both sides; value keeps its
/// raw form (quotes stripped later by [`unquote`]).
fn split_kv(text: &str) -> (String, String) {
    match text.split_once(':') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    }
}

/// Trim and strip surrounding single/double quotes.
fn unquote(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s);
    let s = s
        .strip_prefix('\'')
        .and_then(|x| x.strip_suffix('\''))
        .unwrap_or(s);
    s.trim().to_string()
}

/// Parse an inline list `[a, b, "c"]` (also tolerates a bare `a, b`).
fn parse_inline_list(s: &str) -> Vec<String> {
    let s = s.trim();
    let inner = s
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(s);
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(unquote)
        .filter(|x| !x.is_empty())
        .collect()
}

/// Parse an inline map `{k: v, k2: v2}` (values become JSON strings).
fn parse_inline_map(s: &str) -> Map<String, Value> {
    let s = s.trim();
    let inner = s
        .strip_prefix('{')
        .and_then(|x| x.strip_suffix('}'))
        .unwrap_or(s);
    let mut m = Map::new();
    if inner.trim().is_empty() {
        return m;
    }
    for pair in inner.split(',') {
        if let Some((k, v)) = pair.split_once(':') {
            let k = unquote(k);
            if !k.is_empty() {
                m.insert(k, Value::String(unquote(v)));
            }
        }
    }
    m
}

/// Parse a value that may be an inline map (`{k: v}`), inline list (`[a, b]`),
/// or a bare string. Keys/values are YAML-ish (quotes optional); values become
/// JSON strings, sufficient for template-string step args.
fn parse_value(s: &str) -> Value {
    let s = s.trim();
    if s.starts_with('{') {
        Value::Object(parse_inline_map(s))
    } else if s.starts_with('[') {
        Value::Array(
            parse_inline_list(s)
                .into_iter()
                .map(Value::String)
                .collect(),
        )
    } else {
        Value::String(unquote(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_step_no_steps() {
        let content = r#"---
name: analyze
description: 定居点分析
tools: ["file_read", "web_search"]
---
# Analyze
Do the thing."#;
        let f = parse_flow_def(content);
        assert_eq!(f.name, "analyze");
        assert_eq!(f.description, "定居点分析");
        assert_eq!(f.tools, vec!["file_read", "web_search"]);
        assert!(f.steps.is_empty());
        assert!(!f.is_multi_step());
        assert_eq!(f.body, "# Analyze\nDo the thing.");
    }

    #[test]
    fn multiline_tools_block() {
        let content = r#"---
name: t
description: d
tools:
  - web_search
  - knowledge_add
---
body"#;
        let f = parse_flow_def(content);
        assert_eq!(f.tools, vec!["web_search", "knowledge_add"]);
    }

    /// Elevation predicate: shell_exec/process_start + non-empty shell_allow.
    /// All four combinations — this is the gate both for turn-start flow
    /// matching and for mid-turn `flow_load` grants.

    #[test]
    fn elevates_predicate_matrix() {
        // Placeholders carry their own brackets (args are full inline lists).
        let base = r#"---
name: t
description: d
tools: {tools}
shell_allow: {allow}
---
body"#;
        let mk = |tools: &str, allow: &str| {
            parse_flow_def(&base.replace("{tools}", tools).replace("{allow}", allow))
        };

        // shell_exec + shell_allow => elevates
        assert!(mk(
            r#"["shell_exec", "file_read"]"#,
            r#"["python3 flows/t/scripts/*"]"#
        )
        .elevates());
        // process_start also counts
        assert!(mk(r#"["process_start"]"#, r#"["./bin/*"]"#).elevates());
        // shell_exec but empty shell_allow => no elevation (nothing to scope to)
        assert!(!mk(r#"["shell_exec"]"#, "[]").elevates());
        // shell_allow but no execution tool => no elevation
        assert!(!mk(r#"["file_read"]"#, r#"["python3 x"]"#).elevates());
    }

    #[test]
    fn tools_block_stops_at_next_key() {
        let content = r#"---
name: t
description: d
tools:
  - foo
  - bar
version: 2
---
body"#;
        let f = parse_flow_def(content);
        assert_eq!(f.tools, vec!["foo", "bar"]);
    }

    #[test]
    fn multi_step_dag_basic() {
        let content = r#"---
name: short-drama
description: 生成短剧
tools: [file_write]
steps:
  - id: draft
    kind: agent_loop
  - id: review
    kind: chat
    depends_on: [draft]
  - id: deliver
    kind: chat
    depends_on: [review]
final: deliver
---
共享说明 body"#;
        let f = parse_flow_def(content);
        assert!(f.is_multi_step());
        assert_eq!(f.steps.len(), 3);
        assert_eq!(f.steps[0].id, "draft");
        assert_eq!(f.steps[0].kind, Some(StepKind::AgentLoop));
        assert_eq!(f.steps[1].id, "review");
        assert_eq!(f.steps[1].kind, Some(StepKind::Chat));
        assert_eq!(f.steps[1].depends_on, vec!["draft"]);
        assert_eq!(f.steps[2].depends_on, vec!["review"]);
        assert_eq!(f.final_step.as_deref(), Some("deliver"));
        assert_eq!(f.body, "共享说明 body");
    }

    #[test]
    fn step_when_and_output() {
        let content = r#"---
name: t
description: d
steps:
  - id: draft
    kind: agent_loop
  - id: revise
    kind: agent_loop
    when: "review.decision == 'revise'"
    depends_on: [draft]
    output: file:output/script.txt
---
b"#;
        let f = parse_flow_def(content);
        let revise = f.steps.iter().find(|s| s.id == "revise").unwrap();
        assert_eq!(revise.when.as_deref(), Some("review.decision == 'revise'"));
        assert_eq!(revise.output.as_deref(), Some("file:output/script.txt"));
        assert_eq!(
            revise.output_mode(),
            StepOutputMode::File("output/script.txt".into())
        );
    }

    #[test]
    fn step_output_json_and_llm_default() {
        let content = r#"---
name: t
description: d
steps:
  - id: a
    kind: chat
    output: json
  - id: b
    kind: chat
---
b"#;
        let f = parse_flow_def(content);
        assert_eq!(f.steps[0].output_mode(), StepOutputMode::Json);
        assert_eq!(f.steps[1].output_mode(), StepOutputMode::Llm);
    }

    #[test]
    fn step_on_failure() {
        let content = r#"---
name: t
description: d
steps:
  - id: video
    kind: agent_loop
    on_failure: fallback
  - id: fallback
    kind: agent_loop
---
b"#;
        let f = parse_flow_def(content);
        assert_eq!(f.steps[0].on_failure.as_deref(), Some("fallback"));
    }

    #[test]
    fn step_with_inline_map() {
        let content = r#"---
name: t
description: d
steps:
  - id: draft
    kind: flow_exec
    flow: script-writing
    with: {topic: "{{ input.topic }}"}
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(s.kind, Some(StepKind::FlowExec));
        assert_eq!(
            s.with.get("topic").and_then(|v| v.as_str()),
            Some("{{ input.topic }}")
        );
    }

    #[test]
    fn step_with_block_map() {
        let content = r#"---
name: t
description: d
steps:
  - id: draft
    kind: agent_loop
    with:
      topic: "{{ input.topic }}"
      count: "3"
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(
            s.with.get("topic").and_then(|v| v.as_str()),
            Some("{{ input.topic }}")
        );
        assert_eq!(s.with.get("count").and_then(|v| v.as_str()), Some("3"));
    }

    #[test]
    fn step_depends_on_block_list() {
        let content = r#"---
name: t
description: d
steps:
  - id: deliver
    kind: chat
    depends_on:
      - draft
      - review
---
b"#;
        let f = parse_flow_def(content);
        assert_eq!(f.steps[0].depends_on, vec!["draft", "review"]);
    }

    #[test]
    fn tool_step_args() {
        let content = r#"---
name: t
description: d
steps:
  - id: save
    kind: tool
    tool: file_write
    tool_args: {path: "out.txt", content: "hi"}
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(s.kind, Some(StepKind::Tool));
        assert_eq!(s.tool_name.as_deref(), Some("file_write"));
        assert_eq!(s.tool_args["path"].as_str(), Some("out.txt"));
        assert_eq!(s.tool_args["content"].as_str(), Some("hi"));
    }

    #[test]
    fn map_parallel_field_parses() {
        let content = r#"---
name: batch
description: d
steps:
  - id: fan
    kind: map
    over: "{{ items }}"
    as: item
    flow: sub
    parallel: 4
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(s.kind, Some(StepKind::Map));
        assert_eq!(s.parallel, Some(4));
        // Omitted => None (serial default).
        let content2 = r#"---
name: batch2
description: d
steps:
  - id: fan
    kind: map
    over: "{{ items }}"
    flow: sub
---
b"#;
        let f2 = parse_flow_def(content2);
        assert_eq!(f2.steps[0].parallel, None);
    }

    #[test]
    fn entry_and_top_output() {
        let content = r#"---
name: shot-image
description: d
entry: false
output: json
---
body"#;
        let f = parse_flow_def(content);
        assert_eq!(f.entry, Some(false));
        assert_eq!(f.output.as_deref(), Some("json"));
    }

    #[test]
    fn no_frontmatter_returns_body() {
        let f = parse_flow_def("just a body, no frontmatter");
        assert!(f.name.is_empty());
        assert_eq!(f.body, "just a body, no frontmatter");
    }

    #[test]
    fn empty_steps_array_is_single_step() {
        let content = "---\nname: t\ndescription: d\nsteps: []\n---\nbody";
        let f = parse_flow_def(content);
        assert!(!f.is_multi_step());
        assert!(f.steps.is_empty());
    }

    #[test]
    fn unknown_kind_preserved() {
        let content = r#"---
name: t
description: d
steps:
  - id: g
    kind: delegate
    prompt: "ok?"
---
b"#;
        let f = parse_flow_def(content);
        assert_eq!(f.steps[0].kind, Some(StepKind::Unknown("delegate".into())));
        assert!(!f.steps[0].kind.as_ref().unwrap().is_executable());
        assert_eq!(f.steps[0].prompt.as_deref(), Some("ok?"));
    }

    #[test]
    fn user_input_step_parsed() {
        let content = r#"---
name: t
description: d
steps:
  - id: review
    kind: user_input
    prompt: "继续？回复 ok/取消"
    cancel_keywords: [取消, cancel, 算了]
    timeout_hours: 24
    depends_on: [draft]
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(s.kind, Some(StepKind::UserInput));
        assert!(s.kind.as_ref().unwrap().is_executable());
        assert_eq!(s.cancel_keywords, vec!["取消", "cancel", "算了"]);
        assert_eq!(s.timeout_hours, Some(24.0));
        assert_eq!(s.depends_on, vec!["draft"]);
    }

    #[test]
    fn user_input_cancel_keywords_block_form() {
        let content = r#"---
name: t
description: d
steps:
  - id: review
    kind: user_input
    cancel_keywords:
      - 取消
      - cancel
---
b"#;
        let f = parse_flow_def(content);
        assert_eq!(f.steps[0].cancel_keywords, vec!["取消", "cancel"]);
    }

    #[test]
    fn flow_exec_step_parsed() {
        let content = r#"---
name: t
description: d
steps:
  - id: draft
    kind: flow_exec
    flow: script-writing
    with: {topic: "{{ input.user_message }}", count: "3"}
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(s.kind, Some(StepKind::FlowExec));
        assert!(s.kind.as_ref().unwrap().is_executable());
        assert_eq!(s.flow.as_deref(), Some("script-writing"));
        assert_eq!(
            s.with.get("topic").and_then(|v| v.as_str()),
            Some("{{ input.user_message }}")
        );
        assert_eq!(s.with.get("count").and_then(|v| v.as_str()), Some("3"));
    }

    #[test]
    fn map_step_parsed() {
        let content = r#"---
name: t
description: d
steps:
  - id: shots
    kind: map
    over: "{{ parse_shots }}"
    as: shot
    flow: shot-image
    with: {prompt: "{{ shot.prompt }}"}
    depends_on: [parse_shots]
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(s.kind, Some(StepKind::Map));
        assert!(s.kind.as_ref().unwrap().is_executable());
        assert_eq!(s.over.as_deref(), Some("{{ parse_shots }}"));
        assert_eq!(s.as_name.as_deref(), Some("shot"));
        assert_eq!(s.flow.as_deref(), Some("shot-image"));
        assert_eq!(
            s.with.get("prompt").and_then(|v| v.as_str()),
            Some("{{ shot.prompt }}")
        );
    }

    #[test]
    fn tool_step_is_executable() {
        // Tool is now executed by run_flow (stage 2 step-kind set complete).
        assert_eq!(StepKind::parse("tool"), StepKind::Tool);
        assert!(StepKind::Tool.is_executable());
        assert!(StepKind::FlowExec.is_executable());
        assert!(StepKind::Map.is_executable());
        // Unknown kinds remain non-executable.
        assert!(!StepKind::Unknown("delegate".into()).is_executable());
    }

    #[test]
    fn map_step_with_inline_body_parsed() {
        let content = r#"---
name: t
description: d
steps:
  - id: per_ep
    kind: map
    over: "{{ eps }}"
    as: ep
    body:
      - id: write
        kind: agent_loop
        output: file:out.md
      - id: review_episode
        kind: user_input
        depends_on: [write]
        prompt: "第{{ep.index}}集写完。继续/停止？"
        cancel_keywords:
          - 停止
          - stop
---
b"#;
        let f = parse_flow_def(content);
        let s = &f.steps[0];
        assert_eq!(s.kind, Some(StepKind::Map));
        assert_eq!(s.over.as_deref(), Some("{{ eps }}"));
        assert_eq!(s.as_name.as_deref(), Some("ep"));
        let body = s.body.as_ref().expect("body parsed");
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].id, "write");
        assert_eq!(body[0].kind, Some(StepKind::AgentLoop));
        assert_eq!(body[0].output.as_deref(), Some("file:out.md"));
        assert_eq!(body[1].id, "review_episode");
        assert_eq!(body[1].kind, Some(StepKind::UserInput));
        assert_eq!(body[1].depends_on, vec!["write"]);
        assert_eq!(body[1].cancel_keywords, vec!["停止", "stop"]);
        assert_eq!(
            body[1].prompt.as_deref(),
            Some("第{{ep.index}}集写完。继续/停止？")
        );
    }

    #[test]
    fn map_step_with_block_field_after_body() {
        // A top-level field (`depends_on`) after the `body:` block must close
        // the body correctly and still be parsed.
        let content = r#"---
name: t
description: d
steps:
  - id: eps
    kind: chat
    output: json
  - id: per_ep
    kind: map
    over: "{{ eps }}"
    as: ep
    body:
      - id: write
        kind: chat
      - id: review
        kind: user_input
    depends_on: [eps]
final: per_ep
---
b"#;
        let f = parse_flow_def(content);
        assert_eq!(f.steps.len(), 2);
        let per_ep = f.steps.iter().find(|s| s.id == "per_ep").unwrap();
        assert_eq!(per_ep.body.as_ref().unwrap().len(), 2);
        assert_eq!(per_ep.depends_on, vec!["eps"]);
        assert_eq!(f.final_step.as_deref(), Some("per_ep"));
    }

    #[test]
    fn privilege_and_shell_allow_parsed() {
        let content = r#"---
name: office-xlsx
description: gen excel
privilege: system
tools:
  - file_write
  - shell_exec
shell_allow:
  - "python3 output/scripts/*"
  - python output/scripts/*
---
body"#;
        let f = parse_flow_def(content);
        assert_eq!(f.privilege, FlowPrivilege::System);
        assert_eq!(f.tools, vec!["file_write", "shell_exec"]);
        assert_eq!(
            f.shell_allow,
            vec![
                "python3 output/scripts/*".to_string(),
                "python output/scripts/*".to_string()
            ]
        );
        assert_eq!(
            f.required_max_tool_level(),
            crate::tool::PermissionLevel::Dangerous
        );
    }

    #[test]
    fn privilege_defaults_to_agent() {
        let content = r#"---
name: t
description: d
tools: [file_read]
---
b"#;
        let f = parse_flow_def(content);
        assert_eq!(f.privilege, FlowPrivilege::Agent);
        assert!(f.shell_allow.is_empty());
    }

    #[test]
    fn shell_allow_glob_matches() {
        let patterns = vec!["python3 output/scripts/*".to_string()];
        assert!(command_matches_shell_allow(
            "python3 output/scripts/gen_xlsx_a.py",
            &patterns
        ));
        assert!(!command_matches_shell_allow("rm -rf /", &patterns));
        assert!(!command_matches_shell_allow(
            "python3 /tmp/evil.py",
            &patterns
        ));
        assert!(!command_matches_shell_allow(
            "python3 output/scripts/a.py",
            &[]
        ));
    }

    // ── shell_allow golden-sample parsing + validation ──────────────────

    #[test]
    fn shell_allow_map_entry_parses_golden_samples() {
        let content = r#"---
name: demo
description: d
shell_allow:
  - python3 output/scripts/*
  - pattern: python3 flows/demo/scripts/*
    match: [python3 flows/demo/scripts/run.py]
    not_match: [rm -rf /, bash -c id]
---
body"#;
        let f = parse_flow_def(content);
        assert_eq!(
            f.shell_allow,
            vec![
                "python3 output/scripts/*".to_string(),
                "python3 flows/demo/scripts/*".to_string()
            ]
        );
        assert_eq!(f.shell_allow_checks.len(), 1);
        let check = &f.shell_allow_checks[0];
        assert_eq!(check.pattern, "python3 flows/demo/scripts/*");
        assert_eq!(check.matches, vec!["python3 flows/demo/scripts/run.py"]);
        assert_eq!(check.not_matches, vec!["rm -rf /", "bash -c id"]);
    }

    #[test]
    fn validate_shell_allow_clean_flow_passes() {
        let def = FlowDef {
            shell_allow: vec!["python3 output/scripts/*".to_string()],
            shell_allow_checks: vec![ShellAllowCheck {
                pattern: "python3 output/scripts/*".to_string(),
                matches: vec!["python3 output/scripts/run.py".to_string()],
                not_matches: vec!["rm -rf /".to_string()],
            }],
            ..Default::default()
        };
        assert!(
            validate_shell_allow(&def).is_empty(),
            "{:?}",
            validate_shell_allow(&def)
        );
    }

    #[test]
    fn validate_shell_allow_rejects_star_and_forbidden_base() {
        let star = FlowDef {
            shell_allow: vec!["*".to_string()],
            ..Default::default()
        };
        let errs = validate_shell_allow(&star);
        assert!(errs.iter().any(|e| e.contains("'*'")), "{errs:?}");

        let rm = FlowDef {
            shell_allow: vec!["rm -rf *".to_string()],
            ..Default::default()
        };
        let errs = validate_shell_allow(&rm);
        assert!(errs.iter().any(|e| e.contains("'rm'")), "{errs:?}");

        let mkfs = FlowDef {
            shell_allow: vec!["mkfs.ext4 *".to_string()],
            ..Default::default()
        };
        let errs = validate_shell_allow(&mkfs);
        assert!(errs.iter().any(|e| e.contains("mkfs.ext4")), "{errs:?}");
    }

    #[test]
    fn validate_shell_allow_catches_too_narrow_and_too_broad() {
        // Too narrow: a match example the pattern can never authorize.
        let narrow = FlowDef {
            shell_allow: vec!["python3 output/scripts/run.py".to_string()],
            shell_allow_checks: vec![ShellAllowCheck {
                pattern: "python3 output/scripts/run.py".to_string(),
                matches: vec!["python3 output/scripts/other.py".to_string()],
                not_matches: vec![],
            }],
            ..Default::default()
        };
        let errs = validate_shell_allow(&narrow);
        assert!(errs.iter().any(|e| e.contains("match 示例")), "{errs:?}");

        // Too broad: a not_match example the pattern WOULD authorize.
        let broad = FlowDef {
            shell_allow: vec!["python3 *".to_string()],
            shell_allow_checks: vec![ShellAllowCheck {
                pattern: "python3 *".to_string(),
                matches: vec![],
                not_matches: vec!["python3 -c id".to_string()],
            }],
            ..Default::default()
        };
        let errs = validate_shell_allow(&broad);
        assert!(
            errs.iter().any(|e| e.contains("not_match 示例")),
            "{errs:?}"
        );
    }

    #[test]
    fn validate_shell_allow_rejects_unknown_pattern_reference() {
        let def = FlowDef {
            shell_allow: vec!["python3 output/*".to_string()],
            shell_allow_checks: vec![ShellAllowCheck {
                pattern: "python3 nonexistent/*".to_string(),
                matches: vec![],
                not_matches: vec![],
            }],
            ..Default::default()
        };
        let errs = validate_shell_allow(&def);
        assert!(errs.iter().any(|e| e.contains("不存在")), "{errs:?}");
    }

    #[test]
    fn flow_shell_allow_matches_absolute_workspace_path() {
        // Flow authors write workspace-relative patterns; agents often use the
        // absolute workspace path they discovered. Stripping workspace_root lets
        // the relative pattern match the absolute command at the same script.
        let patterns = vec!["python3 flows/art-director/*".to_string()];
        let ws = std::path::Path::new("/home/u/.aginx/carrier/workspaces/demo");
        let abs = "python3 /home/u/.aginx/carrier/workspaces/demo/flows/art-director/scripts/build.py --x 1";
        assert!(command_matches_flow_shell_allow(abs, &patterns, Some(ws)));
        // Same command without workspace_root insight must NOT match a relative
        // pattern (the original command_matches_shell_allow returns false here).
        assert!(!command_matches_shell_allow(abs, &patterns));
        // A path outside the workspace is still denied.
        assert!(!command_matches_flow_shell_allow(
            "python3 /tmp/evil.py",
            &patterns,
            Some(ws)
        ));
        // Empty workspace_root → falls back to literal match only.
        assert!(!command_matches_flow_shell_allow(abs, &patterns, None));
    }

    #[test]
    fn flow_shell_allow_matches_cd_and_chain() {
        // Agents run `cd <workspace_root> && python3 flows/...` because
        // shell_exec's default cwd is the sender_data_dir. strip_cd_prefix
        // verifies <DIR> is inside the workspace, then matches <REST>.
        let patterns = vec!["python3 flows/topic-researcher/scripts/*".to_string()];

        // Real temp workspace so canonicalize succeeds.
        let ws = std::env::temp_dir().join(format!("flow-cd-test-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        // Relative cd target resolves against workspace_root.
        let cmd_rel = format!(
            "cd {} && python3 flows/topic-researcher/scripts/validate.py arg",
            ws.display()
        );
        assert!(command_matches_flow_shell_allow(
            &cmd_rel,
            &patterns,
            Some(&ws)
        ));

        // cd into a subdir of the workspace is allowed, but REST must still
        // match the pattern (which is workspace-relative). cd-ing into the
        // scripts dir and running `python3 validate.py` does NOT match
        // `python3 flows/.../scripts/*` — the relative path changed. That is
        // correct: the agent should cd to workspace_root and run the full
        // `python3 flows/.../scripts/x.py`.
        std::fs::create_dir_all(ws.join("flows/topic-researcher/scripts")).unwrap();
        let cmd_sub = format!(
            "cd {} && python3 flows/topic-researcher/scripts/validate.py",
            ws.display()
        );
        assert!(command_matches_flow_shell_allow(
            &cmd_sub,
            &patterns,
            Some(&ws)
        ));

        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn flow_shell_allow_rejects_cd_outside_workspace() {
        let patterns = vec!["python3 flows/foo/scripts/*".to_string()];
        let ws = std::env::temp_dir().join(format!("flow-cd-out-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        // cd to /tmp (outside workspace) → strip_cd_prefix returns None →
        // the raw tiers against the full `cd /tmp && ...` string also fail.
        assert!(!command_matches_flow_shell_allow(
            "cd /tmp && python3 flows/foo/scripts/x.py",
            &patterns,
            Some(&ws)
        ));
        // cd to a nonexistent dir → canonicalize fails → None → denied.
        assert!(!command_matches_flow_shell_allow(
            "cd /nonexistent/pathxyz && python3 flows/foo/scripts/x.py",
            &patterns,
            Some(&ws)
        ));
        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn flow_shell_allow_rejects_cd_chain_when_rest_unmatched() {
        let patterns = vec!["python3 flows/foo/scripts/*".to_string()];
        let ws = std::env::temp_dir().join(format!("flow-cd-rest-{}", std::process::id()));
        std::fs::create_dir_all(&ws).unwrap();
        // REST doesn't match the pattern (cat, not python3 flows/...).
        assert!(!command_matches_flow_shell_allow(
            &format!("cd {} && cat flows/foo/scripts/x.py", ws.display()),
            &patterns,
            Some(&ws)
        ));
        std::fs::remove_dir_all(&ws).ok();
    }

    #[test]
    fn flow_shell_allow_matches_relative_dotdot_path() {
        // Flow instructs the agent to run `python3 ../../flows/...` from the
        // sender_data_dir cwd (workspace_root/senders/{owner}/ -> `../../`
        // reaches workspace_root). The relative pattern `python3 flows/...`
        // must match after lexically collapsing the leading `..`.
        let patterns = vec!["python3 flows/outline-writer/scripts/*".to_string()];
        let cmd = "python3 ../../flows/outline-writer/scripts/validate_outline.py output/pipeline-x/大纲.md";
        assert!(command_matches_flow_shell_allow(cmd, &patterns, None));
        // Also works with workspace_root set (the collapse is independent).
        let ws = std::path::Path::new("/home/u/.aginx/carrier/workspaces/ai-writer");
        assert!(command_matches_flow_shell_allow(cmd, &patterns, Some(ws)));

        // Security: `..` traversal that escapes the pattern's directory must
        // still be denied. `flows/foo/scripts/../../../etc/passwd` collapses to
        // `etc/passwd`, which does not share the `flows/...` prefix.
        assert!(!command_matches_flow_shell_allow(
            "python3 flows/foo/scripts/../../../etc/passwd",
            &patterns,
            None
        ));
        // A path outside the allowed scripts dir is denied.
        assert!(!command_matches_flow_shell_allow(
            "python3 ../../etc/passwd",
            &patterns,
            None
        ));
    }

    #[test]
    fn flow_shell_allow_strips_env_prefix_and_dotdot() {
        // Agent prepends a PYTHONPATH= env assignment (itself using a relative
        // ../../flows path) before `python3 ../../flows/...`. Both the env
        // prefix and the `..` must be normalized for the relative pattern to
        // match.
        let patterns = vec!["python3 flows/outline-writer/scripts/*".to_string()];
        let cmd = "PYTHONPATH=../../flows/outline-writer/scripts python3 ../../flows/outline-writer/scripts/validate_outline.py output/pipeline-x/大纲.md";
        assert!(command_matches_flow_shell_allow(cmd, &patterns, None));

        // Multiple env prefixes also stripped.
        let cmd2 =
            "A=1 B=../../flows/x python3 ../../flows/outline-writer/scripts/validate_outline.py";
        assert!(command_matches_flow_shell_allow(cmd2, &patterns, None));

        // Security: env prefix + traversal that escapes is still denied.
        assert!(!command_matches_flow_shell_allow(
            "PYTHONPATH=../../etc python3 ../../etc/passwd",
            &patterns,
            None
        ));
        // A --flag=value is NOT stripped as an env assignment (starts with `-`).
        assert!(!command_matches_flow_shell_allow(
            "python3 --config=../../etc/evil flows/outline-writer/scripts/x.py",
            &patterns,
            None
        ));
    }

    #[test]
    fn flow_shell_allow_denies_traversal_after_matching_prefix() {
        // The raw + workspace-strip tiers match on starts_with(prefix). A command
        // whose path SHARES the pattern's directory prefix but then traverses
        // out via `../../../` must be denied - previously the raw tier
        // short-circuited to allow (the existing dotdot test only passed by
        // using a different dir, so it never exercised the same-prefix escape).
        let patterns = vec!["python3 flows/foo/scripts/*".to_string()];
        // Same-prefix escape: raw starts_with would match, but `..` is detected
        // and the normalize tier collapses to `python3 etc/passwd` (no prefix).
        assert!(!command_matches_flow_shell_allow(
            "python3 flows/foo/scripts/../../../etc/passwd",
            &patterns,
            None,
        ));
        // Same with workspace_root set (workspace-strip tier is also gated).
        let ws = std::path::Path::new("/home/u/.aginx/carrier/workspaces/demo");
        assert!(!command_matches_flow_shell_allow(
            "python3 flows/foo/scripts/../../../etc/passwd",
            &patterns,
            Some(ws),
        ));
        // Legit relative `../../flows/foo/...` (.. BEFORE the allowed prefix)
        // still matches after normalize collapses it back under the prefix.
        assert!(command_matches_flow_shell_allow(
            "python3 ../../flows/foo/scripts/x.py",
            &patterns,
            None,
        ));
        // A filename containing `..` (foo..bar.py) is NOT a traversal segment
        // and must not trigger the skip - raw tier matches as before.
        assert!(command_matches_flow_shell_allow(
            "python3 flows/foo/scripts/foo..bar.py",
            &patterns,
            None,
        ));
    }

    #[test]
    fn deny_tools_parsed() {
        let content = r#"---
name: bus-schedule-poster
description: d
tools: [shell_exec, file_write]
deny_tools:
  - image_generate
  - video_generate
---
body"#;
        let f = parse_flow_def(content);
        assert_eq!(f.deny_tools, vec!["image_generate", "video_generate"]);
    }
}

#[cfg(test)]
mod report_matrix_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn continue_ok() {
        let v = json!({"status": "continue", "next_steps": ["写大纲", "查素材"]});
        assert!(validate_step_report(&v).is_ok());
    }

    #[test]
    fn continue_requires_next_steps() {
        assert!(validate_step_report(&json!({"status": "continue"})).is_err());
        assert!(validate_step_report(&json!({"status": "continue", "next_steps": []})).is_err());
        assert!(
            validate_step_report(&json!({"status": "continue", "next_steps": ["  "]})).is_err()
        );
    }

    #[test]
    fn continue_must_not_carry_blocker() {
        let v = json!({"status": "continue", "next_steps": ["x"], "blocker": "卡住了"});
        assert!(validate_step_report(&v).is_err());
    }

    #[test]
    fn complete_ok() {
        let v =
            json!({"status": "complete", "evidence": ["output/x.md 已生成", "sha256 校验通过"]});
        assert!(validate_step_report(&v).is_ok());
        // evidence as a plain string is fine too
        assert!(
            validate_step_report(&json!({"status": "complete", "evidence": "文件已生成"})).is_ok()
        );
    }

    #[test]
    fn complete_requires_evidence() {
        assert!(validate_step_report(&json!({"status": "complete"})).is_err());
        assert!(validate_step_report(&json!({"status": "complete", "evidence": ""})).is_err());
    }

    #[test]
    fn complete_must_not_carry_next_steps_or_blocker() {
        assert!(validate_step_report(
            &json!({"status": "complete", "evidence": "x", "next_steps": ["y"]})
        )
        .is_err());
        assert!(validate_step_report(
            &json!({"status": "complete", "evidence": "x", "blocker": "b"})
        )
        .is_err());
    }

    #[test]
    fn blocked_ok_and_constrained() {
        assert!(validate_step_report(
            &json!({"status": "blocked", "blocker": "缺少 API 凭证，无法继续"})
        )
        .is_ok());
        assert!(validate_step_report(&json!({"status": "blocked"})).is_err());
        assert!(validate_step_report(&json!({"status": "blocked", "blocker": "  "})).is_err());
        assert!(validate_step_report(
            &json!({"status": "blocked", "blocker": "x", "next_steps": ["y"]})
        )
        .is_err());
    }

    #[test]
    fn status_must_be_exact() {
        assert!(validate_step_report(&json!({"status": "done"})).is_err());
        assert!(validate_step_report(&json!({"status": ""})).is_err());
        assert!(validate_step_report(&json!({})).is_err());
        assert!(validate_step_report(&json!("just a string")).is_err());
        assert!(validate_step_report(&json!([])).is_err());
    }

    #[test]
    fn handoff_cap_enforced() {
        let big = "x".repeat(REPORT_HANDOFF_MAX_CHARS);
        let v = json!({"status": "blocked", "blocker": big});
        assert!(validate_step_report(&v).is_err());
    }

    #[test]
    fn extract_json_span_tolerates_fences_and_prose() {
        // The end_turn report gate feeds model output through this — models
        // wrap JSON in fences or lead with prose.
        assert_eq!(
            extract_json_span("```json\n{\"status\":\"complete\"}\n```"),
            Some("{\"status\":\"complete\"}")
        );
        assert_eq!(
            extract_json_span("好的，结果如下：{\"status\":\"blocked\",\"blocker\":\"x\"} 以上。"),
            Some("{\"status\":\"blocked\",\"blocker\":\"x\"}")
        );
        assert_eq!(extract_json_span("纯文字没有结构"), None);
        assert_eq!(extract_json_span("{"), None);
    }

    /// Golden sample: the flow example embedded in `docs/CLONE-FORMAT.md` must
    /// parse with THIS parser exactly as the doc promises. The doc is the
    /// published format spec (also seeded into every clone's
    /// `knowledge/format-spec.md`); if someone changes the parser without the
    /// doc — or vice versa — this test fails. That is the machine gate that
    /// keeps the spec from drifting, the failure mode that let clone-creator
    /// ship `when_to_use`/`SKILL.md` for a runtime that reads
    /// `description`/`flow.md`.
    #[test]
    fn clone_format_doc_golden_sample() {
        let doc = include_str!("../../../docs/CLONE-FORMAT.md");
        // Extract the fenced flow example under the flow definition heading.
        let canonical = doc
            .split("flows/<name>/flow.md — 流程定义")
            .nth(1)
            .expect("CLONE-FORMAT.md lost its flow definition section");
        let block = canonical
            .split("```markdown\n")
            .nth(1)
            .and_then(|s| s.split("\n```").next())
            .expect("CLONE-FORMAT.md lost its markdown example block");
        let def = parse_flow_def(block);
        assert_eq!(def.name, "flow-name", "doc example name must parse");
        assert!(
            !def.description.is_empty(),
            "doc example description must parse non-empty — the doc teaches that \
             empty description kills the flow, so its own example must demo the fix"
        );
        assert!(
            def.description.starts_with("一句话用途描述"),
            "doc example description drifted: {desc}",
            desc = def.description
        );
        // The example declares `tools` as a block array — must survive parsing.
        assert!(
            def.tools.iter().any(|t| t == "file_read"),
            "doc example tools block must parse, got {tools:?}",
            tools = def.tools
        );
        // `deny_tools` block too.
        assert!(def.deny_tools.iter().any(|t| t == "task_plan"));
        // `shell_allow` quoted glob.
        assert!(!def.shell_allow.is_empty(), "shell_allow must parse");
        // `shell_allow` map entry with golden samples must parse too.
        assert!(
            !def.shell_allow_checks.is_empty(),
            "doc example shell_allow map entry must parse into shell_allow_checks"
        );
        let check = &def.shell_allow_checks[0];
        assert_eq!(check.pattern, "python3 flows/<name>/scripts/render.py");
        assert!(!check.matches.is_empty(), "match examples must parse");
        assert!(
            !check.not_matches.is_empty(),
            "not_match examples must parse"
        );
        // max_iterations numeric.
        assert_eq!(def.max_iterations, Some(8));
        // The doc must also still teach the two hard rules; if these strings
        // vanish the enforcement copy has drifted as surely as the example.
        assert!(doc.contains("skills/"), "doc must document the skills/ ban");
        assert!(
            doc.contains("description"),
            "doc must document the description requirement"
        );
    }
}
