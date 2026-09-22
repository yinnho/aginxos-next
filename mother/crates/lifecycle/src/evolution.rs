//! Conversation evolution — auto-extract knowledge from conversations.
//!
//! Ported from openclone-core/src/evolution.rs, refactored to remove LLM dependency.
//! The kernel calls `build_analysis_prompt()` → sends to LLM → `parse_analysis_response()`
//! → `apply_evolution()` to write knowledge files.
//!
//! Flow:
//! 1. `should_skip()` — local filter, zero cost
//! 2. Kernel calls LLM with `build_analysis_prompt()`
//! 3. `parse_analysis_response()` — extract structured knowledge from JSON
//! 4. `apply_evolution()` — write knowledge files + update MEMORY.md + record versions

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

/// Trivial inputs that are never worth analyzing.
const TRIVIAL_INPUTS: &[&str] = &[
    "ok",
    "好的",
    "嗯",
    "继续",
    "谢谢",
    "感谢",
    "对",
    "是的",
    "是的",
    "可以",
    "明白",
    "知道了",
    "了解",
    "没问题",
    "好",
    "行",
    "嗯嗯",
    "哈哈",
    "哈哈",
    "👍",
    "👌",
    "是的",
    "right",
    "yes",
    "thanks",
    "继续说",
    "然后呢",
    "还有吗",
    "exit",
    "quit",
    "退出",
];

/// A single knowledge candidate extracted from a conversation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeCandidate {
    /// Short title (used as filename, English or pinyin preferred).
    pub title: String,
    /// Full knowledge content.
    pub content: String,
    /// Scope: "shared" (any user benefits) or "private" (user-specific).
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_scope() -> String {
    "shared".to_string()
}

/// Result of analyzing a conversation turn.
#[derive(Debug, Clone)]
pub struct EvolutionAnalysis {
    /// Extracted knowledge candidates.
    pub knowledge: Vec<KnowledgeCandidate>,
    /// Knowledge gaps discovered.
    pub gaps: Vec<String>,
    /// Whether the conversation was trivial / not worth analyzing.
    pub trivial: bool,
}

/// Check if a conversation turn should be skipped (pure local check, zero cost).
pub fn should_skip(user_msg: &str, response: &str) -> bool {
    // Response too short
    if response.len() < 100 {
        return true;
    }
    // Input is trivial
    let trimmed = user_msg.trim().to_lowercase();
    if trimmed.is_empty() || TRIVIAL_INPUTS.contains(&trimmed.as_str()) {
        return true;
    }
    // Input too short (< 4 chars)
    if trimmed.chars().count() < 4 {
        return true;
    }
    false
}

/// Build the system prompt for the LLM analysis call.
///
/// Returns a prompt that instructs the LLM to analyze the conversation
/// and extract new knowledge as JSON.
pub fn build_analysis_prompt() -> String {
    r#"你是知识提取助手。分析这段对话，判断是否产生了值得保存的新知识。

返回 JSON：
{
  "has_new_knowledge": true/false,
  "knowledge": [
    {"title": "简短标题（英文或拼音，用作文件名）", "content": "知识内容（保留原文关键信息）", "scope": "shared或private"}
  ],
  "gaps": ["发现的知识缺口（分身应该知道但不知道的东西）"]
}

判断标准：
1. has_new_knowledge=true：对话中包含已知索引中没有的事实、规则、流程或偏好
2. knowledge：每条知识独立成条，标题简短能作文件名
3. gaps：对话中暴露的分身知识盲区
4. 不要提取：问候语、闲聊、已存在于索引中的内容
5. 知识内容要完整准确，保留关键细节
6. 如果没有新知识，返回 {"has_new_knowledge": false, "knowledge": [], "gaps": []}
7. 只返回 JSON，不要其他文字

scope 判断规则：
- shared：通用知识，任何用户都受益（技术规范、行业知识、工具用法、业务规则）
- private：用户个人数据（偏好、经历、需求、私密信息、用户特定上下文）
- 拿不准时标 private"#
        .to_string()
}

/// Parse the LLM analysis response into structured data.
pub fn parse_analysis_response(text: &str) -> Result<EvolutionAnalysis> {
    let json_text = extract_json(text);

    #[derive(Debug, serde::Deserialize)]
    struct AnalysisResponse {
        #[serde(default)]
        has_new_knowledge: Option<bool>,
        #[serde(default)]
        knowledge: Option<Vec<KnowledgeCandidate>>,
        #[serde(default)]
        gaps: Option<Vec<String>>,
    }

    match serde_json::from_str::<AnalysisResponse>(&json_text) {
        Ok(resp) => {
            let has_knowledge = resp.has_new_knowledge.unwrap_or(false);
            let knowledge = resp.knowledge.unwrap_or_default();
            let gaps = resp.gaps.unwrap_or_default();

            if !has_knowledge && knowledge.is_empty() {
                return Ok(EvolutionAnalysis {
                    knowledge: vec![],
                    gaps,
                    trivial: true,
                });
            }

            Ok(EvolutionAnalysis {
                knowledge,
                gaps,
                trivial: false,
            })
        }
        Err(e) => {
            tracing::warn!("Evolution JSON parse failed: {}", e);
            Ok(EvolutionAnalysis {
                knowledge: vec![],
                gaps: vec![],
                trivial: true,
            })
        }
    }
}

/// Apply evolution results: write knowledge files, update MEMORY.md, record versions.
///
/// When `sender_id` and `home_dir` are provided, knowledge with scope="private"
/// is written to the sender's private directory instead of the shared workspace.
///
/// Returns paths of newly created knowledge files.
pub fn apply_evolution(
    workspace: &Path,
    analysis: &EvolutionAnalysis,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
    home_dir: Option<&Path>,
) -> Vec<PathBuf> {
    let mut saved = Vec::new();

    // Write new knowledge files
    for candidate in &analysis.knowledge {
        let is_private = candidate.scope == "private";
        let target_workspace = if is_private {
            if let (Some(oid), Some(hd)) = (owner_id.or(sender_id), home_dir) {
                Some(carrier_types::config::sender_data_dir(
                    hd,
                    oid,
                    &extract_agent_name(workspace),
                    sender_id,
                ))
            } else {
                None
            }
        } else {
            None
        };

        let target = target_workspace.as_deref().unwrap_or(workspace);
        match write_knowledge(target, candidate) {
            Ok(path) => {
                info!(file = ?path, scope = %candidate.scope, "Evolution: knowledge updated");
                saved.push(path);
            }
            Err(e) => {
                tracing::warn!(error = %e, "Evolution: failed to write knowledge");
            }
        }
    }

    // Mark knowledge gaps in workspace MEMORY.md
    if !analysis.gaps.is_empty() {
        if let Err(e) = append_gaps_to_index(workspace, &analysis.gaps) {
            tracing::warn!(error = %e, "Evolution: failed to append gaps");
        }
        for gap in &analysis.gaps {
            info!(gap = %gap, "Evolution: knowledge gap");
        }
    }

    // Rebuild MEMORY.md index if we wrote new knowledge
    if !saved.is_empty() {
        if let Err(e) = update_memory_index(workspace) {
            tracing::warn!(error = %e, "Evolution: failed to update memory index");
        }
        // Also update private MEMORY.md if we wrote private knowledge
        if analysis.knowledge.iter().any(|k| k.scope == "private") {
            if let (Some(oid), Some(hd)) = (owner_id.or(sender_id), home_dir) {
                let sender_dir = carrier_types::config::sender_data_dir(
                    hd,
                    oid,
                    &extract_agent_name(workspace),
                    sender_id,
                );
                if let Err(e) = update_private_memory_index(&sender_dir) {
                    tracing::warn!(error = %e, "Evolution: failed to update private memory index");
                }
            }
        }
    }

    saved
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Write a knowledge candidate as a markdown file in knowledge/.
///
/// Uses dual-layer format:
/// ```md
/// ---
/// name: ...
/// ---
///
/// [compiled truth — current knowledge, rewritable by compile]
///
/// ---
///
/// - 2026-04-12: learned from conversation
/// ```
///
/// Evolution appends to the timeline (below the second `---`).
/// Compile rewrites the compiled truth (above the second `---`).
fn write_knowledge(workspace: &Path, candidate: &KnowledgeCandidate) -> Result<PathBuf> {
    let knowledge_dir = workspace.join("knowledge");
    fs::create_dir_all(&knowledge_dir)?;

    let safe_title = sanitize_filename(&candidate.title);
    let filename = if safe_title.is_empty() {
        format!("knowledge-{}.md", chrono::Utc::now().timestamp_millis())
    } else {
        format!("{}.md", safe_title)
    };
    let path = knowledge_dir.join(&filename);
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let content = if path.exists() {
        // Existing file — append to timeline section
        let before = fs::read_to_string(&path).unwrap_or_default();
        let (compiled_truth, mut timeline) = split_dual_layer(&before);

        // Add new entry to timeline
        timeline.push_str(&format!(
            "- {}: {} (from conversation)\n",
            date, candidate.title
        ));

        let updated = format!(
            "{}\n\n---\n\n{}",
            compiled_truth.trim_end(),
            timeline.trim_end()
        );

        crate::version::record_version(
            workspace,
            "update",
            &filename,
            Some(&before),
            Some(&updated),
            "evolution",
        )?;

        updated
    } else {
        // New file — create with compiled truth + initial timeline entry
        let content = format!(
            "---\nname: {}\nsource: evolution\ntype: knowledge\nconfidence: INFERRED\n---\n\n{}\n\n---\n\n- {}: created from conversation\n",
            candidate.title,
            candidate.content,
            date
        );

        crate::version::record_version(
            workspace,
            "create",
            &filename,
            None,
            Some(&content),
            "evolution",
        )?;

        content
    };

    fs::write(&path, &content)?;

    Ok(path)
}

/// Rebuild MEMORY.md by scanning knowledge/ directory.
/// Preserves V3 sections (## 技能, ## 身份文件) if they exist.
pub fn update_memory_index(workspace: &Path) -> Result<()> {
    let index_path = workspace.join("MEMORY.md");

    // Read existing MEMORY.md to extract preserved sections
    let existing = if index_path.exists() {
        fs::read_to_string(&index_path).unwrap_or_default()
    } else {
        String::new()
    };

    let preserved = extract_preserved_sections(&existing);

    let mut lines = vec!["# 知识索引".to_string(), String::new()];

    // Preserved: 技能 section (V3 format)
    if let Some(ref skills) = preserved.skills {
        lines.extend(skills.lines().map(|l| l.to_string()));
        lines.push(String::new());
    }

    // Preserved: 身份文件 section (V3 format)
    if let Some(ref identity) = preserved.identity_files {
        lines.extend(identity.lines().map(|l| l.to_string()));
        lines.push(String::new());
    }

    // Rebuilt: 知识 section (scanned from knowledge/)
    let knowledge_dir = workspace.join("knowledge");
    if knowledge_dir.exists() {
        lines.push("## 知识".to_string());
        lines.push(String::new());

        let entries = fs::read_dir(&knowledge_dir)?;
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
            .collect();

        files.sort_by_key(|e| e.file_name());

        for entry in files {
            let path = entry.path();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();

            // Try to extract title from frontmatter
            let (title, confidence) = if let Ok(file_content) = fs::read_to_string(&path) {
                let title = extract_frontmatter_name(&file_content).unwrap_or_else(|| name.clone());
                let conf = extract_confidence(&file_content);
                (title, conf)
            } else {
                (name.clone(), "EXTRACTED".to_string())
            };

            let label = match confidence.as_str() {
                "INFERRED" => format!("[INFERRED] {}", title),
                "AMBIGUOUS" => format!("[AMBIGUOUS] {}", title),
                _ => title, // EXTRACTED or unknown — no tag needed
            };
            lines.push(format!("- [{}](knowledge/{}.md)", label, name));
        }
    }

    // Check for existing gaps section
    if !existing.is_empty() {
        if let Some(gaps_start) = existing.find("## 知识缺口") {
            lines.push(String::new());
            // Preserve gaps section as-is
            for line in existing[gaps_start..].lines() {
                lines.push(line.to_string());
            }
        }
    }

    let content = lines.join("\n");
    fs::write(&index_path, content)?;
    Ok(())
}

/// Append knowledge gaps to MEMORY.md.
fn append_gaps_to_index(workspace: &Path, gaps: &[String]) -> Result<()> {
    let index_path = workspace.join("MEMORY.md");
    let mut content = fs::read_to_string(&index_path).unwrap_or_default();

    if !gaps.is_empty() {
        if !content.contains("## 知识缺口") {
            content.push_str("\n## 知识缺口\n\n");
        }
        for gap in gaps {
            content.push_str(&format!("- [待补充] {}\n", gap));
        }
        fs::write(index_path, content)?;
    }

    Ok(())
}

/// Extract agent name from workspace path (last directory component).
fn extract_agent_name(workspace: &Path) -> String {
    workspace
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Rebuild MEMORY.md for a sender's private knowledge directory.
pub fn update_private_memory_index(sender_dir: &Path) -> Result<()> {
    let index_path = sender_dir.join("MEMORY.md");

    let mut lines = vec![
        "# 私人知识索引".to_string(),
        String::new(),
        "> 此文件由系统自动维护，不要手动编辑。".to_string(),
        String::new(),
    ];

    let knowledge_dir = sender_dir.join("knowledge");
    if knowledge_dir.exists() {
        lines.push("## 私人知识".to_string());
        lines.push(String::new());

        let entries = fs::read_dir(&knowledge_dir)?;
        let mut files: Vec<_> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "md").unwrap_or(false))
            .collect();

        files.sort_by_key(|e| e.file_name());

        for entry in files {
            let path = entry.path();
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();

            let title = if let Ok(file_content) = fs::read_to_string(&path) {
                extract_frontmatter_name(&file_content).unwrap_or_else(|| name.clone())
            } else {
                name.clone()
            };

            lines.push(format!("- [{}](knowledge/{}.md)", title, name));
        }
    }

    let content = lines.join("\n");
    fs::write(&index_path, content)?;
    Ok(())
}

/// Split a dual-layer knowledge file into (compiled_truth, timeline).
///
/// Format (text, not code):
/// `---frontmatter---` ... `compiled truth` ... `---` ... `timeline entries`
///
/// Returns (compiled_truth_with_frontmatter, timeline_text).
/// If no second `---` separator found, the whole body is compiled truth
/// with an empty timeline (legacy format compat).
pub fn split_dual_layer(content: &str) -> (String, String) {
    let lines: Vec<&str> = content.lines().collect();

    // Find frontmatter end (first standalone ---)
    let fm_end = if lines.first().map(|l| l.trim()) == Some("---") {
        lines
            .iter()
            .position(|l| l.trim() == "---")
            .and_then(|start| {
                lines[start + 1..]
                    .iter()
                    .position(|l| l.trim() == "---")
                    .map(|end| start + 1 + end)
            })
    } else {
        None
    };

    // Search for dual-layer separator after frontmatter
    // It's a standalone --- line preceded by an empty line
    let search_start = fm_end.map(|i| i + 1).unwrap_or(0);
    let mut separator_line = None;
    for i in (search_start + 1)..lines.len() {
        if lines[i].trim() == "---" && i > 0 && lines[i - 1].trim().is_empty() {
            separator_line = Some(i);
            break;
        }
    }

    match separator_line {
        Some(sep_idx) => {
            let compiled = lines[..sep_idx].join("\n").trim_end().to_string();
            let timeline = lines[sep_idx + 1..].join("\n").trim().to_string();
            (compiled, timeline)
        }
        None => {
            // No dual-layer separator — treat entire content as compiled truth
            (content.to_string(), String::new())
        }
    }
}

/// Extract `name` from YAML frontmatter (`---\nname: Foo\n---`).
fn extract_frontmatter_name(content: &str) -> Option<String> {
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

/// Extract confidence field from frontmatter. Defaults to EXTRACTED if absent.
fn extract_confidence(content: &str) -> String {
    let Some(rest) = content.strip_prefix("---") else {
        return "EXTRACTED".to_string();
    };
    let Some(end) = rest.find("---") else {
        return "EXTRACTED".to_string();
    };
    let frontmatter = &rest[..end];

    for line in frontmatter.lines() {
        if let Some(value) = line.strip_prefix("confidence:") {
            let value = value.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "EXTRACTED".to_string()
}

/// Extract JSON from text (handles markdown code blocks).
fn extract_json(text: &str) -> String {
    // Try to find JSON in code blocks first
    if let Some(start) = text.find("```json") {
        let json_start = start + 7;
        if let Some(end) = text[json_start..].find("```") {
            return text[json_start..json_start + end].trim().to_string();
        }
    }
    if let Some(start) = text.find("```") {
        let json_start = start + 3;
        if let Some(end) = text[json_start..].find("```") {
            return text[json_start..json_start + end].trim().to_string();
        }
    }
    // Try to find raw JSON object
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return text[start..=end].to_string();
        }
    }
    text.to_string()
}

/// Sanitize a string for use as a filename.
pub fn sanitize_filename(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else if c == ' ' {
                '-'
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .trim_matches('_')
        .to_string()
}

/// Preserved sections from existing MEMORY.md that should survive rebuild.
struct PreservedSections {
    skills: Option<String>,
    identity_files: Option<String>,
}

/// Extract V3 preserved sections (## 技能, ## 身份文件) from existing MEMORY.md.
fn extract_preserved_sections(existing: &str) -> PreservedSections {
    let mut skills: Option<String> = None;
    let mut identity_files: Option<String> = None;

    // Section headers to look for
    let section_headers = ["## 技能", "## 身份文件", "## 知识", "## 知识缺口"];

    for (i, line) in existing.lines().enumerate() {
        if line.starts_with("## 技能") {
            skills = Some(extract_section(existing, i, &section_headers));
        } else if line.starts_with("## 身份文件") {
            identity_files = Some(extract_section(existing, i, &section_headers));
        }
    }

    PreservedSections {
        skills,
        identity_files,
    }
}

/// Extract a section from line index until the next section header or EOF.
fn extract_section(text: &str, start_line: usize, stop_headers: &[&str]) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut section_lines = Vec::new();

    for line in lines.iter().skip(start_line) {
        // Stop if we hit another section header
        if stop_headers.iter().any(|h| line.starts_with(h)) && !section_lines.is_empty() {
            break;
        }
        section_lines.push(*line);
    }

    section_lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_should_skip_short_response() {
        assert!(should_skip("tell me about X", "ok"));
    }

    #[test]
    fn test_should_skip_trivial_input() {
        assert!(should_skip("谢谢", "这是一段足够长的回复内容，超过一百个字符以确保不会因为长度被跳过。这是一段足够长的回复内容。"));
    }

    #[test]
    fn test_should_skip_too_short_input() {
        assert!(should_skip("abc", "这是一段足够长的回复内容，超过一百个字符以确保不会因为长度被跳过。这是一段足够长的回复内容。"));
    }

    #[test]
    fn test_should_not_skip_valid() {
        assert!(!should_skip(
            "请介绍一下退款政策",
            "我们的退款政策如下：购买后7天内可以无条件退款，超过7天需提供退款理由。退款将在3个工作日内处理完成。"
        ));
    }

    #[test]
    fn test_parse_analysis_response_with_knowledge() {
        let json = r#"{"has_new_knowledge": true, "knowledge": [{"title": "refund-policy", "content": "7天内可退款"}], "gaps": ["退货流程不明确"]}"#;
        let result = parse_analysis_response(json).unwrap();
        assert!(!result.trivial);
        assert_eq!(result.knowledge.len(), 1);
        assert_eq!(result.knowledge[0].title, "refund-policy");
        assert_eq!(result.gaps.len(), 1);
    }

    #[test]
    fn test_parse_analysis_response_no_knowledge() {
        let json = r#"{"has_new_knowledge": false, "knowledge": [], "gaps": []}"#;
        let result = parse_analysis_response(json).unwrap();
        assert!(result.trivial);
        assert!(result.knowledge.is_empty());
    }

    #[test]
    fn test_parse_analysis_response_invalid_json() {
        let result = parse_analysis_response("not json at all").unwrap();
        assert!(result.trivial);
        assert!(result.knowledge.is_empty());
    }

    #[test]
    fn test_parse_analysis_response_in_markdown() {
        let text = "```json\n{\"has_new_knowledge\": true, \"knowledge\": [{\"title\": \"test\", \"content\": \"test content\"}], \"gaps\": []}\n```";
        let result = parse_analysis_response(text).unwrap();
        assert!(!result.trivial);
        assert_eq!(result.knowledge.len(), 1);
    }

    #[test]
    fn test_apply_evolution_writes_files() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        fs::create_dir_all(workspace.join("knowledge")).unwrap();

        let analysis = EvolutionAnalysis {
            knowledge: vec![KnowledgeCandidate {
                title: "refund-policy".to_string(),
                content: "7天内可退款".to_string(),
                scope: "shared".to_string(),
            }],
            gaps: vec!["退货流程".to_string()],
            trivial: false,
        };

        let saved = apply_evolution(workspace, &analysis, None, None, None);
        assert_eq!(saved.len(), 1);

        // Knowledge file created with dual-layer format
        let path = workspace.join("knowledge/refund-policy.md");
        assert!(path.exists());
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("7天内可退款"));
        assert!(content.contains("created from conversation"));
        // Verify dual-layer separator present
        let (compiled, timeline) = split_dual_layer(&content);
        assert!(compiled.contains("7天内可退款"));
        assert!(timeline.contains("created from conversation"));

        // MEMORY.md updated
        let memory = fs::read_to_string(workspace.join("MEMORY.md")).unwrap();
        assert!(memory.contains("refund-policy"));
        assert!(memory.contains("退货流程"));

        // Version recorded
        let versions = crate::version::get_all_versions(workspace).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].action, "create");
    }

    #[test]
    fn test_apply_evolution_appends_to_existing() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        fs::create_dir_all(workspace.join("knowledge")).unwrap();

        let candidate = KnowledgeCandidate {
            title: "test-knowledge".to_string(),
            content: "original".to_string(),
            scope: "shared".to_string(),
        };

        // First write — creates file
        write_knowledge(workspace, &candidate).unwrap();
        assert!(workspace.join("knowledge/test-knowledge.md").exists());

        // Second write — appends instead of skipping
        let analysis = EvolutionAnalysis {
            knowledge: vec![KnowledgeCandidate {
                title: "test-knowledge".to_string(),
                content: "updated info".to_string(),
                scope: "shared".to_string(),
            }],
            gaps: vec![],
            trivial: false,
        };
        let saved = apply_evolution(workspace, &analysis, None, None, None);
        assert_eq!(saved.len(), 1, "should append, not skip");

        // File should have dual-layer format: compiled truth preserved + timeline appended
        let content = fs::read_to_string(workspace.join("knowledge/test-knowledge.md")).unwrap();
        let (compiled, timeline) = split_dual_layer(&content);
        assert!(
            compiled.contains("original"),
            "compiled truth should preserve original"
        );
        assert!(timeline.contains("created from conversation"));
        assert!(
            timeline.contains("from conversation"),
            "timeline should have appended entry"
        );

        // Two version records: create + update
        let versions = crate::version::get_all_versions(workspace).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].action, "create");
        assert_eq!(versions[1].action, "update");
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("Refund Policy"), "Refund-Policy");
        assert_eq!(sanitize_filename("退款政策"), "退款政策");
        assert_eq!(sanitize_filename("test-knowledge"), "test-knowledge");
        assert_eq!(sanitize_filename("hello world!"), "hello-world");
    }

    #[test]
    fn test_extract_json_from_markdown() {
        let text = "Here is the analysis:\n```json\n{\"key\": \"value\"}\n```\nDone.";
        assert_eq!(extract_json(text), "{\"key\": \"value\"}");
    }

    #[test]
    fn test_extract_frontmatter_name() {
        let content = "---\nname: Test Knowledge\nsource: evolution\n---\n\nSome content";
        assert_eq!(
            extract_frontmatter_name(content),
            Some("Test Knowledge".to_string())
        );
    }

    #[test]
    fn test_split_dual_layer() {
        let content = "---\nname: test\n---\n\nCompiled truth here.\n\n---\n\n- 2026-04-12: created\n- 2026-04-13: updated";
        let (compiled, timeline) = split_dual_layer(content);
        assert!(compiled.contains("Compiled truth here."));
        assert!(compiled.contains("name: test"));
        assert!(timeline.contains("created"));
        assert!(timeline.contains("updated"));
    }

    #[test]
    fn test_split_dual_layer_legacy_no_separator() {
        let content = "---\nname: test\n---\n\nJust compiled truth, no timeline.";
        let (compiled, timeline) = split_dual_layer(content);
        assert!(compiled.contains("Just compiled truth"));
        assert!(timeline.is_empty());
    }
}
