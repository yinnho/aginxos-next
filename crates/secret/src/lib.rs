//! aginx-secret — the secret sidecar (M36 agsecret; N5② 吸收改姓，store
//! 迁 /var/lib/aginx/secret)。
//!
//! One daemon (`aginx-secretd`) holds every boundary credential;
//! consumers (the gateway daemon, `aginx` faces, future relay/memory
//! services) ask over a unix socket instead of inheriting env files
//! into every spawned CLI. Wire shape: one ndjson request line in, one
//! agio envelope line out.
//!
//! Authorization is scope-level: `/etc/aginx/secret.policy` (tracked,
//! secret-free) maps env names to scopes and scopes to allowed peer exe
//! paths. On Linux the peer exe comes from SO_PEERCRED + `/proc/<pid>/exe`;
//! the daemon never sees a value it wasn't asked for, and its log records
//! only op + scope + peer — never values.
//!
//! Not a defense against hostile local root (single-user phone — that
//! battle is unwinnable by construction); a defense against ambient
//! leakage: env inheritance, argv, transcripts, backup tars.
//!
//! Layout:
//! - [`store`] — the 0600 JSON store at /var/lib/aginx/secret/store
//! - [`policy`] — env→scope and scope→exe allowlists
//! - [`peer`] — who is asking (SO_PEERCRED on device, injectable in tests)
//! - [`handle`] — op dispatch, pure and testable
//! - [`serve`] — the accept loop (shared by the daemon bin and tests)
//! - [`client`] — the ~40-line consumer client

pub mod client;
pub mod handle;
pub mod peer;
pub mod policy;
pub mod serve;
pub mod store;

/// Default socket path (tmpfs — rebuilt every boot).
pub const DEFAULT_SOCKET: &str = "/run/aginx/secret.sock";
/// Default store path. Rides the state tar across rootfs swaps, but is
/// EXCLUDED from aginx-backup: the backup line must not bootstrap from
/// its own cargo. Lost keys are re-entered (`aginx-secret set`).
pub const DEFAULT_STORE: &str = "/var/lib/aginx/secret/store";
/// Default policy path (tracked in the rootfs recipe, no secrets).
pub const DEFAULT_POLICY: &str = "/etc/aginx/secret.policy";
/// Default daemon log (op + scope + peer exe, never values).
pub const DEFAULT_LOG: &str = "/var/log/aginx-secretd.log";

/// The policy scope that unlocks put/rm/list — the human face only.
pub const ADMIN_SCOPE: &str = "admin";

#[cfg(test)]
mod tests {
    #[test]
    fn defaults_are_stable_paths() {
        // consumers embed these; they are contract, not preference
        assert_eq!(super::DEFAULT_SOCKET, "/run/aginx/secret.sock");
        assert_eq!(super::DEFAULT_STORE, "/var/lib/aginx/secret/store");
        assert_eq!(super::DEFAULT_POLICY, "/etc/aginx/secret.policy");
        assert_eq!(super::DEFAULT_LOG, "/var/log/aginx-secretd.log");
        assert_eq!(super::ADMIN_SCOPE, "admin");
    }
}
