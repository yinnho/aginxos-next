//! Data parsers — FAQ CSV/TSV, chat JSON, document paragraph splitting.
//!
//! Uses a tiered fallback approach:
//! - **Tier 1** (pure algorithm, always works): structured parsing — CSV/TSV split,
//!   JSON parse, paragraph split
//! - **Tier 2** (LLM-assisted): when Tier 1 produces poor results, the caller
//!   (kernel) can use `build_tier2_prompt()` to ask an LLM to extract knowledge
//!   from the raw data, then call `parse_tier2_response()` to get entries.
//!
//! The `ParseQuality` enum signals whether Tier 1 succeeded or needs upgrading.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// A parsed knowledge entry (title + content + source).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub title: String,
    pub content: String,
    pub source: Option<String>,
}

/// Quality indicator from Tier 1 parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum ParseQuality {
    /// Tier 1 succeeded — good structured results.
    Good,
    /// Tier 1 produced results but quality is uncertain (few entries, short content).
    Uncertain,
    /// Tier 1 fell back to raw content — definitely needs Tier 2 if available.
    Fallback,
}

/// Result of Tier 1 parsing with quality indicator.
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub entries: Vec<KnowledgeEntry>,
    pub quality: ParseQuality,
}

/// Parse imported data by type (Tier 1 — pure algorithm).
///
/// - `"faq"` / `"csv"` → CSV/TSV FAQ parsing
/// - `"chat"` / `"json"` → Chat JSON parsing
/// - `"document"` / other → Document paragraph parsing
///
/// Returns a `ParseResult` with quality indicator.
pub fn parse_import_data(content: &str, data_type: &str) -> Result<ParseResult> {
    match data_type {
        "faq" | "csv" => {
            let entries = parse_faq(content)?;
            let quality = assess_quality(&entries, content);
            Ok(ParseResult { entries, quality })
        }
        "chat" | "json" => {
            let entries = parse_chat(content)?;
            let quality = assess_quality(&entries, content);
            Ok(ParseResult { entries, quality })
        }
        _ => {
            let entries = parse_document(content)?;
            let quality = assess_quality(&entries, content);
            Ok(ParseResult { entries, quality })
        }
    }
}

/// Build the Tier 2 LLM prompt for extracting knowledge from raw data.
///
/// The kernel sends this to the LLM when Tier 1 quality is insufficient.
pub fn build_tier2_prompt(raw_content: &str, data_type: &str) -> (String, String) {
    let system = r#"你是数据解析助手。用户会提供一段原始数据，请从中提取有价值的知识点。

返回 JSON 数组：
[
  {"title": "简短标题", "content": "完整知识内容"}
]

规则：
1. 每条知识独立成条
2. 标题简短，能作文件名
3. 内容要完整准确，保留关键细节
4. 过滤掉无意义的闲聊和重复内容
5. 只返回 JSON，不要其他文字"#
        .to_string();

    let preview = if raw_content.len() > 6000 {
        format!(
            "{}...(共 {} 字符)",
            &raw_content[..raw_content.ceil_char_boundary(6000)],
            raw_content.len()
        )
    } else {
        raw_content.to_string()
    };

    let user = format!("数据类型: {}\n\n原始数据:\n{}", data_type, preview);
    (system, user)
}

/// Parse the Tier 2 LLM response into knowledge entries.
pub fn parse_tier2_response(text: &str) -> Result<Vec<KnowledgeEntry>> {
    let json_text = extract_json_array(text);

    let parsed: Vec<KnowledgeCandidate> = serde_json::from_str(&json_text)
        .map_err(|e| anyhow::anyhow!("Tier 2 JSON parse failed: {}", e))?;

    Ok(parsed
        .into_iter()
        .map(|c| KnowledgeEntry {
            title: c.title,
            content: c.content,
            source: Some("import-tier2".to_string()),
        })
        .collect())
}

/// Assess the quality of Tier 1 parsing results.
fn assess_quality(entries: &[KnowledgeEntry], raw_content: &str) -> ParseQuality {
    if entries.is_empty() {
        // No entries at all — if raw content is substantial, it's a fallback
        if raw_content.trim().len() > 50 {
            return ParseQuality::Fallback;
        }
        return ParseQuality::Good; // Empty input → empty output is correct
    }

    // Multiple structured entries with distinct titles = good signal
    if entries.len() >= 2 {
        let distinct_titles = entries
            .iter()
            .map(|e| &e.title)
            .collect::<std::collections::HashSet<_>>();
        if distinct_titles.len() == entries.len() {
            return ParseQuality::Good;
        }
    }

    // Check if we only got a single "dump" entry (the fallback behavior)
    if entries.len() == 1 {
        let entry = &entries[0];
        // If the single entry's content is most of the raw content, it's a raw dump
        let ratio = entry.content.len() as f64 / raw_content.len().max(1) as f64;
        if ratio > 0.8 {
            return ParseQuality::Fallback;
        }
    }

    ParseQuality::Good
}

/// Extract JSON array from text (handles markdown code blocks).
fn extract_json_array(text: &str) -> String {
    // Try code block first
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
    // Try raw JSON array
    if let Some(start) = text.find('[') {
        if let Some(end) = text.rfind(']') {
            return text[start..=end].to_string();
        }
    }
    text.to_string()
}

#[derive(Deserialize)]
struct KnowledgeCandidate {
    title: String,
    content: String,
}

/// Parse FAQ/CSV data (tab-separated or comma-separated).
///
/// Each line: `title\tcontent` or `title,content`.
/// Lines with fewer than 2 fields are skipped.
pub fn parse_faq(content: &str) -> Result<Vec<KnowledgeEntry>> {
    let mut entries = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parts: Vec<&str> = if line.contains('\t') {
            line.split('\t').collect()
        } else if line.contains(',') {
            line.split(',').collect()
        } else {
            continue;
        };

        if parts.len() >= 2 {
            entries.push(KnowledgeEntry {
                title: parts[0].trim().to_string(),
                content: parts[1].trim().to_string(),
                source: Some("faq".to_string()),
            });
        }
    }

    Ok(entries)
}

/// Parse chat records (JSON format supported: WeChat, WeCom, WhatsApp, Telegram, DingTalk).
///
/// Falls back to a single entry with the raw content if not valid JSON.
pub fn parse_chat(content: &str) -> Result<Vec<KnowledgeEntry>> {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(content) {
        return parse_json_chat(&json);
    }

    // Fallback: treat as plain text
    Ok(vec![KnowledgeEntry {
        title: "聊天记录".to_string(),
        content: content.to_string(),
        source: Some("chat".to_string()),
    }])
}

/// Parse chat JSON, auto-detecting platform format.
fn parse_json_chat(json: &serde_json::Value) -> Result<Vec<KnowledgeEntry>> {
    let mut entries = Vec::new();

    // WeCom room format: { "roomid": "...", "roomname": "...", "msgs": [...] }
    if json.get("roomid").is_some() {
        if let Some(msgs) = json.get("msgs").and_then(|m| m.as_array()) {
            let room_name = json
                .get("roomname")
                .or_else(|| json.get("roomid"))
                .and_then(|v| v.as_str())
                .unwrap_or("群聊");
            entries.extend(parse_message_array(
                msgs,
                &format!("企业微信-{}", room_name),
            )?);
            return Ok(entries);
        }
    }

    // WeChat/WeCom: { "msgs": [...] }
    if let Some(msgs) = json.get("msgs").and_then(|m| m.as_array()) {
        entries.extend(parse_message_array(msgs, "微信聊天记录")?);
        return Ok(entries);
    }

    // WhatsApp: { "messages": [...], "chat_name": "..." }
    if let Some(msgs) = json.get("messages").and_then(|m| m.as_array()) {
        if json.get("chat_name").is_some() || json.get("contact_name").is_some() {
            entries.extend(parse_message_array(msgs, "WhatsApp 聊天记录")?);
            return Ok(entries);
        }
    }

    // Telegram: { "name": "...", "type": "...", "messages": [...] }
    if let Some(msgs) = json.get("messages").and_then(|m| m.as_array()) {
        if json.get("name").is_some() && json.get("type").is_some() {
            entries.extend(parse_message_array(msgs, "Telegram 聊天记录")?);
            return Ok(entries);
        }
    }

    // DingTalk: { "conversationId": "...", "messages": [...] }
    if let Some(msgs) = json.get("messages").and_then(|m| m.as_array()) {
        if json.get("conversationId").is_some()
            || msgs.first().is_some_and(|m| m.get("senderNick").is_some())
        {
            entries.extend(parse_message_array(msgs, "钉钉聊天记录")?);
            return Ok(entries);
        }
    }

    // DingTalk: direct array with senderNick
    if json.is_array() {
        let arr = json.as_array().unwrap();
        if arr.first().is_some_and(|m| m.get("senderNick").is_some()) {
            entries.extend(parse_message_array(arr, "钉钉聊天记录")?);
            return Ok(entries);
        }
    }

    // Generic: array of messages
    if entries.is_empty() {
        if let Some(arr) = json.as_array() {
            entries.extend(parse_message_array(arr, "聊天记录")?);
        }
    }

    if entries.is_empty() {
        entries.push(KnowledgeEntry {
            title: "聊天记录".to_string(),
            content: json.to_string(),
            source: Some("chat".to_string()),
        });
    }

    Ok(entries)
}

/// Batch messages into groups of 20 per knowledge entry.
fn parse_message_array(msgs: &[serde_json::Value], source: &str) -> Result<Vec<KnowledgeEntry>> {
    let mut entries = Vec::new();
    let mut current_entry = String::new();
    let mut current_title = String::new();
    let mut message_count = 0;

    for msg in msgs {
        let text = extract_message_text(msg);
        if text.is_empty() {
            continue;
        }

        let sender = extract_sender(msg);
        message_count += 1;

        if message_count % 20 == 1 {
            current_title = if sender.is_empty() {
                format!("{} 第{}批", source, (message_count + 19) / 20)
            } else {
                format!("{} - {}第{}批", source, sender, (message_count + 19) / 20)
            };
            current_entry.clear();
        }

        if !sender.is_empty() {
            current_entry.push_str(&format!("【{}】{}\n\n", sender, text));
        } else {
            current_entry.push_str(&format!("{}\n\n", text));
        }

        if message_count % 20 == 0 {
            entries.push(KnowledgeEntry {
                title: current_title.clone(),
                content: current_entry.trim().to_string(),
                source: Some(source.to_string()),
            });
        }
    }

    // Remaining messages
    if message_count % 20 != 0 && !current_entry.is_empty() {
        entries.push(KnowledgeEntry {
            title: current_title,
            content: current_entry.trim().to_string(),
            source: Some(source.to_string()),
        });
    }

    Ok(entries)
}

fn extract_message_text(msg: &serde_json::Value) -> String {
    for key in ["text", "content", "message", "Message", "msg", "Msg"] {
        if let Some(text) = msg.get(key).and_then(|v| v.as_str()) {
            return text.to_string();
        }
    }
    String::new()
}

fn extract_sender(msg: &serde_json::Value) -> String {
    for key in ["sender", "from", "name", "user", "author", "senderNick"] {
        if let Some(name) = msg.get(key).and_then(|v| v.as_str()) {
            return name.to_string();
        }
    }
    String::new()
}

/// Parse document into paragraph-based knowledge entries.
///
/// Splits on double-newlines. Skips blocks shorter than 10 chars.
/// Title is the first 50 chars of each block.
pub fn parse_document(content: &str) -> Result<Vec<KnowledgeEntry>> {
    let mut entries = Vec::new();

    let blocks: Vec<&str> = content
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .collect();

    for block in &blocks {
        let block = block.trim();
        if block.len() < 10 {
            continue;
        }

        let title = if block.chars().count() > 50 {
            format!("{}...", block.chars().take(50).collect::<String>())
        } else {
            block.lines().next().unwrap_or("").to_string()
        };

        entries.push(KnowledgeEntry {
            title,
            content: block.to_string(),
            source: Some("document".to_string()),
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_faq_tsv() {
        let content = "退款政策\t购买后7天可退款\n发货时间\t48小时内发货";
        let entries = parse_faq(content).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].title, "退款政策");
        assert_eq!(entries[0].content, "购买后7天可退款");
    }

    #[test]
    fn test_parse_faq_csv() {
        let content = "Q1,A1\nQ2,A2";
        let entries = parse_faq(content).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn test_parse_faq_skip_invalid() {
        let content = "only_one_field\n\n\t\n";
        let entries = parse_faq(content).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_parse_chat_wechat() {
        let json = r#"{"msgs": [
            {"CreateTime": 1234567890, "Message": "你好"},
            {"CreateTime": 1234567891, "Message": "在吗"}
        ]}"#;
        let entries = parse_chat(json).unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].content.contains("你好"));
    }

    #[test]
    fn test_parse_chat_wecom_room() {
        let json = r#"{
            "roomid": "room123",
            "roomname": "测试群",
            "msgs": [
                {"CreateTime": 1234567890, "Message": "hello"},
                {"CreateTime": 1234567891, "Message": "world"}
            ]
        }"#;
        let entries = parse_chat(json).unwrap();
        assert!(!entries.is_empty());
        assert!(entries[0].content.contains("hello"));
    }

    #[test]
    fn test_parse_chat_plain_text() {
        let content = "这是普通文本聊天记录";
        let entries = parse_chat(content).unwrap();
        assert!(!entries.is_empty());
        assert_eq!(entries[0].title, "聊天记录");
    }

    #[test]
    fn test_parse_document() {
        let content = "第一段落的内容，足够长以满足最低长度要求。这是第一个段落。\n\n第二段落的内容，也足够长。这是第二个段落的内容。";
        let entries = parse_document(content).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source, Some("document".to_string()));
    }

    #[test]
    fn test_parse_document_short_blocks_skipped() {
        let content = "短\n\n这个段落足够长，超过了十个字符的最低要求。";
        let entries = parse_document(content).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_parse_import_data_dispatch() {
        let faq = "Q1,A1";
        let result = parse_import_data(faq, "faq").unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].source, Some("faq".to_string()));
        assert_eq!(result.quality, ParseQuality::Good);

        let doc = "A longer document paragraph with enough content to pass the filter.";
        let result = parse_import_data(doc, "document").unwrap();
        assert!(!result.entries.is_empty());
    }

    #[test]
    fn test_quality_fallback_for_plain_text() {
        let content =
            "This is a plain text chat log that isn't JSON at all, so it should be a fallback.";
        let result = parse_import_data(content, "chat").unwrap();
        assert_eq!(result.quality, ParseQuality::Fallback);
    }

    #[test]
    fn test_quality_good_for_structured_faq() {
        let content = "Q1,A1\nQ2,A2\nQ3,A3";
        let result = parse_import_data(content, "faq").unwrap();
        assert_eq!(result.quality, ParseQuality::Good);
    }

    #[test]
    fn test_build_tier2_prompt() {
        let (sys, user) = build_tier2_prompt("some raw data", "chat");
        assert!(sys.contains("数据解析助手"));
        assert!(user.contains("chat"));
        assert!(user.contains("some raw data"));
    }

    #[test]
    fn test_parse_tier2_response() {
        let json = r#"[{"title": "test", "content": "test content"}]"#;
        let entries = parse_tier2_response(json).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "test");
        assert_eq!(entries[0].source, Some("import-tier2".to_string()));
    }

    #[test]
    fn test_parse_tier2_response_in_markdown() {
        let text = "```json\n[{\"title\": \"a\", \"content\": \"b\"}, {\"title\": \"c\", \"content\": \"d\"}]\n```";
        let entries = parse_tier2_response(text).unwrap();
        assert_eq!(entries.len(), 2);
    }
}
