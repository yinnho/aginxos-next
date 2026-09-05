//! Shared test fixtures (M29). One scratch-dir helper, one
//! executable-writer, one process-global env lock — enough that every
//! crate's tests name these things the same way.
//!
//! Convention, in order of preference (see crates that already do each):
//!
//! 1. Inject paths through the code under test (`agpkg::Paths`) and pass
//!    per-child env with `Command::env` for CLI e2e (`ag` router tests).
//!    These parallelize freely — no lock needed.
//! 2. `env_lock()` only when the code reads a process-global env var by
//!    design and cannot be parameterized (`agdone::dir` is the standing
//!    example). `std::env::set_var` touches process-global state, and
//!    cargo runs tests in threads, so those tests must serialize.
//!
//! Never bake real device paths into tests — everything lands under a
//! `testkit::tmp` scratch dir.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

static SEQ: AtomicU64 = AtomicU64::new(0);
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A unique created scratch dir under the system temp dir:
/// `$TMPDIR/agtest-<pid>-<seq>-<tag>`. Unique across parallel tests and
/// repeated runs; the caller cleans up (or doesn't — it's the temp dir).
pub fn tmp(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!(
        "agtest-{}-{n}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Write a file and mark it 0o755 — sh stubs the router discovers as
/// commands. Parents are created, mirroring `tmp` above.
pub fn write_exec(path: &Path, contents: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(par) = path.parent() {
        std::fs::create_dir_all(par).unwrap();
    }
    std::fs::write(path, contents).unwrap();
    let mut pm = std::fs::metadata(path).unwrap().permissions();
    pm.set_mode(0o755);
    std::fs::set_permissions(path, pm).unwrap();
}

/// Serialize tests that set process-global env vars. Hold the guard for
/// the whole test body: `let _g = testkit::env_lock();`
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
