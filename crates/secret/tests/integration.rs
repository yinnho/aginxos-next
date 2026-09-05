//! End-to-end over a real unix socket: a daemon thread serving a temp
//! store+policy, the real client speaking the real protocol, with peer
//! identity injected per connection (the macOS host has no /proc — the
//! authz matrix still runs, and the Linux cred path gets its receipt in
//! the n5 device suite).

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;

use aginx_secret::client::request;
use aginx_secret::peer::Peer;
use aginx_secret::serve::serve_with;

struct Rig {
    sock: PathBuf,
    store: PathBuf,
    policy: PathBuf,
    log: PathBuf,
    /// which peer identity the daemon reports for the NEXT connection
    who: Arc<AtomicUsize>,
}

const SERVER: usize = 0;
const GATEWAY: usize = 1;
const HUMAN: usize = 2;
const STRANGER: usize = 3;

impl Rig {
    fn new(tag: &str) -> Rig {
        let d = testkit::tmp(tag);
        let rig = Rig {
            sock: d.join("secret.sock"),
            store: d.join("store"),
            policy: d.join("policy"),
            log: d.join("aginx-secretd.log"),
            who: Arc::new(AtomicUsize::new(HUMAN)),
        };
        // the device-shaped policy: brain.primary→server, relay.primary→gateway
        std::fs::write(
            &rig.policy,
            br#"{"env":{"AGINXBRAIN_API_KEY":"brain.primary",
                     "AGINX_RELAY_SECRET":"relay.primary"},
                "allow":{"brain.primary":["/usr/libexec/aginx/aginx-server"],
                         "relay.primary":["/usr/libexec/aginx/aginx-gateway"],
                         "admin":["/usr/bin/aginx-secret"]}}"#,
        )
        .unwrap();
        let sock = rig.sock.clone();
        let store = rig.store.clone();
        let policy = rig.policy.clone();
        let log = rig.log.clone();
        let who = rig.who.clone();
        std::thread::spawn(move || {
            let _ = serve_with(&sock, &store, &policy, &log, move |_: &UnixStream| {
                match who.load(Ordering::SeqCst) {
                    SERVER => Peer { uid: 0, exe: Some("/usr/libexec/aginx/aginx-server".into()) },
                    GATEWAY => Peer { uid: 0, exe: Some("/usr/libexec/aginx/aginx-gateway".into()) },
                    STRANGER => Peer { uid: 0, exe: Some("/usr/bin/some-other-cli".into()) },
                    _ => Peer { uid: 0, exe: Some("/usr/bin/aginx-secret".into()) },
                }
            });
        });
        // wait for the socket to appear (thread start)
        for _ in 0..200 {
            if rig.sock.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(rig.sock.exists(), "daemon never bound {tag}");
        rig
    }

    fn as_(&self, who: usize, req: &serde_json::Value) -> serde_json::Value {
        self.who.store(who, Ordering::SeqCst);
        request(&self.sock, req).expect("daemon round trip")
    }

    fn server(&self, req: &serde_json::Value) -> serde_json::Value {
        self.as_(SERVER, req)
    }
    fn gateway(&self, req: &serde_json::Value) -> serde_json::Value {
        self.as_(GATEWAY, req)
    }
    fn human(&self, req: &serde_json::Value) -> serde_json::Value {
        self.as_(HUMAN, req)
    }
    fn stranger(&self, req: &serde_json::Value) -> serde_json::Value {
        self.as_(STRANGER, req)
    }
}

#[test]
fn full_loop_set_get_env_sign_list_rm() {
    let r = Rig::new("aginx-secret-e2e");

    // human seeds the brain key (stdin path is the CLI's business; here
    // the wire op)
    let resp = r.human(&json!({"op":"put","scope":"brain.primary","value":"sk-e2e-1"}));
    assert_eq!(resp["ok"], json!(true));
    assert!(r.store.exists(), "put persisted the store");

    // server reads it back by scope and by env name
    assert_eq!(r.server(&json!({"op":"get","scope":"brain.primary"}))["data"]["value"], json!("sk-e2e-1"));
    assert_eq!(r.server(&json!({"op":"env","name":"AGINXBRAIN_API_KEY"}))["data"]["value"], json!("sk-e2e-1"));

    // gateway leg (N5⑤): relay secret by env name, sign without export
    r.human(&json!({"op":"put","scope":"relay.primary","value":"e2e-relay"}));
    assert_eq!(r.gateway(&json!({"op":"env","name":"AGINX_RELAY_SECRET"}))["data"]["value"], json!("e2e-relay"));
    let signed = r.gateway(&json!({"op":"sign","scope":"relay.primary","string":"payload"}));
    assert_eq!(signed["ok"], json!(true));
    let mac_hex = signed["data"]["mac"].as_str().unwrap().to_string();
    assert_eq!(mac_hex.len(), 64);
    assert!(!mac_hex.contains("e2e-relay"));

    // list: names only
    let listed = r.human(&json!({"op":"list"}));
    assert!(listed["data"].as_array().unwrap().contains(&json!("brain.primary")));
    assert!(!serde_json::to_string(&listed).unwrap().contains("sk-e2e-1"));

    // rm + confirm gone
    assert_eq!(r.human(&json!({"op":"rm","scope":"relay.primary"}))["ok"], json!(true));
    assert_eq!(r.gateway(&json!({"op":"sign","scope":"relay.primary","string":"payload"}))["error"]["code"], json!("not_found"));
}

#[test]
fn authz_matrix_on_the_wire() {
    let r = Rig::new("aginx-secret-e2e-authz");
    r.human(&json!({"op":"put","scope":"brain.primary","value":"sk-x"}));

    // stranger denied on get/env
    for req in [
        json!({"op":"get","scope":"brain.primary"}),
        json!({"op":"env","name":"AGINXBRAIN_API_KEY"}),
    ] {
        assert_eq!(r.stranger(&req)["error"]["code"], json!("denied"), "{req}");
    }

    // server cannot admin
    assert_eq!(r.server(&json!({"op":"put","scope":"z.z","value":"v"}))["error"]["code"], json!("denied"));
    assert_eq!(r.server(&json!({"op":"list"}))["error"]["code"], json!("denied"));

    // cross-scope: server asking for the gateway's relay secret → denied
    assert_eq!(r.server(&json!({"op":"get","scope":"relay.primary"}))["error"]["code"], json!("denied"));

    // human cannot read a scope it isn't allowlisted for (admin ≠ owner)
    assert_eq!(r.human(&json!({"op":"get","scope":"brain.primary"}))["error"]["code"], json!("denied"));

    // protocol errors
    assert_eq!(r.human(&json!({"op":"nope"}))["error"]["code"], json!("bad_request"));
    assert_eq!(r.human(&json!({"op":"issue","kind":"memory"}))["error"]["code"], json!("not_implemented"));
}

#[test]
fn socket_and_store_permissions_and_log() {
    let r = Rig::new("aginx-secret-e2e-perms");
    use std::os::unix::fs::PermissionsExt;
    let sm = std::fs::metadata(&r.sock).unwrap().permissions().mode();
    assert_eq!(sm & 0o777, 0o600, "socket 0600");
    r.human(&json!({"op":"put","scope":"pay.ali","value":"pk-secret"}));
    let pm = std::fs::metadata(&r.store).unwrap().permissions().mode();
    assert_eq!(pm & 0o777, 0o600, "store 0600");
    let log = std::fs::read_to_string(&r.log).unwrap();
    assert!(!log.contains("pk-secret"), "log never carries values");
    assert!(log.contains("op=put"), "log records the op");
}

#[test]
fn policy_reload_applies_without_restart() {
    let r = Rig::new("aginx-secret-e2e-reload");
    r.human(&json!({"op":"put","scope":"relay.backup","value":"rs-1"}));
    // nobody is allowed yet → gateway denied
    assert_eq!(r.gateway(&json!({"op":"get","scope":"relay.backup"}))["error"]["code"], json!("denied"));
    // grant the gateway, without touching the daemon
    std::fs::write(
        &r.policy,
        br#"{"env":{},"allow":{"relay.backup":["/usr/libexec/aginx/aginx-gateway"],"admin":["/usr/bin/aginx-secret"]}}"#,
    )
    .unwrap();
    assert_eq!(r.gateway(&json!({"op":"get","scope":"relay.backup"}))["data"]["value"], json!("rs-1"));
}

#[test]
fn missing_policy_file_is_deny_all() {
    let r = Rig::new("aginx-secret-e2e-nopolicy");
    // seed while the policy still grants admin…
    r.human(&json!({"op":"put","scope":"brain.primary","value":"sk-y"}));
    // …then lose the policy: every read turns deny (fail-closed)
    std::fs::remove_file(&r.policy).unwrap();
    assert_eq!(r.server(&json!({"op":"get","scope":"brain.primary"}))["error"]["code"], json!("denied"));
}
