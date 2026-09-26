//! knowledge 库 + flows 操作（M35 从 runtime tools/knowledge.rs 搬来）。
//!
//! 搬迁清单：knowledge_list / read / add / update / remove / import /
//! lint / heal / extract / index、clone_evaluate、flow_create / update /
//! load —— 全部只吃 workspace_root（桥经 `_ctx.workspace_root` 注入；
//! 人面用 --workspace）。apply_patch 与 session_summarize 留在 runtime
//! （前者走 agf_bridge 的 sender 域路径解析，后者吃轮次身份与记忆
//! 句柄——都是内核耦合面，不属记忆域）。
//!
//! 行为同构上游：同样的 frontmatter 约定、同样的敏感词闸（凭证进
//! kv 不进知识库）、同样的模糊匹配（exact → prefix → substring）、
//! 同样的版本留痕（carrier_lifecycle::version）。

use carrier_lifecycle as lifecycle;
use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Knowledge list / read
// ---------------------------------------------------------------------------

pub async fn knowledge_list(workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_list requires a workspace root".to_string(),
    ))?;
    let knowledge_dir = root.join("knowledge");

    if !knowledge_dir.exists() {
        return Ok("No knowledge files found (knowledge/ does not exist).".to_string());
    }

    let mut entries = tokio::fs::read_dir(&knowledge_dir)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read knowledge directory: {e}")))?;

    let mut files = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read entry: {e}")))?
    {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            let name = entry.file_name().to_string_lossy().to_string();
            // Try to extract title from frontmatter
            let title = tokio::fs::read_to_string(&path)
                .await
                .ok()
                .and_then(|content| extract_knowledge_title(&content));
            match title {
                Some(t) => files.push(format!("- {} ({})", t, name)),
                None => files.push(format!("- {}", name)),
            }
        }
    }

    files.sort();
    if files.is_empty() {
        Ok("No knowledge files found.".to_string())
    } else {
        Ok(format!(
            "Knowledge files ({}):\n{}",
            files.len(),
            files.join("\n")
        ))
    }
}

pub async fn knowledge_read(input: &Value, workspace_root: Option<&Path>) -> CarrierResult<String> {
    let filename = input["filename"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'filename' parameter".to_string(),
        ))?;
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_read requires a workspace root".to_string(),
    ))?;

    // Security: validate filename (no path traversal)
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err(CarrierError::InvalidInput(
            "Invalid filename: path separators and '..' are forbidden".to_string(),
        ));
    }
    if !filename.ends_with(".md") {
        return Err(CarrierError::InvalidInput(
            "Only .md knowledge files can be read".to_string(),
        ));
    }

    let path = root.join("knowledge").join(filename);

    if !path.exists() {
        // List available files so the LLM can correct the filename
        let knowledge_dir = root.join("knowledge");
        let available: Vec<String> = std::fs::read_dir(&knowledge_dir)
            .map(|entries| {
                let mut names: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        if name.ends_with(".md") {
                            Some(name)
                        } else {
                            None
                        }
                    })
                    .collect();
                names.sort();
                names
            })
            .unwrap_or_default();
        if available.is_empty() {
            return Ok(format!(
                "Knowledge file '{}' not found. No knowledge files exist yet.",
                filename
            ));
        }
        return Ok(format!(
            "Knowledge file '{}' not found. Available files: {}",
            filename,
            available.join(", ")
        ));
    }

    tokio::fs::read_to_string(&path)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read knowledge file: {e}")))
}

/// Extract `name` from YAML frontmatter of a knowledge file.
fn extract_knowledge_title(content: &str) -> Option<String> {
    let content = content.strip_prefix("---")?;
    let end = content.find("---")?;
    let frontmatter = &content[..end];

    for line in frontmatter.lines() {
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Lint / heal（lifecycle health 面）
// ---------------------------------------------------------------------------

pub async fn knowledge_lint(workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_lint requires a workspace root".to_string(),
    ))?;
    let report = lifecycle::health::check_health(root);
    if report.issues.is_empty() {
        Ok("All knowledge files are healthy.".to_string())
    } else {
        let mut out = format!("Found {} issue(s):\n", report.issues.len());
        for issue in &report.issues {
            out.push_str(&format!(
                "- [{:?}] {}: {}\n",
                issue.severity, issue.filename, issue.message
            ));
        }
        Ok(out)
    }
}

pub async fn knowledge_heal(workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_heal requires a workspace root".to_string(),
    ))?;
    let report = lifecycle::health::check_health(root);
    let fixes = lifecycle::health::auto_fix(root, &report);
    Ok(format!("Fixed {} issue(s).", fixes))
}

// ---------------------------------------------------------------------------
// Add / update / remove / import / extract / index
// ---------------------------------------------------------------------------

/// Core logic for adding a knowledge file. Shared by tool and train versions.
pub async fn knowledge_add_core(
    root: &Path,
    title: &str,
    content: &str,
    source_label: &str,
) -> CarrierResult<String> {
    let filename = lifecycle::evolution::sanitize_filename(title);
    let knowledge_dir = root.join("knowledge");
    tokio::fs::create_dir_all(&knowledge_dir)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to create knowledge dir: {e}")))?;
    let path = knowledge_dir.join(format!("{filename}.md"));
    let full = format!(
        "---\nname: {}\ndescription: {}\nconfidence: EXTRACTED\n---\n{}\n---\n",
        title, title, content
    );
    tokio::fs::write(&path, &full)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to write knowledge file: {e}")))?;
    let _ = lifecycle::version::record_version(
        root,
        "create",
        &format!("{filename}.md"),
        None,
        Some(&full),
        source_label,
    );
    Ok(filename)
}

/// Core logic for importing knowledge entries. Shared by tool and train versions.
pub async fn knowledge_import_core(
    root: &Path,
    data: &str,
    data_type: &str,
) -> CarrierResult<(Vec<String>, lifecycle::parsers::ParseQuality)> {
    let result = lifecycle::parsers::parse_import_data(data, data_type)
        .map_err(|e| CarrierError::Serialization(format!("Parse failed: {e}")))?;
    let knowledge_dir = root.join("knowledge");
    tokio::fs::create_dir_all(&knowledge_dir)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to create knowledge dir: {e}")))?;
    let mut saved = Vec::new();
    for entry in &result.entries {
        let filename = lifecycle::evolution::sanitize_filename(&entry.title);
        let path = knowledge_dir.join(format!("{filename}.md"));
        let full = format!(
            "---\nname: {}\ndescription: {}\nconfidence: INFERRED\n---\n{}\n---\n",
            entry.title, entry.title, entry.content
        );
        tokio::fs::write(&path, &full)
            .await
            .map_err(|e| CarrierError::Internal(format!("Failed to write {}: {e}", filename)))?;
        saved.push(filename);
    }
    Ok((saved, result.quality))
}

/// 凭证形状的内容不进共享知识库（add 与 update 同一闸）：私数据走 kv。
fn reject_credential_like(tool: &str, content: &str) -> CarrierResult<()> {
    let content_lower = content.to_lowercase();
    let sensitive_patterns = [
        "app_secret",
        "app_id",
        "api_key",
        "apikey",
        "secret_key",
        "access_token",
        "private_key",
    ];
    if let Some(pattern) = sensitive_patterns
        .iter()
        .find(|p| content_lower.contains(*p))
    {
        return Err(CarrierError::InvalidInput(format!(
            "Rejected: content contains '{pattern}' which looks like credentials/secrets. \
             Use kv_set to store private data in your personal key-value store instead of {tool}."
        )));
    }
    Ok(())
}

pub async fn knowledge_add(input: &Value, workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_add requires a workspace root".to_string(),
    ))?;
    let title = input["title"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'title' parameter".to_string(),
        ))?;
    let content = input["content"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'content' parameter".to_string(),
        ))?;

    reject_credential_like("knowledge_add", content)?;

    let filename = knowledge_add_core(root, title, content, "tool").await?;
    Ok(format!("Knowledge added: {filename}.md"))
}

/// In-place update of an existing knowledge file. Fuzzy-matches the target
/// (same matcher as knowledge_remove), validates the replacement keeps a
/// frontmatter block, rejects credential-looking content (same patterns as
/// knowledge_add), and records a "update" version entry with before/after so
/// self-evolution stays auditable.
pub async fn knowledge_update(
    input: &Value,
    workspace_root: Option<&Path>,
) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_update requires a workspace root".to_string(),
    ))?;
    let filename = input["filename"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'filename' parameter".to_string(),
        ))?;
    let content = input["content"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'content' parameter".to_string(),
        ))?;

    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err(CarrierError::InvalidInput(
            "Invalid filename: path separators and '..' are forbidden".to_string(),
        ));
    }

    // The replacement must be a complete knowledge file: frontmatter fence at
    // the top plus a closing fence. Structured error so the agent can self-heal.
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") || trimmed[3..].find("\n---").is_none() {
        return Err(CarrierError::InvalidInput(
            "content 必须是完整知识文件：以 --- frontmatter 开头（name/description 等），并有闭合的 ---。先 knowledge_read 原文件，在原内容上修改后整体传回。".to_string()
        ));
    }

    reject_credential_like("knowledge_update", content)?;

    let knowledge_dir = root.join("knowledge");
    let target = find_knowledge_file(&knowledge_dir, filename)?;
    let name = target
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let before = tokio::fs::read_to_string(&target)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read existing file: {e}")))?;

    tokio::fs::write(&target, content)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to write knowledge file: {e}")))?;

    let _ = lifecycle::version::record_version(
        root,
        "update",
        &name,
        Some(before.as_str()),
        Some(content),
        "tool",
    );
    let _ = lifecycle::evolution::update_memory_index(root);

    Ok(format!(
        "Knowledge updated in place: {name} (version recorded, index rebuilt)"
    ))
}

pub async fn knowledge_remove(
    input: &Value,
    workspace_root: Option<&Path>,
) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_remove requires a workspace root".to_string(),
    ))?;
    let query = input["filename"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'filename' parameter".to_string(),
        ))?;
    let knowledge_dir = root.join("knowledge");
    let target = find_knowledge_file(&knowledge_dir, query)?;
    let before = tokio::fs::read_to_string(&target).await.ok();
    tokio::fs::remove_file(&target)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to delete: {e}")))?;
    let name = target
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let _ = lifecycle::version::record_version(
        root,
        "delete",
        &name,
        before.as_deref(),
        None,
        "tool",
    );
    let _ = lifecycle::evolution::update_memory_index(root);
    Ok(format!("Knowledge removed: {name}"))
}

pub async fn knowledge_import(
    input: &Value,
    workspace_root: Option<&Path>,
) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_import requires a workspace root".to_string(),
    ))?;
    let data = input["data"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'data' parameter".to_string(),
        ))?;
    let data_type = input["data_type"].as_str().unwrap_or("auto");
    let (saved, quality) = knowledge_import_core(root, data, data_type).await?;
    Ok(format!(
        "Imported {} entries as knowledge files. Quality: {:?}",
        saved.len(),
        quality
    ))
}

pub async fn knowledge_extract(
    input: &Value,
    workspace_root: Option<&Path>,
) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_extract requires a workspace root".to_string(),
    ))?;
    let title = input["title"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'title' parameter".to_string(),
        ))?;
    let content = input["content"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'content' parameter".to_string(),
        ))?;

    let candidate = lifecycle::evolution::KnowledgeCandidate {
        title: title.to_string(),
        content: content.to_string(),
        scope: "shared".to_string(),
    };
    let analysis = lifecycle::evolution::EvolutionAnalysis {
        knowledge: vec![candidate],
        gaps: vec![],
        trivial: false,
    };
    let saved = lifecycle::evolution::apply_evolution(root, &analysis, None, None, None);
    match saved.len() {
        0 => Ok("No knowledge extracted (nothing new to save).".to_string()),
        n => Ok(format!(
            "Extracted {n} knowledge item(s) and updated index."
        )),
    }
}

pub async fn knowledge_index(workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "knowledge_index requires a workspace root".to_string(),
    ))?;
    lifecycle::evolution::update_memory_index(root)
        .map_err(|e| CarrierError::Internal(format!("Failed to rebuild index: {e}")))?;
    Ok("Knowledge index (MEMORY.md) rebuilt successfully.".to_string())
}

// ---------------------------------------------------------------------------
// clone_evaluate
// ---------------------------------------------------------------------------

pub async fn clone_evaluate(workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "clone_evaluate requires a workspace root".to_string(),
    ))?;
    let metrics = lifecycle::evaluate::compute_deterministic_metrics(root);
    Ok(format!(
        "Quality Score: {}/100 ({})\nKnowledge: {} files, {} bytes\nSkills: {}\nIdentity: SOUL={}, SP={}, MEMORY={}",
        metrics.score,
        metrics.grade,
        metrics.knowledge_files,
        metrics.knowledge_total_bytes,
        metrics.flow_count,
        metrics.has_soul,
        metrics.has_system_prompt,
        metrics.has_memory,
    ))
}

// ---------------------------------------------------------------------------
// Flows
// ---------------------------------------------------------------------------

fn parse_string_list(input: &Value, key: &str) -> Vec<String> {
    input[key]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Format a YAML frontmatter `tools:` list block.
fn format_tools_yaml(tools: &[String]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let mut out = String::from("tools:\n");
    for t in tools {
        out.push_str(&format!("  - {t}\n"));
    }
    out
}

/// Split a flow file into (frontmatter_inner, body). frontmatter_inner excludes the `---` fences.
fn split_flow_file(content: &str) -> (Option<String>, String) {
    let trimmed = content.trim_start();
    if let Some(rest) = trimmed.strip_prefix("---") {
        let rest = rest.strip_prefix('\n').unwrap_or(rest);
        if let Some(end) = rest.find("\n---") {
            let fm = rest[..end].to_string();
            let after = &rest[end + 4..]; // skip \n---
            let body = after.strip_prefix('\n').unwrap_or(after).to_string();
            return (Some(fm), body);
        }
    }
    (None, content.to_string())
}

/// Replace or insert `tools:` in YAML frontmatter text (without fences).
fn upsert_frontmatter_tools(fm: &str, tools: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut skipping_tools_list = false;
    let mut tools_written = false;
    for line in fm.lines() {
        let trimmed = line.trim();
        if skipping_tools_list {
            // Continue skipping multi-line list items under tools:
            if trimmed.starts_with('-') || trimmed.is_empty() {
                continue;
            }
            // Also skip inline tools: [...] was a single line already consumed
            skipping_tools_list = false;
        }
        if trimmed.starts_with("tools:") {
            if !tools_written {
                lines.push(format_tools_yaml(tools).trim_end().to_string());
                tools_written = true;
            }
            // Skip old tools value: either same-line list or following `-` lines
            if trimmed == "tools:" || trimmed.ends_with(':') {
                skipping_tools_list = true;
            }
            continue;
        }
        // Drop deprecated toolsets so tools: is the single source of truth
        if trimmed.starts_with("toolsets:") {
            if trimmed == "toolsets:" || trimmed.ends_with(':') && !trimmed.contains('[') {
                skipping_tools_list = true;
            }
            continue;
        }
        lines.push(line.to_string());
    }
    if !tools_written && !tools.is_empty() {
        lines.push(format_tools_yaml(tools).trim_end().to_string());
    }
    lines.join("\n")
}

fn upsert_frontmatter_description(fm: &str, description: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut written = false;
    for line in fm.lines() {
        if line.trim().starts_with("description:") {
            lines.push(format!("description: {description}"));
            written = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !written {
        lines.push(format!("description: {description}"));
    }
    lines.join("\n")
}

pub async fn flow_create(input: &Value, workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "flow_create requires a workspace root".to_string(),
    ))?;
    let name = input["name"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'name' parameter".to_string(),
        ))?;
    let description = input["description"].as_str().unwrap_or("");
    let body = input["body"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'body' parameter".to_string(),
        ))?;
    // Prefer `tools`; accept legacy `toolsets` as alias (same list of tool names).
    let mut tools = parse_string_list(input, "tools");
    if tools.is_empty() {
        tools = parse_string_list(input, "toolsets");
    }

    let flows_dir = root.join("flows");
    tokio::fs::create_dir_all(&flows_dir)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to create flows dir: {e}")))?;

    let filename = lifecycle::evolution::sanitize_filename(name);
    let path = flows_dir.join(format!("{filename}.md"));

    if path.exists() {
        return Err(CarrierError::InvalidInput(format!(
            "Flow '{name}' already exists. Use flow_update to modify it."
        )));
    }

    let mut frontmatter = format!("---\nname: {name}\n");
    if !description.is_empty() {
        frontmatter.push_str(&format!("description: {description}\n"));
    }
    if !tools.is_empty() {
        frontmatter.push_str(&format_tools_yaml(&tools));
    }
    frontmatter.push_str("---\n");

    let full = format!("{frontmatter}\n{body}");
    tokio::fs::write(&path, &full)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to write flow: {e}")))?;

    Ok(format!(
        "Flow '{name}' created successfully{}.",
        if tools.is_empty() {
            String::new()
        } else {
            format!(" with tools: [{}]", tools.join(", "))
        }
    ))
}

pub async fn flow_update(input: &Value, workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "flow_update requires a workspace root".to_string(),
    ))?;
    let name = input["name"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'name' parameter".to_string(),
        ))?;
    let new_body = input["body"].as_str().filter(|s| !s.is_empty());
    let new_tools = {
        let t = parse_string_list(input, "tools");
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };
    let new_description = input["description"].as_str().filter(|s| !s.is_empty());

    if new_body.is_none() && new_tools.is_none() && new_description.is_none() {
        return Err(CarrierError::InvalidInput(
            "flow_update requires at least one of: body, tools, description (non-empty)"
                .to_string(),
        ));
    }

    // Only workspace flows are updateable — system-shared flows are abolished
    // ("全进分身"), every flow lives in the clone's workspace/flows/.
    let private_flows = root.join("flows");
    let private_path = find_flow_path(&private_flows, name).await;

    let source_path = match &private_path {
        Some(p) => p.clone(),
        None => {
            return Err(CarrierError::InvalidInput(format!(
                "Flow '{name}' not found in workspace flows."
            )));
        }
    };

    let existing = tokio::fs::read_to_string(&source_path)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read flow: {e}")))?;

    let (fm_opt, old_body) = split_flow_file(&existing);
    let mut fm = fm_opt.unwrap_or_else(|| format!("name: {name}"));
    if let Some(desc) = new_description {
        fm = upsert_frontmatter_description(&fm, desc);
    }
    if let Some(ref tools) = new_tools {
        fm = upsert_frontmatter_tools(&fm, tools);
    }
    let body = new_body.unwrap_or(old_body.as_str());
    let updated = format!("---\n{}\n---\n\n{}", fm.trim(), body.trim_start());

    // Write in place at the private path (shared flows are refused above, so
    // source_path is always a private workspace flow).
    let target = source_path;

    tokio::fs::write(&target, &updated)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to write flow: {e}")))?;

    let mut notes = Vec::new();
    if let Some(ref tools) = new_tools {
        notes.push(format!("tools=[{}]", tools.join(", ")));
    }
    if new_body.is_some() {
        notes.push("body updated".to_string());
    }
    if new_description.is_some() {
        notes.push("description updated".to_string());
    }

    Ok(format!(
        "Flow '{name}' updated successfully ({}).",
        notes.join("; ")
    ))
}

pub async fn flow_load(input: &Value, workspace_root: Option<&Path>) -> CarrierResult<String> {
    let root = workspace_root.ok_or(CarrierError::Internal(
        "flow_load requires a workspace root".to_string(),
    ))?;
    let name = input["name"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'name' parameter".to_string(),
        ))?;

    // Only the clone's own workspace flows are loadable — system-shared flows
    // (~/.aginx/carrier/flows/) are no longer scanned ("全进分身").
    let dirs = [root.join("flows")];
    for flows_dir in dirs {
        if let Some(path) = find_flow_path(&flows_dir, name).await {
            return tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| CarrierError::Internal(format!("Failed to read flow: {e}")));
        }
    }

    Err(CarrierError::InvalidInput(format!(
        "Flow '{name}' not found."
    )))
}

/// Locate a flow file by name within a flows directory.
///
/// Tries exact flat (`{name}.md`), exact directory (`{name}/flow.md`, falling
/// back to legacy `{name}/SKILL.md`), then a case-insensitive fuzzy match on
/// entry names. Returns the path if found.
async fn find_flow_path(flows_dir: &Path, name: &str) -> Option<PathBuf> {
    if !flows_dir.is_dir() {
        return None;
    }
    let filename = lifecycle::evolution::sanitize_filename(name);
    let flat_path = flows_dir.join(format!("{filename}.md"));
    if flat_path.exists() {
        return Some(flat_path);
    }
    let dir = flows_dir.join(&filename);
    let dir_flow = dir.join("flow.md");
    if dir_flow.exists() {
        return Some(dir_flow);
    }
    let dir_skill = dir.join("SKILL.md");
    if dir_skill.exists() {
        return Some(dir_skill);
    }

    // Fuzzy match on entry names
    let mut entries = tokio::fs::read_dir(flows_dir).await.ok()?;
    while let Some(entry) = entries.next_entry().await.ok()? {
        let entry_name = entry.file_name().to_string_lossy().to_string();
        if !entry_name.to_lowercase().contains(&name.to_lowercase()) {
            continue;
        }
        if entry_name.ends_with(".md") {
            return Some(entry.path());
        }
        if entry.path().is_dir() {
            let flow_md = entry.path().join("flow.md");
            if flow_md.exists() {
                return Some(flow_md);
            }
            let skill_md = entry.path().join("SKILL.md");
            if skill_md.exists() {
                return Some(skill_md);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Fuzzy knowledge-file matcher
// ---------------------------------------------------------------------------

/// Fuzzy-match a knowledge file by name (exact -> prefix -> substring).
fn find_knowledge_file(knowledge_dir: &Path, query: &str) -> CarrierResult<PathBuf> {
    let entries = std::fs::read_dir(knowledge_dir)?;
    let query_lower = query.to_lowercase();
    let query_no_ext = query_lower.trim_end_matches(".md");

    let candidates: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
        .map(|e| e.path())
        .collect();

    // Exact match
    if let Some(exact) = candidates.iter().find(|p| {
        p.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            == query_lower
            || p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                == format!("{query_no_ext}.md")
    }) {
        return Ok(exact.clone());
    }

    // Prefix match
    if let Some(prefix) = candidates.iter().find(|p| {
        p.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            .starts_with(query_no_ext)
    }) {
        return Ok(prefix.clone());
    }

    // Substring match
    if let Some(sub) = candidates.iter().find(|p| {
        p.file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
            .contains(query_no_ext)
    }) {
        return Ok(sub.clone());
    }

    Err(CarrierError::InvalidInput(format!(
        "No knowledge file matching '{}' found",
        query
    )))
}

#[cfg(test)]
mod knowledge_update_tests {
    use super::*;
    use serde_json::json;

    fn input(filename: &str, content: &str) -> Value {
        json!({ "filename": filename, "content": content })
    }

    #[tokio::test]
    async fn update_replaces_in_place_with_fuzzy_match() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let saved = knowledge_add_core(root, "business-info", "old body", "test")
            .await
            .unwrap();
        assert_eq!(saved, "business-info");

        let new_content = "---\nname: business-info\ndescription: 86巴士业务\n---\nnew body\n";
        let out = knowledge_update(&input("business", new_content), Some(root))
            .await
            .unwrap();
        assert!(out.contains("business-info.md"), "{out}");
        // Same path, content replaced in place
        let on_disk = std::fs::read_to_string(root.join("knowledge/business-info.md")).unwrap();
        assert!(on_disk.contains("new body"));
        assert!(!on_disk.contains("old body"));
    }

    #[tokio::test]
    async fn update_rejects_missing_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        knowledge_add_core(root, "policy", "old", "test")
            .await
            .unwrap();
        let err = knowledge_update(&input("policy.md", "just body, no frontmatter"), Some(root))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("frontmatter"), "{err}");
    }

    #[tokio::test]
    async fn update_rejects_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        knowledge_add_core(root, "policy", "old", "test")
            .await
            .unwrap();
        let bad = "---\nname: p\n---\napp_secret = wx123\n";
        let err = knowledge_update(&input("policy.md", bad), Some(root))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("app_secret"), "{err}");
    }

    #[tokio::test]
    async fn add_rejects_credentials() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let err = knowledge_add(
            &json!({"title": "creds", "content": "api_key: sk-123"}),
            Some(root),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("api_key"), "{err}");
    }

    #[tokio::test]
    async fn update_unknown_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("knowledge")).unwrap();
        let err = knowledge_update(&input("nope.md", "---\nname: n\n---\nbody\n"), Some(root))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("No knowledge file"), "{err}");
    }

    #[tokio::test]
    async fn update_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let err = knowledge_update(&input("../evil.md", "---\nname: e\n---\nx\n"), Some(root))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("forbidden"), "{err}");
    }

    #[tokio::test]
    async fn read_rejects_path_traversal_and_lists_available() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        knowledge_add_core(root, "alpha", "body", "test").await.unwrap();
        let err = knowledge_read(&json!({"filename": "../evil.md"}), Some(root))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("forbidden"), "{err}");
        // 未知文件给出可用清单（LLM 自纠错锚点）
        let out = knowledge_read(&json!({"filename": "nope.md"}), Some(root))
            .await
            .unwrap();
        assert!(out.contains("alpha.md"), "{out}");
    }

    #[tokio::test]
    async fn flow_create_update_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let out = flow_create(
            &json!({"name": "writer", "description": "写文章", "tools": ["file_read"], "body": "# 步骤\n1. 读"}),
            Some(root),
        )
        .await
        .unwrap();
        assert!(out.contains("file_read"), "{out}");

        // 重名拒绝
        let err = flow_create(&json!({"name": "writer", "body": "x"}), Some(root))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");

        // 更新 tools 走 upsert，body 缺省保旧
        flow_update(
            &json!({"name": "writer", "tools": ["file_read", "kv_get"]}),
            Some(root),
        )
        .await
        .unwrap();
        let content = flow_load(&json!({"name": "writer"}), Some(root))
            .await
            .unwrap();
        assert!(content.contains("- kv_get"), "{content}");
        assert!(content.contains("# 步骤"), "{content}");

        // 未知 flow
        let err = flow_load(&json!({"name": "nope"}), Some(root))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
    }

    #[tokio::test]
    async fn remove_is_versioned() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        knowledge_add_core(root, "goner", "body", "test").await.unwrap();
        let out = knowledge_remove(&json!({"filename": "goner"}), Some(root))
            .await
            .unwrap();
        assert!(out.contains("Knowledge removed"), "{out}");
        assert!(!root.join("knowledge/goner.md").exists());
    }
}

#[cfg(test)]
mod flow_evolution_tests {
    use super::*;

    #[test]
    fn upsert_tools_replaces_inline_list() {
        let fm = "name: article-writer\ntools: [\"file_read\"]\nversion: 7\n";
        let out = upsert_frontmatter_tools(fm, &["file_read".into(), "file_write".into()]);
        assert!(out.contains("file_write"));
        assert!(out.contains("file_read"));
        assert!(out.contains("version: 7"));
        assert!(!out.contains("tools: [\"file_read\"]"));
    }

    #[test]
    fn upsert_tools_replaces_multiline_list() {
        let fm = "name: x\ntools:\n  - file_read\n  - old_tool\nversion: 1\n";
        let out = upsert_frontmatter_tools(fm, &["file_write".into()]);
        assert!(out.contains("- file_write"));
        assert!(!out.contains("old_tool"));
        assert!(out.contains("version: 1"));
    }

    #[test]
    fn split_flow_preserves_body() {
        let content = "---\nname: x\n---\n\n# Body\n\nstep 1\n";
        let (fm, body) = split_flow_file(content);
        assert!(fm.unwrap().contains("name: x"));
        assert!(body.contains("# Body"));
    }
}
