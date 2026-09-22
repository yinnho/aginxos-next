//! Shell execution tool module.

use super::ToolModule;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use tracing::warn;
use carrier_types::config::ExecSecurityMode;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::taint::{TaintLabel, TaintSink, TaintedValue};
use carrier_types::tool::ToolDefinition;

/// Resolve the `shell_exec`/`cli_exec` subprocess cwd.
///
/// When the turn is driven by a per-user channel (a sender is present), cd into
/// the sender-scoped data dir — the same dir `file_write` writes to — so a shell
/// pipeline can read files the agent just wrote this turn. Without a sender
/// (CLI/system turns), fall back to the workspace root. The path uses the same
/// `sender_data_dir` as the file API, so the two stay byte-aligned.
fn resolve_shell_cwd(ctx: &ToolContext<'_>) -> Option<std::path::PathBuf> {
    let workspace_root = ctx.workspace_root?;
    let home_dir = ctx.home_dir?;
    let agent_name = ctx.agent_name?;
    let dir = carrier_types::config::resolve_turn_cwd(
        home_dir,
        workspace_root,
        agent_name,
        ctx.sender_id,
        ctx.owner_id,
    );
    // `file_write` may not have created it yet this turn; ensure it exists
    // so `current_dir` (cd) doesn't fail.
    let _ = std::fs::create_dir_all(&dir);
    Some(dir)
}

/// Shell execution tools.
pub struct ShellTools;

#[async_trait]
impl ToolModule for ShellTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "shell_exec".to_string(),
            description: "Execute a shell command and return its output.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "The command to execute" },
                    "timeout_seconds": { "type": "integer", "description": "Timeout in seconds (default: 30)" }
                },
                "required": ["command"]
            }),
        }]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        if name != "shell_exec" {
            return None;
        }

        let command = input["command"].as_str().unwrap_or("");
        let exec_policy = ctx.exec_policy;
        let allowed_env = ctx.allowed_env_vars.unwrap_or(&[]);
        let shell_cwd = resolve_shell_cwd(ctx);
        // `cd <DIR> && <REST>`: agents reach flow scripts from the
        // sender_data_dir cwd via this pattern. strip_cd_prefix (verified
        // against the real workspace_root) yields (REST, cd_dir); we execute
        // only REST with cd_dir as cwd. The metachar gate already allowed this
        // single `cd &&` shape; the shell_allow match layer (tool_runner + the
        // re-check below) verified <DIR> is inside the workspace and <REST>
        // matches a pattern.
        let (effective_command, effective_cwd): (String, Option<std::path::PathBuf>) =
            if let Some((rest, cd_dir)) = carrier_types::flow::strip_cd_prefix(command, ctx.workspace_root)
            {
                (rest, Some(cd_dir))
            } else {
                (command.to_string(), shell_cwd)
            };
        let workspace_root = effective_cwd.as_deref();

        // SECURITY: Always check for shell metacharacters, even in Full mode.
        if let Some(reason) = crate::subprocess_sandbox::contains_shell_metacharacters(command) {
            return Some(Err(CarrierError::Sandbox(format!(
                "shell_exec blocked: command contains {reason}. \
                 Shell metacharacters are never allowed."
            ))));
        }

        // SECURITY: hard-banned dangerous prefixes — a floor that holds even in
        // Full mode or when a flow-scoped shell_allow pattern matched (both
        // otherwise bypass the exec allowlist). Checked against the effective
        // command so a `cd <ws> && rm -rf /` chain's REST is also caught.
        if let Some((reason, suggestion)) =
            crate::subprocess_sandbox::check_dangerous_prefix(&effective_command)
        {
            return Some(Err(CarrierError::Sandbox(format!(
                "shell_exec blocked: 危险命令（{reason}）。{suggestion}"
            ))));
        }

        // Flow-scoped shell_allow (private brand skills / system office flows):
        // when the turn stamped non-empty shell_allow and this command matches,
        // that list IS the allowlist for this call — skip global exec_policy
        // allowlist (which often lacks python3 on CS clones). Metachar +
        // shell_allow were already enforced in tool_runner; re-check allow here.
        let flow_shell_scoped = ctx
            .flow_shell_allow
            .map(|p| {
                !p.is_empty()
                    && carrier_types::flow::command_matches_flow_shell_allow(command, p, ctx.workspace_root)
            })
            .unwrap_or(false);

        // Exec policy enforcement (allowlist / deny / full) — unless flow-scoped.
        if !flow_shell_scoped {
            if let Some(policy) = exec_policy {
                if let Err(reason) =
                    crate::subprocess_sandbox::validate_command_allowlist(command, policy)
                {
                    return Some(Err(CarrierError::Sandbox(format!(
                        "shell_exec blocked: {reason}. Current exec_policy.mode = '{:?}'. \
                         To allow shell commands, set exec_policy.mode = 'full' in the agent manifest or config.toml.",
                        policy.mode
                    ))));
                }
            }
        }

        // Skip heuristic taint patterns for Full exec policy or flow-scoped allow.
        let is_full_exec =
            flow_shell_scoped || exec_policy.is_some_and(|p| p.mode == ExecSecurityMode::Full);
        if !is_full_exec {
            let suspicious_patterns = ["curl ", "wget ", "| sh", "| bash", "base64 -d", "eval "];
            for pattern in &suspicious_patterns {
                if command.contains(pattern) {
                    let mut labels = HashSet::new();
                    labels.insert(TaintLabel::ExternalNetwork);
                    let tainted = TaintedValue::new(command, labels, "llm_tool_call");
                    if let Err(violation) = tainted.check_sink(&TaintSink::shell_exec()) {
                        warn!(
                            command = crate::str_utils::safe_truncate_str(command, 80),
                            %violation,
                            "Shell taint check failed"
                        );
                        return Some(Err(CarrierError::Sandbox(format!(
                            "Taint violation: {violation}"
                        ))));
                    }
                }
            }
        }

        // Flow-scoped shell_exec (shell_allow matched) runs vetted flow scripts that
        // may make slow API calls (image/video gen, vision describe) — give them a
        // longer default timeout than the 30s exec_policy default. The agent can
        // still override per-call via the `timeout_seconds` input field.
        let default_timeout_secs = if flow_shell_scoped {
            FLOW_SHELL_DEFAULT_TIMEOUT_SECS
        } else {
            exec_policy.map(|p| p.timeout_secs).unwrap_or(30)
        };
        Some(
            exec_shell(
                input,
                &effective_command,
                allowed_env,
                workspace_root,
                exec_policy,
                default_timeout_secs,
            )
            .await,
        )
    }

    fn permission_level(&self, _tool_name: &str) -> carrier_types::tool::PermissionLevel {
        // shell_exec is the most dangerous tool — irreversible system access
        carrier_types::tool::PermissionLevel::Dangerous
    }
}

/// Default timeout for flow-scoped shell_exec (shell_allow matched). Flow
/// scripts are vetted and routinely make slow calls (brain API image/video
/// generation, vision describe) that exceed the 30s exec_policy default —
/// failing occupancy/seedream generation mid-run.
const FLOW_SHELL_DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Orthogonal exit-status header for a finished subprocess.
///
/// dsh defensive-pattern: `timed_out` / `signal` / `exit_code` are
/// independent facts and must be reported separately, never folded into one
/// another. In particular `status.code()` is `None` when the process was
/// killed by a signal — reporting that as `-1` (the old behavior) fabricated
/// an exit code and hid the signal from the model, which could read a
/// signal-killed run as a clean non-zero exit (or vice versa).
fn exit_status_header(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("Exit code: {code}"),
        None => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                match status.signal() {
                    Some(sig) => format!("Exit code: none (killed by signal {sig})"),
                    None => "Exit code: none (terminated without an exit status)".to_string(),
                }
            }
            #[cfg(not(unix))]
            {
                let _ = status;
                "Exit code: none (terminated without an exit status)".to_string()
            }
        }
    }
}

/// Orthogonal timeout error for a killed-on-deadline subprocess. A timeout is
/// NOT an exit code: the process was killed, so there is no exit status and
/// no captured output — say exactly that instead of implying a command
/// failure the model might retry blindly.
fn timeout_error(timeout_secs: u64) -> CarrierError {
    CarrierError::Internal(format!(
        "Timed out: true (after {timeout_secs}s) — process killed, no exit \
         status or output captured"
    ))
}

async fn exec_shell(
    input: &Value,
    effective_command: &str,
    allowed_env: &[String],
    workspace_root: Option<&Path>,
    exec_policy: Option<&carrier_types::config::ExecPolicy>,
    default_timeout_secs: u64,
) -> CarrierResult<String> {
    // `effective_command` is the command to actually execute. For a
    // `cd <DIR> && <REST>` input the caller already stripped the cd prefix and
    // set workspace_root (= cd_dir) as cwd, so effective_command is just <REST>.
    // For ordinary commands it equals input["command"]. timeout still comes
    // from the original input.
    let command = effective_command;
    let timeout_secs = input["timeout_seconds"]
        .as_u64()
        .unwrap_or(default_timeout_secs);

    let use_direct_exec = exec_policy
        .map(|p| p.mode == ExecSecurityMode::Allowlist)
        .unwrap_or(true);

    let mut cmd = if use_direct_exec {
        let argv = shlex::split(command).ok_or_else(|| {
            CarrierError::InvalidInput(
                "Command contains unmatched quotes or invalid shell syntax".to_string(),
            )
        })?;
        if argv.is_empty() {
            return Err(CarrierError::InvalidInput(
                "Empty command after parsing".to_string(),
            ));
        }
        let mut c = tokio::process::Command::new(&argv[0]);
        if argv.len() > 1 {
            c.args(&argv[1..]);
        }
        c
    } else {
        #[cfg(windows)]
        let git_sh: Option<&str> = {
            const SH_PATHS: &[&str] = &[
                "C:\\Program Files\\Git\\usr\\bin\\sh.exe",
                "C:\\Program Files (x86)\\Git\\usr\\bin\\sh.exe",
            ];
            SH_PATHS
                .iter()
                .copied()
                .find(|p| std::path::Path::new(p).exists())
        };
        let (shell, shell_arg) = if cfg!(windows) {
            #[cfg(windows)]
            {
                if let Some(sh) = git_sh {
                    (sh, "-c")
                } else {
                    ("cmd", "/C")
                }
            }
            #[cfg(not(windows))]
            {
                ("sh", "-c")
            }
        } else {
            ("sh", "-c")
        };
        let mut c = tokio::process::Command::new(shell);
        c.arg(shell_arg).arg(command);
        c
    };

    if let Some(ws) = workspace_root {
        cmd.current_dir(ws);
    }

    crate::subprocess_sandbox::sandbox_command(&mut cmd, allowed_env);

    #[cfg(windows)]
    cmd.env("PYTHONIOENCODING", "utf-8");

    cmd.stdin(std::process::Stdio::null());
    // Kill the child on drop (timeout / cancellation) — same orphan-prevention
    // rationale as cli_exec: a timed-out command must not keep running.
    cmd.kill_on_drop(true);

    let result =
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;

    match result {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let header = exit_status_header(&output.status);

            let max_output = 100_000;
            let stdout_str = if stdout.len() > max_output {
                format!(
                    "{}...\n[truncated, {} total bytes]",
                    crate::str_utils::safe_truncate_str(&stdout, max_output),
                    stdout.len()
                )
            } else {
                stdout.to_string()
            };
            let stderr_str = if stderr.len() > max_output {
                format!(
                    "{}...\n[truncated, {} total bytes]",
                    crate::str_utils::safe_truncate_str(&stderr, max_output),
                    stderr.len()
                )
            } else {
                stderr.to_string()
            };

            Ok(format!(
                "{header}\n\nSTDOUT:\n{stdout_str}\nSTDERR:\n{stderr_str}"
            ))
        }
        Ok(Err(e)) => Err(CarrierError::Internal(format!(
            "Failed to execute command: {e}"
        ))),
        Err(_) => Err(timeout_error(timeout_secs)),
    }
}

// ---------------------------------------------------------------------------
// cli_exec — whitelisted CLI command execution
// ---------------------------------------------------------------------------

/// Whitelisted CLI command execution tool.
///
/// Unlike `shell_exec` (Dangerous), `cli_exec` only allows commands
/// explicitly listed in the config. Arguments are parsed with `shlex`
/// and executed directly — no shell wrapper. Safe for low-privilege agents.
pub struct CliExecTools {
    config: carrier_types::config::CliExecConfig,
}

impl CliExecTools {
    pub fn new(config: carrier_types::config::CliExecConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ToolModule for CliExecTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        if self.config.commands.is_empty() {
            return vec![];
        }

        // Build a description that lists available commands and examples.
        let mut cmd_lines = Vec::new();
        for cmd in &self.config.commands {
            let examples = if cmd.examples.is_empty() {
                String::new()
            } else {
                format!(" (e.g. {})", cmd.examples.join(", "))
            };
            cmd_lines.push(format!("- {}: {}{}", cmd.name, cmd.description, examples));
        }
        let description = format!(
            "Execute a whitelisted CLI command. Available commands:\n{}",
            cmd_lines.join("\n")
        );

        vec![ToolDefinition {
            name: "cli_exec".to_string(),
            description,
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Command name (e.g. 'gh', 'todoist')" },
                    "args": { "type": "string", "description": "Arguments as a single string (e.g. 'pr list --repo owner/repo')" },
                    "timeout_seconds": { "type": "integer", "description": "Max seconds to run (default: exec_policy timeout, ~60s). Set higher (e.g. 180) for long calls such as brain API image/video generation." }
                },
                "required": ["command"]
            }),
        }]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        if name != "cli_exec" {
            return None;
        }

        let command_name = input["command"].as_str().unwrap_or("").trim();

        // 1. Check whitelist
        let allowed = self.config.commands.iter().find(|c| c.name == command_name);
        if allowed.is_none() {
            let available: Vec<&str> = self
                .config
                .commands
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            return Some(Err(CarrierError::Sandbox(format!(
                "Command '{command_name}' not in cli_exec allowlist. Available: {}",
                available.join(", ")
            ))));
        }

        // 2. Parse args with shlex — never start a shell
        let args_str = input["args"].as_str().unwrap_or("");
        let mut argv = vec![command_name.to_string()];
        if !args_str.is_empty() {
            // SECURITY: reject shell metacharacters in args
            if let Some(reason) = crate::subprocess_sandbox::contains_shell_metacharacters(args_str)
            {
                return Some(Err(CarrierError::Sandbox(format!(
                    "cli_exec blocked: args contain {reason}. \
                     Shell metacharacters (pipes, redirects, subshells) are not allowed."
                ))));
            }
            let parsed = shlex::split(args_str).ok_or_else(|| {
                CarrierError::InvalidInput(
                    "Arguments contain unmatched quotes or invalid syntax".to_string(),
                )
            });
            match parsed {
                Ok(parts) => argv.extend(parts),
                Err(e) => return Some(Err(e)),
            }
        }

        // 3. Execute directly — no shell wrapper
        let allowed_env = ctx.allowed_env_vars.unwrap_or(&[]);
        let workspace_root = resolve_shell_cwd(ctx);

        let mut cmd = tokio::process::Command::new(&argv[0]);
        if argv.len() > 1 {
            cmd.args(&argv[1..]);
        }

        if let Some(ws) = workspace_root.as_deref() {
            cmd.current_dir(ws);
        }

        crate::subprocess_sandbox::sandbox_command(&mut cmd, allowed_env);

        #[cfg(windows)]
        cmd.env("PYTHONIOENCODING", "utf-8");

        cmd.stdin(std::process::Stdio::null());
        // Kill the child if this future is dropped (e.g. on timeout). Without
        // this, a timed-out subprocess (brain API scripts, batch runners)
        // becomes an orphan that keeps running and consuming resources / API
        // quota after the tool call has already returned "timed out".
        cmd.kill_on_drop(true);

        let policy_timeout = ctx.exec_policy.map(|p| p.timeout_secs).unwrap_or(30);
        let timeout_secs = input["timeout_seconds"]
            .as_u64()
            .unwrap_or(policy_timeout)
            .min(300); // hard ceiling: no single CLI call runs longer than 5 min
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let header = exit_status_header(&output.status);

                let max_output = 100_000;
                let stdout_str = if stdout.len() > max_output {
                    format!(
                        "{}...\n[truncated, {} total bytes]",
                        crate::str_utils::safe_truncate_str(&stdout, max_output),
                        stdout.len()
                    )
                } else {
                    stdout.to_string()
                };
                let stderr_str = if stderr.len() > max_output {
                    format!(
                        "{}...\n[truncated, {} total bytes]",
                        crate::str_utils::safe_truncate_str(&stderr, max_output),
                        stderr.len()
                    )
                } else {
                    stderr.to_string()
                };

                Some(Ok(format!(
                    "{header}\n\nSTDOUT:\n{stdout_str}\nSTDERR:\n{stderr_str}"
                )))
            }
            Ok(Err(e)) => Some(Err(CarrierError::Sandbox(format!(
                "Failed to execute command: {e}"
            )))),
            Err(_) => Some(Err(timeout_error(timeout_secs))),
        }
    }

    fn permission_level(&self, _tool_name: &str) -> carrier_types::tool::PermissionLevel {
        // cli_exec is restricted to whitelisted commands only — safe for Write-level agents
        carrier_types::tool::PermissionLevel::Write
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exit_header_reports_plain_code() {
        // Raw wait status 0<<8 = exited with code 0.
        let status = std::os::unix::process::ExitStatusExt::from_raw(0 << 8);
        assert_eq!(exit_status_header(&status), "Exit code: 0");
    }

    #[cfg(unix)]
    #[test]
    fn exit_header_reports_signal_without_fabricating_code() {
        // Raw wait status 15 = killed by SIGTERM: code() is None, signal() is 15.
        // The old `unwrap_or(-1)` reported this as "Exit code: -1", hiding the
        // signal entirely.
        let status = std::os::unix::process::ExitStatusExt::from_raw(15);
        assert_eq!(
            exit_status_header(&status),
            "Exit code: none (killed by signal 15)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn exit_header_distinguishes_signal_kill_from_exit_15() {
        // Exit code 15 (raw 15<<8 | 0) must NOT be confused with SIGTERM.
        let exited = std::os::unix::process::ExitStatusExt::from_raw(15 << 8);
        assert_eq!(exit_status_header(&exited), "Exit code: 15");
    }

    #[test]
    fn timeout_error_names_its_facts() {
        let msg = timeout_error(30).to_string();
        assert!(
            msg.contains("Timed out: true"),
            "must state timed_out: {msg}"
        );
        assert!(
            msg.contains("no exit status"),
            "must state no exit code was captured: {msg}"
        );
    }
}
