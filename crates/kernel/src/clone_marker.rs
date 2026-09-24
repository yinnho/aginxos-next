//! `[CLONE_INSTALL:<name>]` reply marker — kernel-side clone installation.
//!
//! The clone-creator agent generates a new clone's definition layer
//! **incrementally**: each file lands on disk in its own workspace under
//! `staging/<name>/` via one `file_write` call per file. That makes
//! generation crash-resumable (a timeout/compaction mid-way loses nothing —
//! the next turn lists `staging/<name>/` and writes only the missing files)
//! and removes the giant single-payload `clone_install` tool call that
//! previously died on 180s timeouts.
//!
//! When the agent's final reply carries `[CLONE_INSTALL:<name>]`, the kernel —
//! not the agent — performs the side effect (reliable-side-effects doctrine,
//! same pattern as the `[PUBLISH:app_id]` marker in opencarrier): read
//! `staging/<name>/` → `clone_install_files` (format validation, workspace
//! write, manifest, spawn) → replace the marker with a receipt. On failure the
//! receipt carries the structured validation error and staging is preserved,
//! so the next turn repairs the named files and re-emits the marker.
//!
//! The handler runs in the kernel messaging layer at turn finalization —
//! channel-agnostic (webui / WeChat / cron all pass through it).

use std::collections::BTreeMap;
use std::path::Path;

use tracing::{info, warn};

use crate::kernel::CarrierKernel;

/// Marker prefix scanned for in reply text.
pub const MARKER_PREFIX: &str = "[CLONE_INSTALL:";

/// A parsed marker: the clone name and the verbatim span to replace.
pub struct CloneInstallMarker {
    pub name: String,
    pub span: String,
}

/// Parse all `[CLONE_INSTALL:<name>]` markers from reply text.
///
/// The span is returned verbatim so the handler can replace exactly what was
/// parsed (name is trimmed; the span keeps original whitespace if any).
pub fn parse_clone_install_markers(text: &str) -> Vec<CloneInstallMarker> {
    let mut markers = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(MARKER_PREFIX) {
        let after = &rest[start + MARKER_PREFIX.len()..];
        match after.find(']') {
            Some(end) => {
                let name = after[..end].trim();
                if !name.is_empty() && !name.contains('[') {
                    markers.push(CloneInstallMarker {
                        name: name.to_string(),
                        span: format!("{MARKER_PREFIX}{}]", &after[..end]),
                    });
                }
                rest = &after[end + 1..];
            }
            // Unterminated marker — leave it alone (probably prose).
            None => break,
        }
    }
    markers
}

/// Clone-name gate, mirroring the rules enforced inside
/// [`CarrierKernel::clone_install_files`] (1-64 lowercase alnum/hyphen, no
/// leading/trailing hyphen). Checked BEFORE any filesystem access so a
/// malformed marker can never name a directory outside `staging/`.
pub fn valid_clone_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Recursively collect files under `dir` into `relpath -> bytes`.
///
/// Dot-entries (`.dup/`, `.DS_Store`, editor droppings) are skipped — they are
/// workspace machinery, not definition-layer files.
fn collect_staging_files(dir: &Path, prefix: &str, out: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().to_string();
        if fname.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() {
            fname
        } else {
            format!("{prefix}/{fname}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect_staging_files(&path, &rel, out)?;
        } else {
            out.insert(rel, std::fs::read(&path)?);
        }
    }
    Ok(())
}

impl CarrierKernel {
    /// Execute `[CLONE_INSTALL:<name>]` markers in a completed agent reply,
    /// rewriting each marker span into a user-facing receipt (success or
    /// structured failure). Called at turn finalization on every channel.
    ///
    /// `workspace` is the *emitting agent's* workspace — staging is its own
    /// `staging/<name>/` subdir, so a half-built clone is always visible to
    /// the agent that made it, from any channel.
    pub async fn process_clone_install_markers(
        &self,
        agent_name: &str,
        workspace: Option<&Path>,
        response: &mut String,
    ) {
        let markers = parse_clone_install_markers(response);
        if markers.is_empty() {
            return;
        }

        // Dedupe by name: the same marker emitted twice installs once.
        let mut seen: Vec<String> = Vec::new();

        for marker in markers {
            if seen.iter().any(|n| n == &marker.name) {
                // Duplicate span of an already-processed name: strip it (its
                // receipt is already in the text).
                *response = response.replace(&marker.span, "");
                continue;
            }
            seen.push(marker.name.clone());

            let name = marker.name.clone();
            let span = marker.span.clone();

            if !valid_clone_name(&name) {
                warn!(%name, "CLONE_INSTALL marker rejected: invalid clone name");
                let receipt = format!(
                    "⚠️ [CLONE_INSTALL:{name}] 分身名不合法（需 1-64 位小写字母/数字/短横线，首尾不能是短横线），修正后在回复里重发标记"
                );
                *response = response.replace(&span, &receipt);
                continue;
            }

            let Some(workspace) = workspace else {
                warn!(%name, %agent_name, "CLONE_INSTALL marker ignored: emitting agent has no workspace");
                let receipt = format!(
                    "⚠️ [CLONE_INSTALL:{name}] 发出标记的分身（{agent_name}）没有 workspace，无法从 staging 安装"
                );
                *response = response.replace(&span, &receipt);
                continue;
            };

            let staging = workspace.join("staging").join(&name);
            let mut files = BTreeMap::new();
            let collect_err = match collect_staging_files(&staging, "", &mut files) {
                Ok(()) if files.is_empty() => None,
                Ok(()) => None,
                Err(e) => Some(e),
            };

            if collect_err.is_some() || files.is_empty() {
                let reason = match &collect_err {
                    Some(e) => format!("读取 staging/{name}/ 失败：{e}"),
                    None => format!("staging/{name}/ 是空的或不存在"),
                };
                warn!(%name, %reason, "CLONE_INSTALL marker: staging not ready");
                let receipt = format!(
                    "⚠️ [CLONE_INSTALL:{name}] {reason}——先用 file_write 把定义层文件逐个写到 staging/{name}/（template.json 先行），全部写完后在回复里重发 [CLONE_INSTALL:{name}]"
                );
                *response = response.replace(&span, &receipt);
                continue;
            }

            info!(
                %name,
                files = files.len(),
                emitting = %agent_name,
                "CLONE_INSTALL marker: installing clone from staging"
            );
            match self.clone_install_files(&name, files).await {
                Ok((agent_id, _installed_name, display)) => {
                    // Success: staging has been consumed into the real
                    // workspace — remove it so a re-emitted marker doesn't
                    // reinstall from a stale copy.
                    if let Err(e) = std::fs::remove_dir_all(&staging) {
                        warn!(%name, error = %e, "Failed to remove consumed staging dir");
                    }
                    info!(%name, %agent_id, "CLONE_INSTALL marker: clone installed");
                    let receipt = format!(
                        "✅ 分身「{display}」（{name}）已安装上线（agent_id={agent_id}），去侧栏/通讯录就能看到它"
                    );
                    *response = response.replace(&span, &receipt);
                }
                Err(e) => {
                    // Failure: staging stays on disk; the receipt carries the
                    // structured error so the next turn repairs only the named
                    // files and re-emits the marker.
                    warn!(%name, error = %e, "CLONE_INSTALL marker: install failed");
                    let receipt = format!(
                        "⚠️ [CLONE_INSTALL:{name}] 安装未通过：{e}\n半成品保留在 staging/{name}/——按上面的错误修复对应文件后，在回复里重发 [CLONE_INSTALL:{name}] 即可"
                    );
                    *response = response.replace(&span, &receipt);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_marker_with_surrounding_text() {
        let markers = parse_clone_install_markers(
            "定义层文件已全部写好，下面安装：\n[CLONE_INSTALL:customer-support]\n完成后请查看。",
        );
        assert_eq!(markers.len(), 1);
        assert_eq!(markers[0].name, "customer-support");
        assert_eq!(markers[0].span, "[CLONE_INSTALL:customer-support]");
    }

    #[test]
    fn parses_multiple_markers_in_order() {
        let markers =
            parse_clone_install_markers("[CLONE_INSTALL:alpha] mid [CLONE_INSTALL:beta-2]");
        assert_eq!(
            markers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "beta-2"]
        );
    }

    #[test]
    fn ignores_unterminated_and_nested_markers() {
        // Unterminated — prose like "[CLONE_INSTALL: 未完成" must not parse.
        assert!(parse_clone_install_markers("[CLONE_INSTALL: 未完成").is_empty());
        // Nested bracket inside name — reject (span ambiguous).
        assert!(parse_clone_install_markers("[CLONE_INSTALL:a[b]]").is_empty());
        // Empty name.
        assert!(parse_clone_install_markers("[CLONE_INSTALL:]").is_empty());
    }

    #[test]
    fn no_markers_leaves_text_untouched_semantics() {
        assert!(parse_clone_install_markers("普通回复，没有标记").is_empty());
        assert!(parse_clone_install_markers("CLONE_INSTALL:no-brackets").is_empty());
    }

    #[test]
    fn clone_name_rules() {
        assert!(valid_clone_name("a"));
        assert!(valid_clone_name("customer-support-2"));
        assert!(!valid_clone_name(""));
        assert!(!valid_clone_name("-lead"));
        assert!(!valid_clone_name("trail-"));
        assert!(!valid_clone_name("UpperCase"));
        assert!(!valid_clone_name("with_underscore"));
        assert!(!valid_clone_name("with space"));
        assert!(!valid_clone_name("中文"));
        assert!(!valid_clone_name(&"x".repeat(65)));
    }

    #[test]
    fn staging_collection_walks_and_skips_dots() {
        let dir = std::env::temp_dir().join(format!(
            "ocm-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("my-clone");
        std::fs::create_dir_all(root.join("knowledge/sub")).unwrap();
        std::fs::create_dir_all(root.join(".dup")).unwrap();
        std::fs::write(root.join("SOUL.md"), "soul").unwrap();
        std::fs::write(root.join("knowledge/a.md"), "a").unwrap();
        std::fs::write(root.join("knowledge/sub/b.md"), "b").unwrap();
        std::fs::write(root.join(".dup/objects"), "history").unwrap();
        std::fs::write(root.join(".DS_Store"), "junk").unwrap();

        let mut files = BTreeMap::new();
        collect_staging_files(&root, "", &mut files).unwrap();
        assert_eq!(
            files.keys().cloned().collect::<Vec<_>>(),
            vec![
                "SOUL.md".to_string(),
                "knowledge/a.md".to_string(),
                "knowledge/sub/b.md".to_string()
            ]
        );
        assert_eq!(files["knowledge/sub/b.md"], b"b");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn staging_collection_missing_dir_is_io_error_caller_handles() {
        let mut files = BTreeMap::new();
        // Missing dir → read_dir error (caller renders "staging 不存在").
        assert!(collect_staging_files(Path::new("/nonexistent-ocm"), "", &mut files).is_err());
        assert!(files.is_empty());
    }
}
