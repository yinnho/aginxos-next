//! Feedback pipeline — anonymize knowledge and push back to Hub.
//!
//! When a clone (forked from a Hub template) learns new knowledge through
//! evolution, this module:
//! 1. Builds an LLM prompt to anonymize PII from knowledge
//! 2. Saves anonymized feedback to `feedback/*.json`
//! 3. Pushes feedback to the Hub for template authors to review
//!
//! The kernel calls `build_anonymize_prompt()` → sends to LLM →
//! `parse_anonymize_response()` → `save_feedback()` → `push_feedback_to_hub()`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A single anonymized feedback entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackEntry {
    /// Knowledge title (anonymized).
    pub title: String,
    /// Knowledge content (anonymized).
    pub content: String,
    /// Source template name.
    pub source_template: String,
    /// ISO 8601 timestamp.
    pub timestamp: String,
    /// Whether PII has been stripped.
    pub anonymized: bool,
}

/// Build the LLM system prompt + user prompt for anonymizing knowledge.
///
/// Returns (system_prompt, user_prompt). The kernel sends these to the LLM
/// and passes the response to `parse_anonymize_response()`.
pub fn build_anonymize_prompt(title: &str, content: &str) -> (String, String) {
    let system = r#"你是数据脱敏助手。将知识内容中的敏感信息替换为通用描述。

脱敏规则：
1. 人名 → "某人"、"用户"、"客户"
2. 电话号码、邮箱 → "[已脱敏]"
3. 公司名称 → "某公司"
4. 具体价格、金额 → "[具体数字]"
5. 地址 → "某地"
6. 保留知识的结构和事实逻辑

返回 JSON：
{
  "title": "脱敏后的标题",
  "content": "脱敏后的内容"
}

只返回 JSON，不要其他文字。"#
        .to_string();

    let user = format!("标题: {}\n\n内容:\n{}", title, content);
    (system, user)
}

/// Parse the LLM anonymization response.
///
/// Falls back to conservative anonymization (replace digits with X, truncate)
/// if JSON parsing fails.
pub fn parse_anonymize_response(text: &str) -> Result<(String, String)> {
    let json_text = extract_json(text);

    #[derive(Deserialize)]
    struct AnonymizeResult {
        title: String,
        content: String,
    }

    match serde_json::from_str::<AnonymizeResult>(&json_text) {
        Ok(result) => Ok((result.title, result.content)),
        Err(_) => {
            // Conservative fallback: replace digits with X, truncate
            let fallback_content: String = text
                .replace(char::is_numeric, "X")
                .chars()
                .take(200)
                .collect();
            Ok(("feedback-entry".to_string(), fallback_content))
        }
    }
}

/// Save anonymized feedback to `feedback/` directory.
pub fn save_feedback(
    workspace: &Path,
    source_template: &str,
    title: &str,
    content: &str,
) -> Result<PathBuf> {
    let feedback_dir = workspace.join("feedback");
    std::fs::create_dir_all(&feedback_dir)?;

    let now = chrono::Utc::now().timestamp();
    let safe_title: String = crate::evolution::sanitize_filename(&title.to_lowercase())
        .chars()
        .take(30)
        .collect();

    let filename = format!("{}-{}.json", now, safe_title);
    let path = feedback_dir.join(&filename);

    let entry = FeedbackEntry {
        title: title.to_string(),
        content: content.to_string(),
        source_template: source_template.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        anonymized: true,
    };

    let json = serde_json::to_string_pretty(&entry)?;
    std::fs::write(&path, json)?;

    Ok(path)
}

/// Collect all feedback entries from `feedback/` directory.
pub fn collect_feedback(workspace: &Path) -> Result<Vec<FeedbackEntry>> {
    let feedback_dir = workspace.join("feedback");
    if !feedback_dir.exists() {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&feedback_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "json").unwrap_or(false) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(feedback) = serde_json::from_str::<FeedbackEntry>(&content) {
                    entries.push(feedback);
                }
            }
        }
    }

    entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    Ok(entries)
}

/// Push feedback entries to the Hub.
///
/// Uses the carrier_clone::hub module for all HTTP calls.
/// Returns a list of results (success/failure messages).
pub async fn push_feedback_to_hub(
    hub_url: &str,
    api_key: &str,
    entries: &[FeedbackEntry],
) -> Result<Vec<String>> {
    let mut results = Vec::new();
    for entry in entries {
        match carrier_clone::hub::push_feedback(
            hub_url,
            api_key,
            &entry.source_template,
            &entry.title,
            &entry.content,
            Some(&entry.source_template),
        )
        .await
        {
            Ok(()) => results.push(format!("ok: {}", entry.title)),
            Err(e) => results.push(format!("fail: {} ({})", entry.title, e)),
        }
    }
    Ok(results)
}

/// Extract JSON object from text (handles markdown code blocks).
fn extract_json(text: &str) -> String {
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
    if let Some(start) = text.find('{') {
        if let Some(end) = text.rfind('}') {
            return text[start..=end].to_string();
        }
    }
    text.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_build_anonymize_prompt() {
        let (sys, user) = build_anonymize_prompt("退款政策", "张三的退款金额为500元");
        assert!(sys.contains("脱敏"));
        assert!(user.contains("张三"));
        assert!(user.contains("500"));
    }

    #[test]
    fn test_parse_anonymize_response_valid() {
        let json = r#"{"title": "某人的退款", "content": "退款金额为[具体数字]"}"#;
        let (title, content) = parse_anonymize_response(json).unwrap();
        assert_eq!(title, "某人的退款");
        assert_eq!(content, "退款金额为[具体数字]");
    }

    #[test]
    fn test_parse_anonymize_response_in_markdown() {
        let text = "```json\n{\"title\": \"test\", \"content\": \"anonymized\"}\n```";
        let (title, content) = parse_anonymize_response(text).unwrap();
        assert_eq!(title, "test");
        assert_eq!(content, "anonymized");
    }

    #[test]
    fn test_parse_anonymize_response_fallback() {
        let (title, content) = parse_anonymize_response("not json at all").unwrap();
        assert_eq!(title, "feedback-entry");
        assert!(!content.is_empty());
    }

    #[test]
    fn test_save_and_collect_feedback() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        let path = save_feedback(workspace, "test-template", "标题", "内容").unwrap();
        assert!(path.exists());

        let entries = collect_feedback(workspace).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "标题");
        assert_eq!(entries[0].source_template, "test-template");
        assert!(entries[0].anonymized);
    }

    #[test]
    fn test_collect_feedback_empty() {
        let tmp = TempDir::new().unwrap();
        let entries = collect_feedback(tmp.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_collect_feedback_sorted_by_timestamp() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        // Write entries with different timestamps
        let e1 = FeedbackEntry {
            title: "second".to_string(),
            content: "c".to_string(),
            source_template: "t".to_string(),
            timestamp: "2026-04-13T00:00:00Z".to_string(),
            anonymized: true,
        };
        let e2 = FeedbackEntry {
            title: "first".to_string(),
            content: "c".to_string(),
            source_template: "t".to_string(),
            timestamp: "2026-04-12T00:00:00Z".to_string(),
            anonymized: true,
        };
        let dir = workspace.join("feedback");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), serde_json::to_string(&e1).unwrap()).unwrap();
        std::fs::write(dir.join("b.json"), serde_json::to_string(&e2).unwrap()).unwrap();

        let entries = collect_feedback(workspace).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "first");
        assert_eq!(entries[1].title, "second");
    }
}
