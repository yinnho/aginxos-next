//! ACP session store — file-based JSONL persistence for serve mode.
//!
//! Stores sessions in a Claude-compatible JSONL format so aginx can scan them directly.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Session metadata persisted alongside the JSONL file.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub session_id: String,
    pub agent_id: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

/// File-backed session store for ACP serve mode.
#[derive(Clone)]
pub struct AcpSessionStore {
    base_dir: PathBuf,
    file_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl AcpSessionStore {
    /// Create a new store rooted at `base_dir`.
    pub fn new(base_dir: &Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
            file_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a new session directory and empty meta file.
    pub fn create_session(
        &self,
        session_id: &str,
        agent_id: &str,
        cwd: &str,
    ) -> Result<(), String> {
        let dir = self.session_dir(session_id, cwd);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

        let meta = SessionMeta {
            session_id: session_id.to_string(),
            agent_id: agent_id.to_string(),
            cwd: cwd.to_string(),
            title: None,
            created_at: now_millis(),
            updated_at: now_millis(),
        };
        self.write_meta(&meta, cwd)?;
        Ok(())
    }

    /// Delete a session's JSONL and meta files.
    pub fn delete_session(&self, session_id: &str) -> Result<bool, String> {
        if let Some((jsonl, meta)) = self.find_session_paths(session_id) {
            let _ = std::fs::remove_file(&jsonl);
            let _ = std::fs::remove_file(&meta);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// List all sessions by scanning meta files.
    pub fn list_sessions(&self) -> Result<Vec<serde_json::Value>, String> {
        let mut sessions = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let workspace_dir = entry.path();
                if !workspace_dir.is_dir() {
                    continue;
                }
                if let Ok(files) = std::fs::read_dir(&workspace_dir) {
                    for file in files.flatten() {
                        let path = file.path();
                        if path.extension().and_then(|e| e.to_str()) != Some("json") {
                            continue;
                        }
                        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                        if !stem.ends_with(".meta") {
                            continue;
                        }
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            if let Ok(meta) = serde_json::from_str::<SessionMeta>(&content) {
                                sessions.push(serde_json::json!({
                                    "sessionId": meta.session_id,
                                    "agentId": meta.agent_id,
                                    "cwd": meta.cwd,
                                    "title": meta.title,
                                    "createdAt": meta.created_at,
                                    "updatedAt": meta.updated_at,
                                }));
                            }
                        }
                    }
                }
            }
        }
        sessions.sort_by(|a, b| {
            let a_updated = a.get("updatedAt").and_then(|v| v.as_u64()).unwrap_or(0);
            let b_updated = b.get("updatedAt").and_then(|v| v.as_u64()).unwrap_or(0);
            b_updated.cmp(&a_updated)
        });
        Ok(sessions)
    }

    /// Append a user message to the session JSONL.
    pub fn append_user_message(
        &self,
        session_id: &str,
        content: &str,
        cwd: Option<&str>,
    ) -> Result<(), String> {
        let (jsonl_path, meta_path, cwd_val) = if let Some(c) = cwd {
            let jsonl = self.jsonl_path(session_id, c);
            let meta = self.meta_path(session_id, c);
            (jsonl, meta, c.to_string())
        } else {
            let (jsonl, meta) = self
                .find_session_paths(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            let meta_content = std::fs::read_to_string(&meta).map_err(|e| e.to_string())?;
            let m: SessionMeta = serde_json::from_str(&meta_content).map_err(|e| e.to_string())?;
            (jsonl, meta, m.cwd)
        };

        let lock = self.get_lock(session_id);
        let _lock = lock.lock().unwrap_or_else(|e| e.into_inner());

        let line = serde_json::json!({
            "type": "user",
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "cwd": cwd_val,
            "message": {
                "content": content
            }
        });

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_path)
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, &line).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;

        // Update meta title if empty
        if meta_path.exists() {
            if let Ok(meta_content) = std::fs::read_to_string(&meta_path) {
                if let Ok(mut meta) = serde_json::from_str::<SessionMeta>(&meta_content) {
                    if meta.title.is_none() {
                        let truncated = if content.chars().count() > 100 {
                            content.chars().take(100).collect::<String>() + "..."
                        } else {
                            content.to_string()
                        };
                        meta.title = Some(truncated);
                    }
                    meta.updated_at = now_millis();
                    let _ = self.write_meta(&meta, &cwd_val);
                }
            }
        }

        Ok(())
    }

    /// Append an assistant message to the session JSONL.
    pub fn append_assistant_message(&self, session_id: &str, content: &str) -> Result<(), String> {
        let (jsonl_path, meta_path, cwd_val) = {
            let (jsonl, meta) = self
                .find_session_paths(session_id)
                .ok_or_else(|| format!("Session not found: {}", session_id))?;
            let meta_content = std::fs::read_to_string(&meta).map_err(|e| e.to_string())?;
            let m: SessionMeta = serde_json::from_str(&meta_content).map_err(|e| e.to_string())?;
            (jsonl, meta, m.cwd)
        };

        let lock = self.get_lock(session_id);
        let _lock = lock.lock().unwrap_or_else(|e| e.into_inner());

        let line = serde_json::json!({
            "type": "assistant",
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "message": {
                "content": [{"type": "text", "text": content}]
            }
        });

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_path)
            .map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut file, &line).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;

        // Update meta updated_at
        if meta_path.exists() {
            if let Ok(meta_content) = std::fs::read_to_string(&meta_path) {
                if let Ok(mut meta) = serde_json::from_str::<SessionMeta>(&meta_content) {
                    meta.updated_at = now_millis();
                    let _ = self.write_meta(&meta, &cwd_val);
                }
            }
        }

        Ok(())
    }

    /// Get session metadata by ID.
    pub fn get_session_meta(&self, session_id: &str) -> Option<SessionMeta> {
        let (_, meta_path) = self.find_session_paths(session_id)?;
        let content = std::fs::read_to_string(&meta_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Read messages from JSONL, returning the last `limit` entries in aginx format.
    pub fn get_messages(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, String> {
        let (jsonl_path, _) = self
            .find_session_paths(session_id)
            .ok_or_else(|| format!("Session not found: {}", session_id))?;

        let file = std::fs::File::open(&jsonl_path).map_err(|e| e.to_string())?;
        let reader = std::io::BufReader::new(file);
        let mut messages = Vec::new();

        for line in std::io::BufRead::lines(reader) {
            let line = line.map_err(|e| e.to_string())?;
            if line.trim().is_empty() {
                continue;
            }
            let event: serde_json::Value =
                serde_json::from_str(&line).map_err(|e| e.to_string())?;

            let event_type = event.get("type").and_then(|v| v.as_str());
            match event_type {
                Some("user") => {
                    if let Some(content) = event
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_str())
                    {
                        messages.push(serde_json::json!({
                            "role": "user",
                            "content": content,
                        }));
                    }
                }
                Some("assistant") => {
                    if let Some(msg_content) = event.get("message").and_then(|m| m.get("content")) {
                        let text = if let Some(t) = msg_content.get("text").and_then(|t| t.as_str())
                        {
                            t.to_string()
                        } else if let Some(arr) = msg_content.as_array() {
                            arr.iter()
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("")
                        } else {
                            String::new()
                        };
                        if !text.is_empty() {
                            messages.push(serde_json::json!({
                                "role": "assistant",
                                "content": text,
                            }));
                        }
                    }
                }
                _ => {}
            }
        }

        let start = if messages.len() > limit {
            messages.len() - limit
        } else {
            0
        };
        Ok(messages[start..].to_vec())
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn workspace_basename(cwd: &str) -> String {
        Path::new(cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("default")
            .to_string()
    }

    fn session_dir(&self, _session_id: &str, cwd: &str) -> PathBuf {
        self.base_dir.join(Self::workspace_basename(cwd))
    }

    fn jsonl_path(&self, session_id: &str, cwd: &str) -> PathBuf {
        self.session_dir(session_id, cwd)
            .join(format!("{}.jsonl", session_id))
    }

    fn meta_path(&self, session_id: &str, cwd: &str) -> PathBuf {
        self.session_dir(session_id, cwd)
            .join(format!("{}.meta.json", session_id))
    }

    fn write_meta(&self, meta: &SessionMeta, cwd: &str) -> Result<(), String> {
        let path = self.meta_path(&meta.session_id, cwd);
        let tmp = path.with_extension("tmp");
        let json = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn get_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.file_locks.lock().unwrap_or_else(|e| e.into_inner());
        locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    fn find_session_paths(&self, session_id: &str) -> Option<(PathBuf, PathBuf)> {
        if let Ok(entries) = std::fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                let workspace_dir = entry.path();
                if !workspace_dir.is_dir() {
                    continue;
                }
                let jsonl = workspace_dir.join(format!("{}.jsonl", session_id));
                let meta = workspace_dir.join(format!("{}.meta.json", session_id));
                if meta.exists() {
                    return Some((jsonl, meta));
                }
            }
        }
        None
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_list_sessions() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AcpSessionStore::new(dir.path());

        store
            .create_session("sess_001", "agent_a", "/project/foo")
            .unwrap();
        store
            .create_session("sess_002", "agent_a", "/project/bar")
            .unwrap();

        let sessions = store.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);

        let ids: Vec<String> = sessions
            .iter()
            .map(|s| {
                s.get("sessionId")
                    .and_then(|v| v.as_str())
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert!(ids.contains(&"sess_001".to_string()));
        assert!(ids.contains(&"sess_002".to_string()));
    }

    #[test]
    fn test_append_and_get_messages() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AcpSessionStore::new(dir.path());

        store
            .create_session("sess_003", "agent_b", "/workspace")
            .unwrap();
        store
            .append_user_message("sess_003", "Hello", Some("/workspace"))
            .unwrap();
        store
            .append_assistant_message("sess_003", "Hi there!")
            .unwrap();

        let messages = store.get_messages("sess_003", 10).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "Hi there!");
    }

    #[test]
    fn test_delete_session() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AcpSessionStore::new(dir.path());

        store.create_session("sess_004", "agent_c", "/tmp").unwrap();
        assert!(store.delete_session("sess_004").unwrap());
        assert!(!store.delete_session("sess_004").unwrap());
    }

    #[test]
    fn test_meta_title_set_from_first_user_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = AcpSessionStore::new(dir.path());

        store.create_session("sess_005", "agent_d", "/tmp").unwrap();
        let long_msg = "a".repeat(200);
        store
            .append_user_message("sess_005", &long_msg, Some("/tmp"))
            .unwrap();

        let sessions = store.list_sessions().unwrap();
        let title = sessions[0]
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap()
            .to_string();
        assert!(title.starts_with("aaaaaaaaaa"));
        assert!(title.ends_with("..."));
    }
}
