//! aginx-done — provision 的 done 标记纪律（M27 agdone；N5② 吸收改姓，
//! 态迁 /var/lib/aginx/done）。
//!
//! 一次性引导步骤（M28 的 python/pip finalize 是第一个真实租户）需要
//! "成功才留痕"的标记：步骤失败不写标记，下次开机自然重试；标记坏了
//! （非常规文件、不可读）当不存在。态落 /var/lib/aginx/done/<name>，随
//! aginx-update 的 state tar 存活换机（/var/lib 在包内，M26 起）。
//!
//! 约定：
//! - 标记的存在即真值；文件内容（mark 时刻的 epoch 秒）仅供人读。
//! - 名字限 [A-Za-z0-9._-]+ —— 标记名是文件名，遍历类名字直接 usage 拒。
//! - check 的退出码三值：0=已标记，3=未标记（查询的合法答案，不是
//!   失败），1=io 故障。provision 侧的惯用法：
//!     aginx-done check python-finalize || { pip install -r … && aginx-done mark python-finalize; }
//!   （ensure 不是这个用途：ensure 先盖戳再干活，步骤失败标记已在——
//!   恰是本纪律要堵的洞。ensure 只给天然幂等的盖戳场景。）

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_DIR: &str = "/var/lib/aginx/done";

/// The marker directory (AGINX_DONE_DIR overrides for tests/dev loops).
pub fn dir() -> PathBuf {
    std::env::var_os("AGINX_DONE_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_DIR))
}

/// Marker names are file names: [A-Za-z0-9._-]+, no traversal, no empty.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.starts_with('.') && name.len() == 1 {
        return Err(format!("bad marker name '{name}'"));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-') {
        return Err(format!("bad marker name '{name}' (allowed: A-Za-z0-9._-)"));
    }
    if name == "." || name == ".." {
        return Err(format!("bad marker name '{name}'"));
    }
    Ok(())
}

/// Marked = a regular file sits at the marker path. A directory or an
/// unreadable anything is a BAD marker and reads as unmarked — the
/// acceptance rule from the M27 plan ("坏标记当不存在").
pub fn is_marked(name: &str) -> bool {
    match fs::metadata(dir().join(name)) {
        Ok(m) => m.is_file(),
        Err(_) => false,
    }
}

/// What the marker records, if anything legible (content is advisory).
pub fn marked_at(name: &str) -> Option<u64> {
    let s = fs::read_to_string(dir().join(name)).ok()?;
    s.trim().parse().ok()
}

/// Stamp the marker now; returns the recorded epoch seconds.
pub fn mark(name: &str) -> std::io::Result<u64> {
    validate_name(name).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    fs::create_dir_all(dir())?;
    fs::write(dir().join(name), format!("{now}\n"))?;
    Ok(now)
}

/// Remove one marker; a missing one is already gone (idempotent).
/// Returns true if something was removed.
pub fn reset(name: &str) -> bool {
    if validate_name(name).is_err() {
        return false;
    }
    fs::remove_file(dir().join(name)).is_ok()
}

/// Remove every regular file directly under the done dir.
/// Returns how many went.
pub fn reset_all() -> usize {
    let mut n = 0usize;
    if let Ok(rd) = fs::read_dir(dir()) {
        for e in rd.flatten() {
            if e.path().is_file() && fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
    }
    n
}

/// All live markers, name-sorted with their recorded epochs.
pub fn list() -> Vec<(String, Option<u64>)> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir()) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if validate_name(name).is_err() {
                    continue;
                }
                out.push((name.to_string(), marked_at(name)));
            }
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// dir() reads a process-global env var, so tests that point it
    /// anywhere must serialize (testkit convention #2: only when the
    /// code reads a global env var by design). The guard is part of the
    /// return so the lock lives as long as the test.
    fn tmp(tag: &str) -> (std::sync::MutexGuard<'static, ()>, PathBuf) {
        let g = testkit::env_lock();
        let d = testkit::tmp(&format!("aginx-done-{tag}"));
        std::env::set_var("AGINX_DONE_DIR", &d);
        (g, d)
    }

    #[test]
    fn default_dir_is_the_aginx_state_home() {
        // N5③ contract: /var/lib/aginx is THE state root on device.
        assert_eq!(DEFAULT_DIR, "/var/lib/aginx/done");
    }

    #[test]
    fn mark_check_reset_cycle() {
        let (_g, _d) = tmp("cycle");
        assert!(!is_marked("python-finalize"));
        let at = mark("python-finalize").unwrap();
        assert!(is_marked("python-finalize"));
        assert_eq!(marked_at("python-finalize"), Some(at));
        assert!(reset("python-finalize"));
        assert!(!is_marked("python-finalize"));
        assert!(!reset("python-finalize")); // idempotent
    }

    #[test]
    fn bad_marker_reads_as_unmarked() {
        let (_g, _d) = tmp("bad");
        fs::create_dir_all(dir().join("wedge")).unwrap(); // a directory squatting the path
        assert!(!is_marked("wedge"));
        assert_eq!(marked_at("wedge"), None);
    }

    #[test]
    fn traversal_and_weird_names_rejected() {
        let (_g, _d) = tmp("names");
        for bad in ["", "..", "../escape", "a/b", "with space"] {
            assert!(mark(bad).is_err(), "{bad:?} should be refused");
        }
        for good in ["python-finalize", "step_2", "v1.0", "x"] {
            assert!(mark(good).is_ok(), "{good:?} should be accepted");
        }
    }

    #[test]
    fn reset_all_takes_only_regular_files() {
        let (_g, _d) = tmp("all");
        mark("a").unwrap();
        mark("b").unwrap();
        fs::create_dir_all(dir().join("keepdir")).unwrap();
        assert_eq!(reset_all(), 2);
        assert!(list().is_empty());
        assert!(dir().join("keepdir").is_dir()); // untouched, not our business
    }
}
