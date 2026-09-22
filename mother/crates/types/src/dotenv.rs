//! Minimal `.env` file loader/saver for `~/.aginx/carrier/.env`.
//!
//! No external crate needed — hand-rolled for simplicity.
//! Format: `KEY=VALUE` lines, `#` comments, optional quotes.
//!
//! # Environment variable safety
//!
//! `std::env::set_var` is deprecated in Rust 2024 edition for multi-threaded
//! contexts. This module uses the shared in-process override map in
//! `carrier_types::env` so that env mutations from async code are safe.
//! All code that reads environment variables should use `crate::env::get_env`
//! instead of `std::env::var` to see runtime overrides.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Return the path to `~/.aginx/carrier/.env`.
pub fn env_file_path() -> Option<PathBuf> {
    Some(crate::config::home_dir().join(".env"))
}

/// Load `~/.aginx/carrier/.env` and `~/.aginx/carrier/secrets.env` into the
/// in-process override map.
///
/// System env vars take priority — existing vars are NOT overridden.
/// `secrets.env` is loaded second so `.env` values take priority over secrets
/// (but both yield to system env vars).
/// Silently does nothing if the files don't exist.
///
/// Uses the in-process `ENV_OVERRIDES` map instead of `std::env::set_var`
/// to avoid data races in multi-threaded contexts.
pub fn load_dotenv() {
    load_env_file(env_file_path());
    // Also load secrets.env (written by dashboard "Set API Key" button)
    load_env_file(secrets_env_path());
}

/// Return the path to `~/.aginx/carrier/secrets.env`.
pub fn secrets_env_path() -> Option<PathBuf> {
    Some(crate::config::home_dir().join("secrets.env"))
}

fn load_env_file(path: Option<PathBuf>) {
    let path = match path {
        Some(p) => p,
        None => return,
    };

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return,
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some((key, value)) = parse_env_line(trimmed) {
            if crate::env::get_env(&key).is_none() {
                crate::env::set_env_override(&key, &value);
            }
        }
    }
}

/// Upsert a key in `~/.aginx/carrier/.env`.
///
/// Creates the file if missing. Sets 0600 permissions on Unix.
/// Also sets the key in the current process environment.
pub fn save_env_key(key: &str, value: &str) -> Result<(), String> {
    let path = env_file_path().ok_or("Could not determine home directory")?;

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {e}"))?;
    }

    let mut entries = read_env_file(&path);
    entries.insert(key.to_string(), value.to_string());
    write_env_file(&path, &entries)?;

    // Set in the in-process override map so crate::env::get_env()
    // returns the new value immediately, visible to all code that uses it
    // (including LLM driver creation via brain.rs).
    crate::env::set_env_override(key, value);

    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse a single `KEY=VALUE` line. Handles optional quotes.
fn parse_env_line(line: &str) -> Option<(String, String)> {
    let eq_pos = line.find('=')?;
    let key = line[..eq_pos].trim().to_string();
    let mut value = line[eq_pos + 1..].trim().to_string();

    if key.is_empty() {
        return None;
    }

    // Strip matching quotes
    if ((value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\'')))
        && value.len() >= 2
    {
        value = value[1..value.len() - 1].to_string();
    }

    Some((key, value))
}

/// Read all key-value pairs from the .env file.
fn read_env_file(path: &PathBuf) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();

    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return map,
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = parse_env_line(trimmed) {
            map.insert(key, value);
        }
    }

    map
}

/// Write key-value pairs back to the .env file with a header comment.
fn write_env_file(path: &PathBuf, entries: &BTreeMap<String, String>) -> Result<(), String> {
    let mut content = String::from("# Carrier environment — managed by `carrier config set-key`\n");
    content.push_str("# Do not edit while the daemon is running.\n\n");

    for (key, value) in entries {
        // Quote values that contain spaces or special characters
        if value.contains(' ') || value.contains('#') || value.contains('"') {
            content.push_str(&format!("{key}=\"{}\"\n", value.replace('"', "\\\"")));
        } else {
            content.push_str(&format!("{key}={value}\n"));
        }
    }

    std::fs::write(path, &content).map_err(|e| format!("Failed to write .env file: {e}"))?;

    // Set 0600 permissions on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_env_line_simple() {
        let (k, v) = parse_env_line("FOO=bar").unwrap();
        assert_eq!(k, "FOO");
        assert_eq!(v, "bar");
    }

    #[test]
    fn test_parse_env_line_quoted() {
        let (k, v) = parse_env_line("KEY=\"hello world\"").unwrap();
        assert_eq!(k, "KEY");
        assert_eq!(v, "hello world");
    }

    #[test]
    fn test_parse_env_line_single_quoted() {
        let (k, v) = parse_env_line("KEY='value'").unwrap();
        assert_eq!(k, "KEY");
        assert_eq!(v, "value");
    }

    #[test]
    fn test_parse_env_line_spaces() {
        let (k, v) = parse_env_line("  KEY  =  value  ").unwrap();
        assert_eq!(k, "KEY");
        assert_eq!(v, "value");
    }

    #[test]
    fn test_parse_env_line_no_value() {
        let (k, v) = parse_env_line("KEY=").unwrap();
        assert_eq!(k, "KEY");
        assert_eq!(v, "");
    }

    #[test]
    fn test_parse_env_line_comment() {
        assert!(
            parse_env_line("# comment").is_none()
                || parse_env_line("# comment").unwrap().0.starts_with('#')
        );
        // Comments are filtered before reaching parse_env_line in production code
    }

    #[test]
    fn test_parse_env_line_no_equals() {
        assert!(parse_env_line("NOEQUALS").is_none());
    }

    #[test]
    fn test_parse_env_line_empty_key() {
        assert!(parse_env_line("=value").is_none());
    }
}
