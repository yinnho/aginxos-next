//! Workspace file watcher — monitors `knowledge/` for changes and triggers compile.
//!
//! Uses the `notify` crate for cross-platform filesystem event watching.
//! Debounces rapid changes to avoid triggering compile on every write flush.
//!
//! Usage:
//! ```ignore
//! let llm: Arc<dyn Fn(&str, &str, u32) -> Result<String> + Send + Sync> = ...;
//! let handle = watcher::spawn_watcher(workspace, config, llm, None);
//! // ... later ...
//! handle.stop();
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, warn};
use carrier_types::error::{CarrierError, CarrierResult};

use crate::compile::{run_compile, CompileResult};
use crate::evolution_config::EvolutionConfig;

/// Debounce window — ignore events that arrive within this duration of the last trigger.
const DEBOUNCE_SECS: u64 = 5;

/// Type-erased LLM call closure stored in the watcher.
pub type LlmCallback = dyn Fn(&str, &str, u32) -> anyhow::Result<String> + Send + Sync;

/// Handle to stop the file watcher.
pub struct WatcherHandle {
    #[allow(dead_code)] // dropped to stop watching
    watcher: RecommendedWatcher,
    #[allow(dead_code)]
    knowledge_dir: PathBuf,
}

impl WatcherHandle {
    /// Stop the file watcher.
    pub fn stop(self) {
        drop(self);
    }
}

/// Callback invoked after a compile run triggered by the watcher.
pub type CompileCallback = dyn Fn(&CompileResult) + Send + Sync;

/// Spawn a file watcher that monitors `knowledge/` and triggers compile on changes.
///
/// Returns a `WatcherHandle` that stops the watcher when dropped.
pub fn spawn_watcher(
    workspace: PathBuf,
    config: EvolutionConfig,
    llm_call: Arc<LlmCallback>,
    on_compile: Option<Arc<CompileCallback>>,
) -> CarrierResult<WatcherHandle> {
    let knowledge_dir = workspace.join("knowledge");

    if !knowledge_dir.exists() {
        std::fs::create_dir_all(&knowledge_dir)
            .map_err(|e| CarrierError::Internal(format!("Failed to create knowledge dir: {e}")))?;
    }

    let last_trigger = Arc::new(std::sync::Mutex::new(
        Instant::now() - Duration::from_secs(DEBOUNCE_SECS + 1),
    ));

    let workspace_clone = workspace.clone();
    let config_clone = config.clone();
    let llm_call_clone = llm_call.clone();
    let on_compile_clone = on_compile.clone();

    let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        let event = match res {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "Watcher error");
                return;
            }
        };

        // Only react to file modifications, creations, and removals
        match event.kind {
            EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_) => {}
            _ => return,
        }

        // Only react to .md files in knowledge dir
        let relevant = event
            .paths
            .iter()
            .any(|p| p.extension().map(|e| e == "md").unwrap_or(false));
        if !relevant {
            return;
        }

        // Debounce: skip if triggered too recently
        {
            let mut last = last_trigger.lock().unwrap();
            let now = Instant::now();
            if now.duration_since(*last) < Duration::from_secs(DEBOUNCE_SECS) {
                debug!("Debouncing watcher event");
                return;
            }
            *last = now;
        }

        debug!("Knowledge change detected, triggering compile");

        let result = run_compile(&workspace_clone, &config_clone, &*llm_call_clone);

        info!(
            metadata = result.metadata_generated,
            merged = result.files_merged,
            compressed = result.files_compressed,
            errors = result.errors.len(),
            "Watcher-triggered compile complete"
        );

        if let Some(cb) = &on_compile_clone {
            cb(&result);
        }
    })
    .map_err(|e| CarrierError::Internal(format!("Failed to create watcher: {e}")))?;

    watcher
        .watch(&knowledge_dir, RecursiveMode::Recursive)
        .map_err(|e| CarrierError::Internal(format!("Failed to watch knowledge dir: {e}")))?;

    info!(
        dir = %knowledge_dir.display(),
        debounce_secs = DEBOUNCE_SECS,
        "Knowledge watcher started"
    );

    Ok(WatcherHandle {
        watcher,
        knowledge_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_watcher_debounce_timing() {
        let start = Instant::now() - Duration::from_secs(DEBOUNCE_SECS + 1);
        let last = Arc::new(std::sync::Mutex::new(start));

        // First trigger should succeed (elapsed > DEBOUNCE_SECS)
        {
            let mut guard = last.lock().unwrap();
            let now = Instant::now();
            assert!(now.duration_since(*guard) >= Duration::from_secs(DEBOUNCE_SECS));
            *guard = now;
        }

        // Immediate second trigger should be debounced
        {
            let guard = last.lock().unwrap();
            let now = Instant::now();
            assert!(now.duration_since(*guard) < Duration::from_secs(DEBOUNCE_SECS));
        }
    }
}
