//! The accept loop — shared by the daemon bin and the integration tests.
//!
//! One request line per connection (ndjson), one envelope line back. The
//! loop is single-threaded on purpose: requests are tiny and rare, and a
//! serial daemon is the simplest thing that cannot interleave store
//! writes. `serve_with` takes the peer-identity function so tests can
//! speak the real protocol against chosen identities without /proc.
//!
//! Policy is re-read per request (a few hundred bytes — cheap, and policy
//! edits apply without a restart). The store is loaded once and kept in
//! memory; put/rm persist through it.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use serde_json::Value;

use crate::handle::handle;
use crate::peer::{peer_of, Peer};
use crate::policy::Policy;
use crate::store::Store;

/// Hard cap on one request line. Requests carry values on put — 64 KiB is
/// generous for any credential and stops a runaway client from feeding
/// the daemon an unbounded line.
const MAX_LINE: usize = 64 * 1024;

pub fn serve(
    sock: &Path,
    store_path: &Path,
    policy_path: &Path,
    log_path: &Path,
) -> std::io::Result<()> {
    serve_with(sock, store_path, policy_path, log_path, peer_of)
}

pub fn serve_with<F>(
    sock: &Path,
    store_path: &Path,
    policy_path: &Path,
    log_path: &Path,
    peer_of: F,
) -> std::io::Result<()>
where
    F: Fn(&UnixStream) -> Peer,
{
    if let Some(dir) = sock.parent() {
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let _ = std::fs::remove_file(sock); // stale socket from a dead boot
    let listener = UnixListener::bind(sock)?;
    std::fs::set_permissions(sock, std::fs::Permissions::from_mode(0o600))?;

    let store = Store::load(store_path).map_err(|e| {
        eprintln!("aginx-secretd: store {store_path:?}: {e}");
        e
    })?;
    let mut store = store;
    log_line(log_path, &format!("started sock={sock:?} store={store_path:?}"));

    for conn in listener.incoming() {
        let Ok(stream) = conn else { continue };
        let peer = peer_of(&stream);
        let (resp, op, scope) = serve_conn(&mut store, policy_path, &peer, &stream);
        // op + scope + peer only — values never touch the log
        log_line(
            log_path,
            &format!("op={op} scope={scope} peer={} ok={}", peer.exe.unwrap_or("<none>".into()), resp["ok"]),
        );
        let mut w = match stream.try_clone() {
            Ok(w) => w,
            Err(_) => continue,
        };
        let _ = writeln!(w, "{resp}");
        let _ = w.flush();
    }
    Ok(())
}

/// Read one capped line, dispatch, return (envelope, op, scope) — the
/// trailing pair exists only for logging.
fn serve_conn(
    store: &mut Store,
    policy_path: &Path,
    peer: &Peer,
    stream: &UnixStream,
) -> (Value, String, String) {
    let reader = BufReader::new(stream);
    let mut line = String::new();
    // read_line grows unbounded — cap it manually via take
    let mut capped = reader.take(MAX_LINE as u64 + 1);
    let n = capped.read_line(&mut line).unwrap_or(0);
    if n == 0 || n > MAX_LINE {
        return (
            agio::fail(agio::ErrorType::Usage, "bad_request", "empty or oversized request"),
            "<none>".into(),
            "<none>".into(),
        );
    }
    let req: Value = match serde_json::from_str(line.trim_end()) {
        Ok(v) => v,
        Err(_) => {
            return (
                agio::fail(agio::ErrorType::Usage, "bad_request", "request is not json"),
                "<bad>".into(),
                "<none>".into(),
            )
        }
    };
    // fail-closed: unreadable policy still serves nothing (empty policy)
    let policy = Policy::load(policy_path).unwrap_or_default();
    let op = req.get("op").and_then(Value::as_str).unwrap_or("<none>").to_string();
    let scope = req
        .get("scope")
        .or_else(|| req.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string();
    let resp = handle(store, &policy, peer, &req);
    (resp, op, scope)
}

fn log_line(path: &Path, what: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{ts} {what}");
    }
}
