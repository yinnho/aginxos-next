//! Process-wide environment variable override map.
//!
//! `std::env::set_var` is deprecated in Rust 2024 for multi-threaded contexts.
//! This module provides an in-process override map that [`get_env`] checks
//! *before* falling back to `std::env::var`. All crates should use [`get_env`]
//! when reading env vars that may be set at runtime (e.g., LLM provider API keys).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::Mutex;

/// In-process environment overrides, prioritized over the process environment.
static ENV_OVERRIDES: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Get an environment variable, checking the in-process override map first.
///
/// This is the safe replacement for `std::env::var` when the value may have
/// been set at runtime via [`set_env_override`]. It checks `ENV_OVERRIDES`
/// first, then falls back to the process environment.
pub fn get_env(key: &str) -> Option<String> {
    let overrides = ENV_OVERRIDES.lock().unwrap();
    if let Some(val) = overrides.get(key).cloned() {
        return Some(val);
    }
    drop(overrides);
    std::env::var(key).ok()
}

/// Set an environment variable override in the in-process map.
///
/// The override takes priority over the process environment for [`get_env`]
/// calls. This is the safe replacement for `std::env::set_var` in async
/// contexts.
pub fn set_env_override(key: &str, value: &str) {
    let mut overrides = ENV_OVERRIDES.lock().unwrap();
    overrides.insert(key.to_string(), value.to_string());
}

/// Credential resolution with the secret-sidecar leg (M36, DECISIONS §9 on
/// the OS side): env (overrides + process env, incl. `~/.aginx/carrier/.env`
/// loaded at boot) first, then the agsecretd sidecar's `env` op as the
/// gap-filler. Sidecar misses — absent daemon, denied peer, unmapped name —
/// read as `None`, never as a hard error, so a host without the sidecar
/// behaves exactly as before.
///
/// Use this wherever a config names an env var that holds a secret
/// (`brain.api_key_env`, `api.hmac.secret_env`, `auth_env`, …); plain
/// configuration lookups stay on [`get_env`].
pub fn get_secret(key: &str) -> Option<String> {
    if let Some(v) = get_env(key) {
        return Some(v);
    }
    crate::sidecar::env_lookup(key)
}
