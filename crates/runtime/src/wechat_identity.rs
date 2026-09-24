//! Process-level cache of WeChat OA user identity (from 86bus `bind-openid`).
//!
//! The weixin-oa webhook (`api::routes::weixin_oa`) calls the 86bus
//! `bind-openid` endpoint on inbound messages and stores the returned
//! `matched` role here, keyed by the user's service-account openid
//! (`openid_sa`). The prompt builder reads it back by `sender_id` to inject a
//! "current user identity" section into the system prompt.
//!
//! This is a side-channel between two crates (api writes, runtime/prompt_builder
//! reads) that avoids threading a new parameter through the entire
//! send_to_agent → prepare_agent_context → build_and_apply_prompt chain.
//!
//! Entries are ephemeral (process lifetime) and refreshed on a TTL by the
//! webhook. An absent entry means "unknown / not a weixin-oa user" — distinct
//! from an entry whose role is `""` (identified as a regular user).

use dashmap::DashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Default TTL for cached identity before the webhook refreshes it.
pub const DEFAULT_TTL_SECS: u64 = 30 * 60;

struct IdentityEntry {
    /// The raw `matched` value from bind-openid: "admin", "carrier_user", or "".
    role: String,
    fetched_at: Instant,
}

/// Global identity cache, keyed by sender_id (openid_sa).
static WECHAT_IDENTITY: LazyLock<DashMap<String, IdentityEntry>> = LazyLock::new(DashMap::new);

/// Cached unionid per openid_sa, so `cgi-bin/user/info` is queried at most once
/// per user. An empty string means "queried, no resolvable unionid" (so we don't
/// re-query on every message); absence means "not yet queried".
static WECHAT_UNIONID: LazyLock<DashMap<String, String>> = LazyLock::new(DashMap::new);

/// Store the identified role for a user (called by the webhook).
/// `role` is the raw `matched` string; an empty string is a meaningful value
/// ("regular user"), so store it as-is.
pub fn set(sender_id: &str, role: &str) {
    WECHAT_IDENTITY.insert(
        sender_id.to_string(),
        IdentityEntry {
            role: role.to_string(),
            fetched_at: Instant::now(),
        },
    );
}

/// Read the cached role for a user (called by the prompt builder).
/// Returns `None` when there is no entry (unknown / not a weixin-oa user),
/// and `Some("")` when the user was identified as a regular user.
pub fn get(sender_id: &str) -> Option<String> {
    WECHAT_IDENTITY.get(sender_id).map(|e| e.role.clone())
}

/// Whether the cached entry is stale or absent and should be refreshed.
/// Called by the webhook to decide whether to issue a bind-openid call.
pub fn needs_refresh(sender_id: &str, ttl_secs: u64) -> bool {
    match WECHAT_IDENTITY.get(sender_id) {
        Some(e) => e.fetched_at.elapsed() > Duration::from_secs(ttl_secs),
        None => true,
    }
}

/// Cache a user's unionid (queried once per openid). Pass `""` to record that
/// the user has no resolvable unionid, so we don't re-query on every message.
pub fn set_unionid(sender_id: &str, unionid: &str) {
    WECHAT_UNIONID.insert(sender_id.to_string(), unionid.to_string());
}

/// Returns `Some` if this openid's unionid has already been queried (the value
/// may be `""` meaning "no unionid"), or `None` if not yet queried.
pub fn get_unionid(sender_id: &str) -> Option<String> {
    WECHAT_UNIONID.get(sender_id).map(|e| e.value().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_refresh() {
        // Unique key per test run to avoid collisions with other tests.
        let key = format!("test-user-{}", std::process::id());
        assert!(get(&key).is_none());
        assert!(needs_refresh(&key, DEFAULT_TTL_SECS));

        set(&key, "admin");
        assert_eq!(get(&key).as_deref(), Some("admin"));
        assert!(!needs_refresh(&key, DEFAULT_TTL_SECS));

        // Empty role is a meaningful value (regular user), not "absent".
        set(&key, "");
        assert_eq!(get(&key).as_deref(), Some(""));
    }
}
