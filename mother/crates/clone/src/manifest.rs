//! File-level manifest of a clone workspace's definition layer.
//!
//! Used by the `dup` VCS and the opencarrier dup-remote endpoints to do
//! git-style file-level sync (no packed archive). The manifest is a map of
//! relative path -> SHA-256 for every definition-layer file, plus a top-level
//! `hash` (SHA-256 of the sorted `path:hash` serialization) used as a state id
//! for fast-forward comparison.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Runtime dirs/files excluded from the definition-layer manifest: agent
/// runtime state (output/sessions/history/logs/...), the `.dup/` VCS state dir,
/// `admins.json` (deployment-specific admin list), and `api_tools.toml`
/// (deployment-specific API tool config - endpoints/HMAC keys, not shareable
/// via dup/DupHub; managed via the `api_tool_register` admin tool, like
/// `bind_agent` in session.json).
const SKIP: &[&str] = &[
    "agent.toml",
    "AGENT.json",
    "admins.json",
    "api_tools.toml",
    "output",
    "sessions",
    "history",
    "logs",
    "users",
    "data",
    "senders",
    ".lifecycle",
    ".dup",
];

/// True if a top-level entry is a test-workspace dir: `test`, `test2`, ... or
/// `test-foo`. (Catches `test`/`testN` that the old `test-` prefix missed.)
pub fn is_test_dir(top: &str) -> bool {
    top == "test"
        || top.starts_with("test-")
        || (top.starts_with("test") && top[4..].chars().all(|c| c.is_ascii_digit()))
}

/// True if a file name is a backup: `foo.bak` or `foo.bak.<timestamp>`.
pub fn is_bak(top: &str) -> bool {
    top.ends_with(".bak") || top.contains(".bak.")
}

/// A file-level snapshot of a workspace's definition layer.
///
/// `files` maps relative path -> hex SHA-256 of file content. `hash` is the
/// SHA-256 of the sorted `path:hash` serialization, a stable state id used to
/// detect fast-forward / divergence without transferring file contents.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    /// relative path -> hex SHA-256 of file content
    pub files: BTreeMap<String, String>,
    /// SHA-256 of the sorted `path:hash` serialization - state id
    pub hash: String,
}

impl Manifest {
    /// An empty manifest (no tracked files). Its `hash` is the SHA-256 of "".
    pub fn empty() -> Self {
        let mut files = BTreeMap::new();
        let hash = manifest_hash(&files);
        files.clear(); // keep clippy happy; empty either way
        Manifest { files, hash }
    }
}

/// Build a manifest by walking `workspace` and hashing every definition-layer
/// file. Selection uses `iter_definition_files` (the shared definition-layer
/// walk, excludes runtime dirs + `.dup/` VCS state), so the manifest tracks
/// exactly the files sent file-level to DupHub.
pub fn build_manifest(workspace: &Path) -> Result<Manifest> {
    let entries = iter_definition_files(workspace)?;
    let mut files: BTreeMap<String, String> = BTreeMap::new();
    for (rel, abs) in &entries {
        let data = std::fs::read(abs).with_context(|| format!("read {}", abs.display()))?;
        files.insert(rel.clone(), sha256_hex(&data));
    }
    let hash = manifest_hash(&files);
    Ok(Manifest { files, hash })
}

/// Read every definition-layer file in `workspace` into a `path -> bytes` map.
/// The local-side mirror of `hub::fetch_dup_files`: shares `iter_definition_files`
/// with `build_manifest`, so the file set (and thus the manifest hash) is
/// guaranteed identical. Used by `hub push` to send file-level content to DupHub.
///
/// Uses `SKIP` (which includes `.dup/`), so the local `.dup/` VCS state is NOT
/// leaked into the pushed payload.
pub fn collect_definition_files(workspace: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let entries = iter_definition_files(workspace)?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (rel, abs) in &entries {
        let data = std::fs::read(abs).with_context(|| format!("read {}", abs.display()))?;
        files.insert(rel.clone(), data);
    }
    Ok(files)
}

/// Walk `workspace` and return the sorted `(rel_path, abs_path)` pairs for every
/// definition-layer file. Applies `SKIP` (runtime dirs, `agent.toml`,
/// `AGENT.json`, `admins.json`, `.dup/`) with `is_test_dir` and `is_bak`, and
/// skips macOS `._`/`.DS_Store` plus dup VCS artifacts (`.dup-theirs`,
/// `.duptmp`). Shared by `build_manifest` and `collect_definition_files` so both
/// enumerate the same file set.
fn iter_definition_files(workspace: &Path) -> Result<Vec<(String, std::path::PathBuf)>> {
    let mut out: Vec<(String, std::path::PathBuf)> = Vec::new();
    walk_collect(workspace, workspace, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn walk_collect(
    base: &Path,
    cur: &Path,
    out: &mut Vec<(String, std::path::PathBuf)>,
) -> Result<()> {
    let entries = std::fs::read_dir(cur).with_context(|| format!("read_dir {}", cur.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let name_str = entry.file_name().to_string_lossy().into_owned();

        // Skip macOS Apple Double / .DS_Store
        if name_str.starts_with("._") || name_str == ".DS_Store" {
            continue;
        }
        // Skip dup VCS artifacts: conflict sidecars + transient write tmps.
        if name_str.ends_with(".dup-theirs") || name_str.ends_with(".duptmp") {
            continue;
        }
        // Skip Python bytecode: `__pycache__/` dirs and loose `*.pyc` files.
        // Server-side validator runs (`python3 flows/<n>/scripts/*.py`) drop
        // __pycache__ next to the imported modules; regenerated on every run,
        // they are runtime build artifacts, never definition layer. Unfiltered
        // they polluted the dup manifest and drifted every local pull
        // (2026-08-17: three .pyc entries in the 86bus manifest).
        if path.is_dir() && name_str == "__pycache__" {
            continue;
        }
        if name_str.ends_with(".pyc") {
            continue;
        }

        if path.is_dir() {
            walk_collect(base, &path, out)?;
        } else {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let top = rel_str.split('/').next().unwrap_or(&rel_str);
            // Skip runtime layer + .dup VCS + test-workspace dirs + backup files.
            if SKIP.contains(&top) || is_test_dir(top) || is_bak(top) {
                continue;
            }
            out.push((rel_str, path));
        }
    }
    Ok(())
}

/// Compute the manifest state id: SHA-256 of the sorted `path:hash` lines.
pub fn manifest_hash(files: &BTreeMap<String, String>) -> String {
    let mut h = Sha256::new();
    for (p, sha) in files {
        h.update(p.as_bytes());
        h.update(b":");
        h.update(sha.as_bytes());
        h.update(b"\n");
    }
    format!("{:x}", h.finalize())
}

/// Compute SHA-256 of arbitrary bytes (hex).
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    format!("{:x}", h.finalize())
}

/// Ensure `template.json` in `files` has a `version` field.
///
/// DupHub requires the `version` field in `template.json` to extract listing
/// metadata (description/display_name/category) - a clone whose template.json
/// lacks `version` publishes with an EMPTY DupHub listing (the files are there,
/// but the listing shows no description/name/category). clone-creator's flow
/// example includes `"version":"1"`, but agents sometimes omit it.
///
/// Adds `"version": "1"` if (and only if) the field is missing. Never overwrites
/// an existing `version` - re-install/publish must not rewrite it (that would
/// change the file hash and defeat dup debounce). Returns `true` if a version
/// was added. Invalid/non-object template.json is left untouched (returns false).
pub fn ensure_template_version(files: &mut BTreeMap<String, Vec<u8>>) -> bool {
    let Some(bytes) = files.get_mut("template.json") else {
        return false;
    };
    let Ok(mut tj) = serde_json::from_slice::<serde_json::Value>(bytes.as_slice()) else {
        return false;
    };
    let Some(obj) = tj.as_object_mut() else {
        return false;
    };
    if obj.contains_key("version") {
        return false;
    }
    obj.insert(
        "version".to_string(),
        serde_json::Value::String("1".to_string()),
    );
    match serde_json::to_vec_pretty(&tj) {
        Ok(new_bytes) => {
            *bytes = new_bytes;
            true
        }
        Err(_) => false,
    }
}

/// Install-time hard format validation (the "law enforcement" half of
/// `docs/CLONE-FORMAT.md`). Rejects definition-layer layouts the runtime
/// silently mis-parses — the two historical killers:
///
/// 1. Top-level `skills/` directory: `scan_flows` only scans `flows/`, so every
///    skill shipped there is invisible (the batch-generation-era plague).
/// 2. Flow files without a non-empty `description` frontmatter: the flow isn't
///    injected, and every tool it declares is dead.
///
/// The error message is deliberately structured (file -> what's wrong ->
/// expectation -> fix hint) so an agent receiving it via `Error: …` can
/// self-repair in one round instead of hammering the same failing call.
/// This is a WARN-level gate, not a hard reject, for `skills/` — legacy hub
/// templates re-install through this path, and refusing them would break the
/// hub reinstall flow wholesale. The errors below are returned as a list of
/// actionable strings by the caller's discretion.
pub fn validate_install_format(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<String>> {
    let mut errors = Vec::new();

    for (rel, content) in files {
        let top = rel.split('/').next().unwrap_or(rel);
        // 1. Legacy `skills/` top-level directory — dead on arrival.
        if top == "skills" {
            errors.push(format!(
                "文件 '{rel}' 位于已废弃的 skills/ 目录——运行时只扫描 flows/，skills/ 下的流程完全不可见。\
                 修复：把 skills/<名称>/SKILL.md 移到 flows/<名称>/flow.md（frontmatter 保持 name/description/version），\
                 并从本次提交中删除 skills/ 路径的文件"
            ));
            continue;
        }

        // 2. Flow definition files under flows/ must carry a non-empty
        //    single-line `description` (block scalars read as literal "|").
        let segs: Vec<&str> = rel.split('/').collect();
        if segs.len() >= 3 && segs[0] == "flows" {
            let fname = segs.last().copied().unwrap_or_default();
            if fname == "flow.md" || fname == "SKILL.md" {
                let text = String::from_utf8_lossy(content);
                if let Some(desc) = extract_frontmatter_description(&text) {
                    let trimmed = desc.trim();
                    if trimmed.is_empty() || trimmed == "|" || trimmed.len() < 4 {
                        errors.push(format!(
                            "流程文件 '{rel}' 的 description 为空/过短/块标量——空 description 的 flow 不会被注入，\
                             其声明的工具全部失效。修复：frontmatter 写单行非空 description（一句话用途，≤50字），\
                             不要用 YAML 块标量（description: | 多行）"
                        ));
                    }
                } else {
                    errors.push(format!(
                        "流程文件 '{rel}' 缺少 description 字段（或无 frontmatter）——\
                         修复：frontmatter 必须包含 name、description（单行非空）、version"
                    ));
                }

                // 3. shell_allow declarations: structural lint (forbidden base /
                //    `*` bypass) + golden-sample (match/not_match) verification.
                let def = carrier_types::flow::parse_flow_def(&text);
                errors.extend(
                    carrier_types::flow::validate_shell_allow(&def)
                        .into_iter()
                        .map(|e| format!("文件 '{rel}': {e}")),
                );
            }
        }
    }

    Ok(errors)
}

/// Extract the `description:` value from a flow file's frontmatter
/// (line-oriented, mirrors `carrier_types::flow::parse_flow_def` — including its
/// single-line limitation, which is exactly what we're validating against).
fn extract_frontmatter_description(content: &str) -> Option<String> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    for line in rest[..end].lines() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("description:") {
            return Some(v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

/// 安装预览（M30 权限预览）：一批安装文件将带来的 flows 与 shell 权限。
/// 只读不改；`agent install --dry-run` 消费。与 validate_install_format
/// 同源走 `parse_flow_def`，保证预览说的就是安装闸门看的。
pub struct InstallPreview {
    pub file_count: usize,
    pub flows: Vec<FlowPreview>,
}

pub struct FlowPreview {
    pub path: String,
    pub name: String,
    pub description: String,
    pub shell_allow: Vec<String>,
}

pub fn install_preview(files: &BTreeMap<String, Vec<u8>>) -> InstallPreview {
    let mut flows = Vec::new();
    for (rel, content) in files {
        let segs: Vec<&str> = rel.split('/').collect();
        if segs.len() >= 3 && segs[0] == "flows" {
            let fname = segs.last().copied().unwrap_or_default();
            if fname == "flow.md" {
                let text = String::from_utf8_lossy(content);
                let def = carrier_types::flow::parse_flow_def(&text);
                flows.push(FlowPreview {
                    path: rel.clone(),
                    name: if def.name.is_empty() {
                        segs[segs.len() - 2].to_string()
                    } else {
                        def.name
                    },
                    description: def.description,
                    shell_allow: def.shell_allow,
                });
            }
        }
    }
    flows.sort_by(|a, b| a.path.cmp(&b.path));
    InstallPreview { file_count: files.len(), flows }
}

/// Write a set of files (`path -> bytes`) into `workspace`, enforcing the same
/// definition-layer + traversal safety as the `dup` push endpoint. Creates
/// parent dirs and writes atomically (`.duptmp` + rename). Files outside the
/// definition layer (runtime dirs, `agent.toml`/`AGENT.json`, test dirs, `.bak`)
/// are skipped with a warning rather than written.
///
/// This writes individually-fetched files (e.g. pulled from a DupHub manifest)
/// into the workspace, enforcing the definition-layer boundary. Returns security
/// warnings (empty on clean).
pub fn write_files_to_workspace(
    files: &BTreeMap<String, Vec<u8>>,
    workspace: &Path,
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    // The workspace may not exist yet (fresh install, unlike the dup push path
    // where it always does) - create it so canonicalize works.
    std::fs::create_dir_all(workspace).with_context(|| format!("mkdir {}", workspace.display()))?;
    let ws_canonical = workspace
        .canonicalize()
        .with_context(|| format!("canonicalize {}", workspace.display()))?;
    for (rel, content) in files {
        let p = Path::new(rel);
        if p.components()
            .any(|c| matches!(c, Component::ParentDir | Component::RootDir))
        {
            anyhow::bail!("path traversal denied: {rel}");
        }
        let top = rel.split('/').next().unwrap_or(rel);
        if SKIP.contains(&top) || is_test_dir(top) || is_bak(top) {
            warnings.push(format!("skipped non-definition-layer file: {rel}"));
            continue;
        }
        // Python bytecode never re-enters via an old hub/remote manifest
        // (built before the walk filter): mirror the walk_collect exclusion
        // on the inbound path.
        if rel.split('/').any(|c| c == "__pycache__") || rel.ends_with(".pyc") {
            warnings.push(format!("skipped python bytecode artifact: {rel}"));
            continue;
        }
        let file_path = workspace.join(rel);
        // Defense-in-depth: for an EXISTING entry, canonicalize and ensure it
        // stays within the workspace (catches symlink escape). New files are
        // already confined by the component check above (no `..` / root), so we
        // don't canonicalize their (possibly non-existent) parent - that would
        // compare a canonical workspace against a non-canonical join and false-
        // reject when the workspace ancestor is a symlink (e.g. macOS /var).
        if file_path.exists() {
            let canon = file_path
                .canonicalize()
                .unwrap_or_else(|_| file_path.clone());
            if !canon.starts_with(&ws_canonical) {
                anyhow::bail!("path traversal denied: {rel}");
            }
        }
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).with_context(|| format!("mkdir {rel}"))?;
        }
        // Atomic write: sibling .duptmp then rename.
        let filename = file_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = file_path.with_file_name(format!(".{filename}.duptmp"));
        std::fs::write(&tmp, content).with_context(|| format!("write {rel}"))?;
        if let Err(e) = std::fs::rename(&tmp, &file_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("rename {rel}"));
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn ensure_template_version_adds_missing_version() {
        let mut files = BTreeMap::new();
        files.insert(
            "template.json".to_string(),
            br#"{"name":"x","description":"d","display_name":"X","category":"c"}"#.to_vec(),
        );
        assert!(ensure_template_version(&mut files));
        let tj: serde_json::Value = serde_json::from_slice(&files["template.json"]).unwrap();
        assert_eq!(tj["version"], "1");
        assert_eq!(tj["description"], "d");
        assert_eq!(tj["display_name"], "X");
    }

    #[test]
    fn ensure_template_version_preserves_existing_version() {
        let mut files = BTreeMap::new();
        let orig = br#"{"name":"x","version":"3","description":"d"}"#.to_vec();
        files.insert("template.json".to_string(), orig.clone());
        assert!(!ensure_template_version(&mut files));
        // Unchanged (never rewrites an existing version - would defeat debounce).
        assert_eq!(files["template.json"], orig);
        let tj: serde_json::Value = serde_json::from_slice(&files["template.json"]).unwrap();
        assert_eq!(tj["version"], "3");
    }

    #[test]
    fn ensure_template_version_handles_missing_and_invalid() {
        // No template.json at all.
        let mut files = BTreeMap::new();
        files.insert("SOUL.md".to_string(), b"soul".to_vec());
        assert!(!ensure_template_version(&mut files));
        // Invalid JSON -> untouched.
        let mut files2 = BTreeMap::new();
        let bad = b"{not json".to_vec();
        files2.insert("template.json".to_string(), bad.clone());
        assert!(!ensure_template_version(&mut files2));
        assert_eq!(files2["template.json"], bad);
    }

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("oc-manifest-{name}"));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    #[test]
    fn write_files_creates_fresh_workspace_and_writes() {
        let tmp = tmp_dir("fresh");
        assert!(!tmp.exists(), "precondition: workspace must not exist");
        let mut files = BTreeMap::new();
        files.insert("SOUL.md".to_string(), b"hello".to_vec());
        files.insert("knowledge/nested/deep.md".to_string(), b"world".to_vec());
        let warnings = write_files_to_workspace(&files, &tmp).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(std::fs::read(tmp.join("SOUL.md")).unwrap(), b"hello");
        assert_eq!(
            std::fs::read(tmp.join("knowledge/nested/deep.md")).unwrap(),
            b"world"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_files_rejects_traversal() {
        let tmp = tmp_dir("trav");
        let mut files = BTreeMap::new();
        files.insert("../escape.md".to_string(), b"x".to_vec());
        assert!(write_files_to_workspace(&files, &tmp).is_err());
        assert!(!tmp.join("../escape.md").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Python bytecode (`__pycache__/*.pyc`, loose `.pyc`) is a server-side
    /// validator run artifact, not definition layer: the manifest walk must
    /// never list it, and the inbound write path must refuse it even when an
    /// old remote/hub manifest (built pre-filter) carries it.
    #[test]
    fn pycache_never_in_manifest_nor_written() {
        let tmp = tmp_dir("pyc");
        std::fs::create_dir_all(tmp.join("flows/t/scripts/__pycache__")).unwrap();
        std::fs::write(tmp.join("flows/t/flow.md"), b"---\nname: t\n---\nb").unwrap();
        std::fs::write(tmp.join("flows/t/scripts/v.py"), b"print(1)").unwrap();
        std::fs::write(
            tmp.join("flows/t/scripts/__pycache__/v.cpython-310.pyc"),
            b"\x00 bytecode",
        )
        .unwrap();
        std::fs::write(tmp.join("flows/t/loose.pyc"), b"\x00 bytecode").unwrap();

        let manifest = build_manifest(&tmp).unwrap();
        assert!(manifest.files.contains_key("flows/t/flow.md"));
        assert!(manifest.files.contains_key("flows/t/scripts/v.py"));
        assert!(
            !manifest
                .files
                .keys()
                .any(|p| p.contains("__pycache__") || p.ends_with(".pyc")),
            "bytecode leaked into manifest: {:?}",
            manifest
                .files
                .keys()
                .filter(|p| p.contains("pycache") || p.ends_with(".pyc"))
                .collect::<Vec<_>>()
        );

        // Inbound: a stale manifest carrying .pyc entries is skipped with a
        // warning, never written to disk. (Drop the setup-time loose.pyc so
        // the not-written assertion can't pass on a setup artifact.)
        std::fs::remove_file(tmp.join("flows/t/loose.pyc")).unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            "flows/t/flow.md".to_string(),
            b"---\nname: t\n---\nb2".to_vec(),
        );
        files.insert(
            "flows/t/__pycache__/v.cpython-310.pyc".to_string(),
            b"\x00 bytecode".to_vec(),
        );
        files.insert("flows/t/loose.pyc".to_string(), b"\x00 bytecode".to_vec());
        let warnings = write_files_to_workspace(&files, &tmp).unwrap();
        assert_eq!(
            warnings.len(),
            2,
            "both bytecode entries warned: {warnings:?}"
        );
        assert!(!tmp.join("flows/t/__pycache__").exists());
        assert!(!tmp.join("flows/t/loose.pyc").exists());
        assert_eq!(
            std::fs::read(tmp.join("flows/t/flow.md")).unwrap(),
            b"---\nname: t\n---\nb2"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn write_files_skips_runtime_layer_with_warnings() {
        let tmp = tmp_dir("skip");
        let mut files = BTreeMap::new();
        files.insert("agent.toml".to_string(), b"regen".to_vec());
        files.insert("sessions/x.json".to_string(), b"runtime".to_vec());
        files.insert("admins.json".to_string(), b"adm".to_vec());
        files.insert("SOUL.md".to_string(), b"keep".to_vec());
        let warnings = write_files_to_workspace(&files, &tmp).unwrap();
        // agent.toml + sessions/ + admins.json skipped; SOUL.md kept.
        assert_eq!(warnings.len(), 3);
        assert!(!tmp.join("agent.toml").exists());
        assert!(!tmp.join("sessions/x.json").exists());
        assert!(!tmp.join("admins.json").exists());
        assert_eq!(std::fs::read(tmp.join("SOUL.md")).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn build_manifest_and_collect_definition_files_agree_and_exclude_dup() {
        let tmp = tmp_dir("agree");
        std::fs::create_dir_all(tmp.join("knowledge")).unwrap();
        std::fs::create_dir_all(tmp.join(".dup")).unwrap();
        std::fs::create_dir_all(tmp.join("sessions")).unwrap();
        std::fs::write(tmp.join("SOUL.md"), b"soul").unwrap();
        std::fs::write(tmp.join("knowledge/a.md"), b"a").unwrap();
        // .dup/ VCS state + sessions/ runtime must be excluded.
        std::fs::write(tmp.join(".dup/state"), b"vcs").unwrap();
        std::fs::write(tmp.join("sessions/x.json"), b"rt").unwrap();

        let manifest = build_manifest(&tmp).unwrap();
        let collected = collect_definition_files(&tmp).unwrap();

        // Identical file set (shared walk).
        let manifest_keys: Vec<&String> = manifest.files.keys().collect();
        let collect_keys: Vec<&String> = collected.keys().collect();
        assert_eq!(manifest_keys, collect_keys);

        // Definition files present, .dup + sessions excluded.
        assert!(manifest.files.contains_key("SOUL.md"));
        assert!(manifest.files.contains_key("knowledge/a.md"));
        assert!(!manifest.files.contains_key(".dup/state"));
        assert!(!manifest.files.contains_key("sessions/x.json"));

        // Per-file sha in manifest matches sha256 of collected bytes.
        assert_eq!(manifest.files["SOUL.md"], sha256_hex(b"soul"));
        assert_eq!(collected["SOUL.md"], b"soul");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn validate_install_format_rejects_legacy_layouts() {
        let mut files = BTreeMap::new();
        files.insert(
            "skills/answer/SKILL.md".to_string(),
            b"---\nname: answer\ndescription: x\nversion: 1\n---\nbody".to_vec(),
        );
        let errs = validate_install_format(&files).unwrap();
        assert!(errs.len() == 1 && errs[0].contains("skills/"), "{errs:?}");

        // Flow without description -> rejected.
        let mut files = BTreeMap::new();
        files.insert(
            "flows/write/flow.md".to_string(),
            b"---\nname: write\nversion: 1\n---\nbody".to_vec(),
        );
        let errs = validate_install_format(&files).unwrap();
        assert!(
            errs.len() == 1 && errs[0].contains("description"),
            "{errs:?}"
        );

        // YAML block scalar reads as literal "|" -> rejected.
        let mut files = BTreeMap::new();
        files.insert(
            "flows/write/flow.md".to_string(),
            b"---\nname: write\ndescription: |\n  multi line\nversion: 1\n---".to_vec(),
        );
        let errs = validate_install_format(&files).unwrap();
        assert_eq!(errs.len(), 1, "{errs:?}");

        // Canonical format passes clean.
        let mut files = BTreeMap::new();
        files.insert(
            "flows/write/flow.md".to_string(),
            "---\nname: write\ndescription: 写文章流程\nversion: 2\n---\nbody"
                .as_bytes()
                .to_vec(),
        );
        files.insert("template.json".to_string(), b"{}".to_vec());
        assert!(validate_install_format(&files).unwrap().is_empty());

        // Non-flow files under flows/ (scripts etc.) are not checked.
        let mut files = BTreeMap::new();
        files.insert(
            "flows/write/scripts/run.py".to_string(),
            b"print(1)".to_vec(),
        );
        assert!(validate_install_format(&files).unwrap().is_empty());
    }

    #[test]
    fn validate_install_format_checks_shell_allow() {
        // `*` shell_allow pattern (total bypass) is rejected at install.
        let mut files = BTreeMap::new();
        files.insert(
            "flows/write/flow.md".to_string(),
            "---\nname: write\ndescription: 写文章\nversion: 1\nshell_allow:\n  - \"*\"\n---\nbody"
                .as_bytes()
                .to_vec(),
        );
        let errs = validate_install_format(&files).unwrap();
        assert!(errs.iter().any(|e| e.contains("shell_allow")), "{errs:?}");

        // A not_match golden sample the pattern WOULD match is rejected.
        let mut files = BTreeMap::new();
        files.insert(
            "flows/write/flow.md".to_string(),
            "---\nname: write\ndescription: 写文章\nversion: 1\nshell_allow:\n  - pattern: \"python3 *\"\n    not_match: [\"python3 -c id\"]\n---\nbody"
                .as_bytes()
                .to_vec(),
        );
        let errs = validate_install_format(&files).unwrap();
        assert!(errs.iter().any(|e| e.contains("not_match")), "{errs:?}");

        // A clean map-form shell_allow passes with no errors.
        let mut files = BTreeMap::new();
        files.insert(
            "flows/write/flow.md".to_string(),
            "---\nname: write\ndescription: 写文章\nversion: 1\nshell_allow:\n  - \"python3 flows/write/scripts/*\"\n  - pattern: \"python3 flows/write/scripts/*\"\n    match: [\"python3 flows/write/scripts/run.py\"]\n    not_match: [\"rm -rf /\"]\n---\nbody"
                .as_bytes()
                .to_vec(),
        );
        assert!(
            validate_install_format(&files).unwrap().is_empty(),
            "{:?}",
            validate_install_format(&files).unwrap()
        );
    }
}
