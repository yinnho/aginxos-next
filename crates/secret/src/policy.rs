//! Policy: the secret-free allowlist file at /etc/aginx/secret.policy.
//!
//! ```json
//! {
//!   "env":   { "AGINXBRAIN_API_KEY": "brain.primary" },
//!   "allow": { "brain.primary": ["/usr/libexec/aginx/aginx-server"],
//!              "admin":         ["/usr/bin/aginx-secret"] }
//! }
//! ```
//!
//! `env` maps the env var names consumers historically looked up
//! (`api_key_env`, `secret_env`) to scopes, so the sidecar can serve the
//! same lookup without the value ever living in the environment. `allow`
//! gates every read/sign op on the peer's exe realpath. Missing file or
//! missing entry means deny — the policy is fail-closed by construction.
//! `admin` is the pseudo-scope that unlocks put/rm/list (human face only).
//!
//! The allow match strips the kernel's `" (deleted)"` suffix: aginx-pkg
//! replaces binaries with tmp+rename, so a still-running daemon keeps
//! serving under its deleted inode until its unit restarts.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Default, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub allow: BTreeMap<String, Vec<String>>,
}

impl Policy {
    /// Load from a JSON file. Missing file → empty (deny-by-default)
    /// policy; a malformed file is reported so the daemon can log it and
    /// keep the empty policy rather than guess.
    pub fn load(path: &Path) -> Result<Policy, String> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Policy::default()),
            Err(e) => return Err(format!("read {path:?}: {e}")),
        };
        serde_json::from_slice(&bytes).map_err(|e| format!("parse {path:?}: {e}"))
    }

    /// Scope behind an env var name, if policy maps it.
    pub fn scope_for_env(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }

    /// May the peer exe use this scope? No exe identity → no.
    pub fn allows(&self, scope: &str, exe: Option<&str>) -> bool {
        let Some(exe) = exe else { return false };
        let exe = exe.strip_suffix(" (deleted)").unwrap_or(exe);
        self.allow.get(scope).is_some_and(|v| v.iter().any(|p| p == exe))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(env: &[(&str, &str)], allow: &[(&str, &[&str])]) -> Policy {
        let mut p = Policy::default();
        for (k, v) in env {
            p.env.insert(k.to_string(), v.to_string());
        }
        for (k, v) in allow {
            p.allow.insert(k.to_string(), v.iter().map(|s| s.to_string()).collect());
        }
        p
    }

    #[test]
    fn missing_file_is_empty_deny_all() {
        let d = testkit::tmp("aginx-secret-policy-missing");
        let p = Policy::load(&d.join("nope")).unwrap();
        assert!(p.env.is_empty());
        assert!(!p.allows("brain.primary", Some("/usr/libexec/aginx/aginx-server")));
        assert!(!p.allows("admin", Some("/usr/bin/aginx-secret")));
    }

    #[test]
    fn malformed_file_is_an_error_not_a_guess() {
        let d = testkit::tmp("aginx-secret-policy-bad");
        let f = d.join("policy");
        std::fs::write(&f, b"{oops").unwrap();
        assert!(Policy::load(&f).is_err());
    }

    #[test]
    fn allow_is_exact_scope_and_exe() {
        let p = policy(
            &[("AGINXBRAIN_API_KEY", "brain.primary")],
            &[("brain.primary", &["/usr/libexec/aginx/aginx-server"])],
        );
        assert_eq!(p.scope_for_env("AGINXBRAIN_API_KEY"), Some("brain.primary"));
        assert_eq!(p.scope_for_env("NOPE_KEY"), None);
        assert!(p.allows("brain.primary", Some("/usr/libexec/aginx/aginx-server")));
        assert!(!p.allows("brain.primary", Some("/usr/bin/aginx-secret")));
        assert!(!p.allows("brain.primary", None), "no identity → deny");
        assert!(!p.allows("other.scope", Some("/usr/libexec/aginx/aginx-server")));
        // kernel " (deleted)" suffix (aginx-pkg tmp+rename of a running exe)
        // normalizes to the same binary
        assert!(p.allows("brain.primary", Some("/usr/libexec/aginx/aginx-server (deleted)")));
    }

    #[test]
    fn device_policy_shape_round_trips() {
        // the tracked rootfs recipe's exact shape
        let d = testkit::tmp("aginx-secret-policy-device");
        let f = d.join("policy");
        std::fs::write(
            &f,
            br#"{"env":{"AGINXBRAIN_API_KEY":"brain.primary"},
                "allow":{"brain.primary":["/usr/libexec/aginx/aginx-server"],
                         "admin":["/usr/bin/aginx-secret"]}}"#,
        )
        .unwrap();
        let p = Policy::load(&f).unwrap();
        assert_eq!(p.scope_for_env("AGINXBRAIN_API_KEY"), Some("brain.primary"));
        assert!(p.allows("admin", Some("/usr/bin/aginx-secret")));
    }
}
