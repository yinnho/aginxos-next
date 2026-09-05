//! Op dispatch: one request Value in, one agio envelope Value out.
//!
//! Pure with respect to the socket — everything testable without I/O.
//! Ops:
//!
//! - `get {scope}`            bearer-style secret to an allowed exe
//! - `env {name}`             same, but the scope comes from policy.env
//! - `sign {scope, string}`   HMAC-SHA256 over string; key never leaves
//! - `issue {kind, ttl}`      short-lived token mint — M37, spec'd only
//! - `put {scope, value}`     admin only (human face; value via stdin)
//! - `rm {scope}`             admin only
//! - `list {}`                admin only; scope names, never values
//!
//! Error codes are stable strings consumers may branch on: `denied`,
//! `not_found`, `bad_request`, `not_implemented`, `store_error`.

use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;

use crate::peer::Peer;
use crate::policy::Policy;
use crate::store::Store;
use crate::ADMIN_SCOPE;

type Envelope = Value;

fn denied(scope: &str, peer: &Peer) -> Envelope {
    let who = peer.exe.as_deref().unwrap_or("<unidentified>");
    agio::fail_hint(
        agio::ErrorType::Auth,
        "denied",
        &format!("peer {who} is not allowed to use scope {scope}"),
        "scopes are granted per-exe in /etc/aginx/secret.policy",
    )
}

fn not_found(scope: &str) -> Envelope {
    agio::fail(agio::ErrorType::NotFound, "not_found", &format!("no secret at scope {scope}"))
}

fn bad_request(msg: &str) -> Envelope {
    agio::fail(agio::ErrorType::Usage, "bad_request", msg)
}

fn require_scope(req: &Value) -> Result<&str, Envelope> {
    let s = req.get("scope").and_then(Value::as_str).unwrap_or_default();
    if s.is_empty() || s.contains(char::is_whitespace) {
        return Err(bad_request("op needs a non-empty scope"));
    }
    Ok(s)
}

fn hmac_hex(key: &[u8], string: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(string.as_bytes());
    let out = mac.finalize().into_bytes();
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Serve one parsed request. `store` is mutated by put/rm and saved on
/// success — the caller passes the daemon's live store.
pub fn handle(store: &mut Store, policy: &Policy, peer: &Peer, req: &Value) -> Envelope {
    let op = req.get("op").and_then(Value::as_str).unwrap_or_default();

    // ---- admin ops: the human face only -------------------------------
    if matches!(op, "put" | "rm" | "list") {
        if !policy.allows(ADMIN_SCOPE, peer.exe()) {
            return denied(ADMIN_SCOPE, peer);
        }
        return match op {
            "put" => {
                let scope = match require_scope(req) {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                let value = req.get("value").and_then(Value::as_str).unwrap_or_default();
                if value.is_empty() {
                    return bad_request("put needs a non-empty value (stdin, not argv)");
                }
                store.set(scope, value);
                match store.save() {
                    Ok(()) => agio::ok(json!({"scope": scope})),
                    Err(e) => agio::fail(agio::ErrorType::Io, "store_error", &e.to_string()),
                }
            }
            "rm" => {
                let scope = match require_scope(req) {
                    Ok(s) => s,
                    Err(e) => return e,
                };
                if !store.remove(scope) {
                    return not_found(scope);
                }
                match store.save() {
                    Ok(()) => agio::ok(json!({"scope": scope})),
                    Err(e) => agio::fail(agio::ErrorType::Io, "store_error", &e.to_string()),
                }
            }
            _ => {
                // list: scope NAMES only — values never ride this op
                agio::ok_meta(
                    json!(store.scopes()),
                    json!({"count": store.scopes().len()}),
                )
            }
        };
    }

    // ---- read/sign ops: per-scope allowlist ----------------------------
    match op {
        "get" => {
            let scope = match require_scope(req) {
                Ok(s) => s,
                Err(e) => return e,
            };
            if !policy.allows(scope, peer.exe()) {
                return denied(scope, peer);
            }
            match store.get(scope) {
                Some(v) => agio::ok(json!({"scope": scope, "value": v})),
                None => not_found(scope),
            }
        }
        "env" => {
            let name = req.get("name").and_then(Value::as_str).unwrap_or_default();
            if name.is_empty() {
                return bad_request("env needs a non-empty name");
            }
            let Some(scope) = policy.scope_for_env(name) else {
                return agio::fail(
                    agio::ErrorType::NotFound,
                    "not_found",
                    &format!("no policy mapping for env {name}"),
                );
            };
            if !policy.allows(scope, peer.exe()) {
                return denied(scope, peer);
            }
            match store.get(scope) {
                Some(v) => agio::ok(json!({"scope": scope, "value": v})),
                None => not_found(scope),
            }
        }
        "sign" => {
            let scope = match require_scope(req) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let string = req.get("string").and_then(Value::as_str).unwrap_or_default();
            if string.is_empty() {
                return bad_request("sign needs a non-empty string");
            }
            if !policy.allows(scope, peer.exe()) {
                return denied(scope, peer);
            }
            match store.get(scope) {
                Some(k) => agio::ok(json!({"scope": scope, "mac": hmac_hex(k.as_bytes(), string)})),
                None => not_found(scope),
            }
        }
        "issue" => agio::fail_hint(
            agio::ErrorType::State,
            "not_implemented",
            "token minting arrives with memory roaming (M37)",
            "get/sign carry v1; issue is spec'd, not built",
        ),
        _ => bad_request(&format!("unknown op {op:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::Policy;

    /// The device-shaped matrix: brain.primary belongs to the server,
    /// relay.primary to the gateway (N5⑤), admin to the human face.
    fn setup() -> (Store, Policy) {
        let d = testkit::tmp("aginx-secret-handle");
        let mut store = Store::load(&d.join("store")).unwrap();
        store.set("brain.primary", "sk-live-1");
        store.set("relay.primary", "relay-hmac-key");
        let mut p = Policy::default();
        p.env.insert("AGINXBRAIN_API_KEY".into(), "brain.primary".into());
        p.env.insert("AGINX_RELAY_SECRET".into(), "relay.primary".into());
        p.allow.insert(
            "brain.primary".into(),
            vec!["/usr/libexec/aginx/aginx-server".into()],
        );
        p.allow.insert(
            "relay.primary".into(),
            vec!["/usr/libexec/aginx/aginx-gateway".into()],
        );
        p.allow.insert("admin".into(), vec!["/usr/bin/aginx-secret".into()]);
        (store, p)
    }

    fn server() -> Peer {
        Peer { uid: 0, exe: Some("/usr/libexec/aginx/aginx-server".into()) }
    }
    fn gateway() -> Peer {
        Peer { uid: 0, exe: Some("/usr/libexec/aginx/aginx-gateway".into()) }
    }
    fn human() -> Peer {
        Peer { uid: 0, exe: Some("/usr/bin/aginx-secret".into()) }
    }
    fn stranger() -> Peer {
        Peer { uid: 0, exe: Some("/usr/bin/some-other-cli".into()) }
    }

    #[test]
    fn get_allows_only_scope_owner() {
        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &server(), &json!({"op":"get","scope":"brain.primary"}));
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["value"], json!("sk-live-1"));

        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &stranger(), &json!({"op":"get","scope":"brain.primary"}));
        assert_eq!(r["ok"], json!(false));
        assert_eq!(r["error"]["code"], json!("denied"));

        // allowed exe, wrong scope → still denied (server asking for the
        // gateway's relay secret is a confused-deputy, not a member)
        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &server(), &json!({"op":"get","scope":"relay.primary"}));
        assert_eq!(r["error"]["code"], json!("denied"));
    }

    #[test]
    fn unidentified_peer_denied_everything() {
        let (s, p) = setup();
        let anon = Peer { uid: 0, exe: None };
        let r = handle(&mut s.clone_store(), &p, &anon, &json!({"op":"get","scope":"brain.primary"}));
        assert_eq!(r["error"]["code"], json!("denied"));
    }

    #[test]
    fn env_maps_name_then_gates_scope() {
        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &server(), &json!({"op":"env","name":"AGINXBRAIN_API_KEY"}));
        assert_eq!(r["ok"], json!(true));
        assert_eq!(r["data"]["value"], json!("sk-live-1"));

        // the gateway leg (N5⑤): env name in, relay secret out
        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &gateway(), &json!({"op":"env","name":"AGINX_RELAY_SECRET"}));
        assert_eq!(r["data"]["value"], json!("relay-hmac-key"));

        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &stranger(), &json!({"op":"env","name":"AGINXBRAIN_API_KEY"}));
        assert_eq!(r["error"]["code"], json!("denied"));

        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &server(), &json!({"op":"env","name":"NOPE"}));
        assert_eq!(r["error"]["code"], json!("not_found"));
    }

    #[test]
    fn sign_returns_mac_not_key() {
        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &gateway(), &json!({"op":"sign","scope":"relay.primary","string":"hello"}));
        assert_eq!(r["ok"], json!(true));
        let mac = r["data"]["mac"].as_str().unwrap().to_string();
        assert_eq!(mac.len(), 64, "sha256 hex");
        assert_eq!(mac, hmac_hex(b"relay-hmac-key", "hello"));
        // the response never contains the key material
        assert!(!serde_json::to_string(&r).unwrap().contains("relay-hmac-key"));

        let (s, p) = setup();
        let r = handle(&mut s.clone_store(), &p, &stranger(), &json!({"op":"sign","scope":"relay.primary","string":"hello"}));
        assert_eq!(r["error"]["code"], json!("denied"));
    }

    #[test]
    fn put_rm_list_are_admin_only() {
        let (mut s, p) = setup();
        let r = handle(&mut s, &p, &server(), &json!({"op":"put","scope":"x.y","value":"v"}));
        assert_eq!(r["error"]["code"], json!("denied"), "server is not admin");

        let d = testkit::tmp("aginx-secret-handle-put");
        let mut s2 = Store::load(&d.join("store")).unwrap();
        let r = handle(&mut s2, &p, &human(), &json!({"op":"put","scope":"pay.ali","value":"pk-1"}));
        assert_eq!(r["ok"], json!(true));
        assert!(d.join("store").exists(), "put persisted");
        let r = handle(&mut s2, &p, &human(), &json!({"op":"list"}));
        let scopes = r["data"].as_array().unwrap();
        assert!(scopes.contains(&json!("pay.ali")));
        assert!(!serde_json::to_string(&r).unwrap().contains("pk-1"), "list leaks no values");
        let r = handle(&mut s2, &p, &human(), &json!({"op":"rm","scope":"pay.ali"}));
        assert_eq!(r["ok"], json!(true));
        let r = handle(&mut s2, &p, &human(), &json!({"op":"rm","scope":"pay.ali"}));
        assert_eq!(r["error"]["code"], json!("not_found"), "rm is explicit");
    }

    #[test]
    fn put_rejects_empty_and_bad_shapes() {
        let (mut s, p) = setup();
        let r = handle(&mut s, &p, &human(), &json!({"op":"put","scope":"a.b","value":""}));
        assert_eq!(r["error"]["code"], json!("bad_request"));
        let r = handle(&mut s, &p, &human(), &json!({"op":"put","scope":"","value":"v"}));
        assert_eq!(r["error"]["code"], json!("bad_request"));
        let r = handle(&mut s, &p, &human(), &json!({"op":"put","scope":"a b","value":"v"}));
        assert_eq!(r["error"]["code"], json!("bad_request"));
        let r = handle(&mut s, &p, &human(), &json!({"op":"nope"}));
        assert_eq!(r["error"]["code"], json!("bad_request"));
        let r = handle(&mut s, &p, &human(), &json!({"op":"issue","kind":"memory","ttl":300}));
        assert_eq!(r["error"]["code"], json!("not_implemented"));
    }

    // test helper: same store content, fresh handle (handle() only mutates
    // on put/rm which the tests that need it drive explicitly)
    trait CloneStore {
        fn clone_store(&self) -> Store;
    }
    impl CloneStore for Store {
        fn clone_store(&self) -> Store {
            let d = testkit::tmp("aginx-secret-handle-clone");
            let mut s = Store::load(&d.join("store")).unwrap();
            for sc in self.scopes() {
                s.set(sc, self.get(sc).unwrap());
            }
            s
        }
    }

    #[test]
    fn hmac_reference_vector() {
        // well-known HMAC-SHA256 vector (key=b"key")
        assert_eq!(
            hmac_hex(b"key", "The quick brown fox jumps over the lazy dog"),
            "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8"
        );
    }
}
