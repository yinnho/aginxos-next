//! Per-clone admin management — reads/writes `admins.json` in the clone workspace.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

const ADMINS_FILE: &str = "admins.json";

/// Global lock for admins.json mutations. Admin operations are infrequent
/// (once per user binding/approval), so a single lock is fine — it prevents
/// TOCTOU races without per-workspace complexity.
static GLOBAL_ADMIN_LOCK: Mutex<()> = Mutex::new(());

fn get_lock() -> std::sync::MutexGuard<'static, ()> {
    GLOBAL_ADMIN_LOCK.lock().unwrap()
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct AdminsFile {
    #[serde(default)]
    pub admins: Vec<AdminEntry>,
    #[serde(default)]
    pub pending: Vec<PendingEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AdminEntry {
    pub sender_id: String,
    pub sender_name: String,
    /// "creator" (first binder, irrevocable) or "admin" (approved via dashboard)
    pub role: String,
    pub approved_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PendingEntry {
    pub sender_id: String,
    pub sender_name: String,
    pub requested_at: String,
}

/// Typed errors for admin-store mutations.
///
/// Replaces ad-hoc `Result<_, String>` so callers match on variants instead of
/// substring-matching error messages (which silently breaks when the message
/// wording changes). `Display` strings are kept stable for log compatibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminError {
    /// `add_pending`: sender is already an admin.
    AlreadyAdmin,
    /// `add_pending`: a pending request already exists for this sender.
    AlreadyPending,
    /// `approve`: sender not found in the pending list.
    NotFoundInPending,
    /// `revoke`: sender not found in the admins list.
    NotFoundInAdmins,
    /// `revoke`: target is the creator and cannot be revoked.
    IsCreator,
    /// Serialization or filesystem failure.
    Io(String),
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdminError::AlreadyAdmin => write!(f, "already_admin"),
            AdminError::AlreadyPending => write!(f, "already_pending"),
            AdminError::NotFoundInPending => write!(f, "not_found_in_pending"),
            AdminError::NotFoundInAdmins => write!(f, "not_found_in_admins"),
            AdminError::IsCreator => write!(f, "cannot_revoke_creator"),
            AdminError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for AdminError {}

pub fn read_admins(workspace: &Path) -> AdminsFile {
    let path = workspace.join(ADMINS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => AdminsFile::default(),
    }
}

/// Atomic write: write to temp file then rename (crash-safe on POSIX).
fn write_admins_atomic(workspace: &Path, admins: &AdminsFile) -> Result<(), AdminError> {
    let path = workspace.join(ADMINS_FILE);
    let content = serde_json::to_string_pretty(admins)
        .map_err(|e| AdminError::Io(format!("Serialize error: {e}")))?;
    let tmp_path = path.with_extension("tmp");
    std::fs::write(&tmp_path, &content).map_err(|e| AdminError::Io(format!("Write error: {e}")))?;
    std::fs::rename(&tmp_path, &path).map_err(|e| AdminError::Io(format!("Rename error: {e}")))
}

pub fn is_admin(workspace: &Path, sender_id: &str) -> bool {
    let _lock = get_lock();
    let admins = read_admins(workspace);
    admins.admins.iter().any(|a| a.sender_id == sender_id)
}

pub fn add_pending(workspace: &Path, sender_id: &str, sender_name: &str) -> Result<(), AdminError> {
    let _lock = get_lock();
    let mut admins = read_admins(workspace);

    if admins.admins.iter().any(|a| a.sender_id == sender_id) {
        return Err(AdminError::AlreadyAdmin);
    }
    if admins.pending.iter().any(|p| p.sender_id == sender_id) {
        return Err(AdminError::AlreadyPending);
    }

    admins.pending.push(PendingEntry {
        sender_id: sender_id.to_string(),
        sender_name: sender_name.to_string(),
        requested_at: chrono::Utc::now().to_rfc3339(),
    });

    write_admins_atomic(workspace, &admins)
}

pub fn approve(workspace: &Path, sender_id: &str) -> Result<(), AdminError> {
    let _lock = get_lock();
    let mut admins = read_admins(workspace);

    let idx = admins
        .pending
        .iter()
        .position(|p| p.sender_id == sender_id)
        .ok_or(AdminError::NotFoundInPending)?;

    let entry = admins.pending.remove(idx);
    admins.admins.push(AdminEntry {
        sender_id: entry.sender_id,
        sender_name: entry.sender_name,
        role: "admin".to_string(),
        approved_at: chrono::Utc::now().to_rfc3339(),
    });

    write_admins_atomic(workspace, &admins)
}

pub fn revoke(workspace: &Path, sender_id: &str) -> Result<(), AdminError> {
    let _lock = get_lock();
    let mut admins = read_admins(workspace);

    let entry = admins
        .admins
        .iter()
        .find(|a| a.sender_id == sender_id)
        .ok_or(AdminError::NotFoundInAdmins)?;

    if entry.role == "creator" {
        return Err(AdminError::IsCreator);
    }

    admins.admins.retain(|a| a.sender_id != sender_id);
    write_admins_atomic(workspace, &admins)
}

/// Auto-assign the first bound sender as creator. Only writes if admins is empty.
pub fn auto_assign_creator(
    workspace: &Path,
    sender_id: &str,
    sender_name: &str,
) -> Result<bool, AdminError> {
    let _lock = get_lock();
    let admins = read_admins(workspace);
    if !admins.admins.is_empty() {
        return Ok(false);
    }

    let mut admins = admins;
    admins.admins.push(AdminEntry {
        sender_id: sender_id.to_string(),
        sender_name: sender_name.to_string(),
        role: "creator".to_string(),
        approved_at: chrono::Utc::now().to_rfc3339(),
    });

    write_admins_atomic(workspace, &admins)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_read_missing_returns_default() {
        let dir = TempDir::new().unwrap();
        let admins = read_admins(dir.path());
        assert!(admins.admins.is_empty());
        assert!(admins.pending.is_empty());
    }

    #[test]
    fn test_auto_assign_creator() {
        let dir = TempDir::new().unwrap();
        let created = auto_assign_creator(dir.path(), "user1", "Alice").unwrap();
        assert!(created);

        let admins = read_admins(dir.path());
        assert_eq!(admins.admins.len(), 1);
        assert_eq!(admins.admins[0].role, "creator");
        assert_eq!(admins.admins[0].sender_id, "user1");
    }

    #[test]
    fn test_auto_assign_skips_if_admins_exist() {
        let dir = TempDir::new().unwrap();
        auto_assign_creator(dir.path(), "user1", "Alice").unwrap();
        let created = auto_assign_creator(dir.path(), "user2", "Bob").unwrap();
        assert!(!created);
    }

    #[test]
    fn test_add_pending_and_approve() {
        let dir = TempDir::new().unwrap();
        auto_assign_creator(dir.path(), "user1", "Alice").unwrap();

        add_pending(dir.path(), "user2", "Bob").unwrap();
        let admins = read_admins(dir.path());
        assert_eq!(admins.pending.len(), 1);

        approve(dir.path(), "user2").unwrap();
        let admins = read_admins(dir.path());
        assert!(admins.pending.is_empty());
        assert_eq!(admins.admins.len(), 2);
        assert_eq!(admins.admins[1].role, "admin");

        assert!(is_admin(dir.path(), "user2"));
    }

    #[test]
    fn test_add_pending_duplicate() {
        let dir = TempDir::new().unwrap();
        auto_assign_creator(dir.path(), "user1", "Alice").unwrap();

        add_pending(dir.path(), "user2", "Bob").unwrap();
        let result = add_pending(dir.path(), "user2", "Bob");
        assert!(matches!(result, Err(AdminError::AlreadyPending)));

        let result = add_pending(dir.path(), "user1", "Alice");
        assert!(matches!(result, Err(AdminError::AlreadyAdmin)));
    }

    #[test]
    fn test_revoke_admin() {
        let dir = TempDir::new().unwrap();
        auto_assign_creator(dir.path(), "user1", "Alice").unwrap();
        add_pending(dir.path(), "user2", "Bob").unwrap();
        approve(dir.path(), "user2").unwrap();

        revoke(dir.path(), "user2").unwrap();
        assert!(!is_admin(dir.path(), "user2"));
    }

    #[test]
    fn test_cannot_revoke_creator() {
        let dir = TempDir::new().unwrap();
        auto_assign_creator(dir.path(), "user1", "Alice").unwrap();

        let result = revoke(dir.path(), "user1");
        assert!(matches!(result, Err(AdminError::IsCreator)));
    }
}
