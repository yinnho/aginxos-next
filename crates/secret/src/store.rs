//! The credential store: a flat `scope → secret` JSON map on disk.
//!
//! Scope names are dotted paths (`brain.primary`, `relay.primary`) —
//! the policy allowlist is written against them. The file is 0600 root;
//! writes go through tmp+rename so a crash never leaves a half-written
//! store. The store rides the state tar across rootfs swaps
//! (/var/lib/aginx is a state-tar member) but is excluded from
//! aginx-backup (§9 铁律): the backup line must not bootstrap from its
//! own cargo. Lost keys are re-entered (`aginx-secret set`).

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::json;

pub struct Store {
    path: PathBuf,
    map: BTreeMap<String, String>,
}

impl Store {
    /// Load the store; a missing file is an empty store (first boot),
    /// anything else is an error the caller reports.
    pub fn load(path: &Path) -> io::Result<Store> {
        let map = match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, format!("bad store json: {e}"))
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        Ok(Store { path: path.to_path_buf(), map })
    }

    pub fn get(&self, scope: &str) -> Option<&String> {
        self.map.get(scope)
    }

    pub fn set(&mut self, scope: &str, value: &str) {
        self.map.insert(scope.to_string(), value.to_string());
    }

    /// Remove a scope; false when it wasn't there.
    pub fn remove(&mut self, scope: &str) -> bool {
        self.map.remove(scope).is_some()
    }

    pub fn scopes(&self) -> Vec<&str> {
        self.map.keys().map(String::as_str).collect()
    }

    /// Persist atomically (tmp + rename, 0600).
    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension("tmp");
        let body = serde_json::to_vec_pretty(&json!(&self.map))?;
        fs::write(&tmp, &body)?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_store() {
        let d = testkit::tmp("aginx-secret-store-missing");
        let s = Store::load(&d.join("nope.json")).unwrap();
        assert!(s.scopes().is_empty());
        assert_eq!(s.get("brain.primary"), None);
    }

    #[test]
    fn round_trip_and_mode() {
        use std::os::unix::fs::PermissionsExt;
        let d = testkit::tmp("aginx-secret-store-rt");
        let p = d.join("store");
        let mut s = Store::load(&p).unwrap();
        s.set("brain.primary", "sk-test-1");
        s.set("relay.primary", "relay-test-key");
        s.save().unwrap();

        let mode = fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "store must be 0600");
        assert!(!p.with_extension("tmp").exists(), "tmp renamed away");

        let mut back = Store::load(&p).unwrap();
        assert_eq!(back.get("brain.primary").unwrap(), "sk-test-1");
        assert_eq!(back.scopes(), vec!["brain.primary", "relay.primary"]);
        assert!(back.remove("relay.primary"));
        assert!(!back.remove("relay.primary"));
        back.save().unwrap();
        let again = Store::load(&p).unwrap();
        assert_eq!(again.scopes(), vec!["brain.primary"]);
    }

    #[test]
    fn corrupt_store_is_invalid_data() {
        let d = testkit::tmp("aginx-secret-store-bad");
        let p = d.join("store");
        fs::write(&p, b"{not json").unwrap();
        // no unwrap_err: Store intentionally has no Debug (it holds
        // secrets — a stray Debug print would leak values)
        match Store::load(&p) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            Ok(_) => panic!("corrupt store must not load"),
        }
    }
}
