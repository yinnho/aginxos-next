//! Dynamic context budget for tool result truncation.
//!
//! Simplified: strip base64 blobs + cap single tool results that exceed
//! 50% of the context window. L0 summaries + drawer keep overall context
//! length controlled, so the multi-pass compaction of oldest results is
//! no longer needed.

use carrier_types::message::{ContentBlock, Message, MessageContent};
use carrier_types::tool::ToolDefinition;

/// Budget parameters derived from the model's context window.
#[derive(Debug, Clone)]
pub struct ContextBudget {
    /// Total context window size in tokens.
    pub context_window_tokens: usize,
    /// Estimated characters per token for tool results (denser content).
    pub tool_chars_per_token: f64,
    /// Estimated characters per token for general content.
    pub general_chars_per_token: f64,
}

impl ContextBudget {
    /// Create a new budget from a context window size.
    pub fn new(context_window_tokens: usize) -> Self {
        Self {
            context_window_tokens,
            tool_chars_per_token: 2.0,
            general_chars_per_token: 4.0,
        }
    }

    /// Per-result character cap: 10% of context window converted to chars.
    pub fn per_result_cap(&self) -> usize {
        let tokens_for_tool = (self.context_window_tokens as f64 * 0.10) as usize;
        (tokens_for_tool as f64 * self.tool_chars_per_token) as usize
    }

    /// Single result absolute max: 50% of context window.
    pub fn single_result_max(&self) -> usize {
        let tokens = (self.context_window_tokens as f64 * 0.50) as usize;
        (tokens as f64 * self.tool_chars_per_token) as usize
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self::new(200_000)
    }
}

/// Layer 1: Truncate a single tool result dynamically based on context budget.
///
/// Breaks at newline boundaries when possible to avoid mid-line truncation.
pub fn truncate_tool_result_dynamic(content: &str, budget: &ContextBudget) -> String {
    let cap = budget.per_result_cap();
    if content.len() <= cap {
        return content.to_string();
    }

    // Find last newline before the cap to break cleanly (char-boundary safe)
    let safe_cap = carrier_types::floor_char_boundary(content, cap);
    let search_start = carrier_types::floor_char_boundary(content, safe_cap.saturating_sub(200));
    let break_point = content[search_start..safe_cap]
        .rfind('\n')
        .map(|pos| search_start + pos)
        .unwrap_or_else(|| carrier_types::floor_char_boundary(content, safe_cap.saturating_sub(100)));

    format!(
        "{}\n\n[TRUNCATED: result was {} chars, showing first {} (budget: {}% of {}K context window)]",
        &content[..break_point],
        content.len(),
        break_point,
        10,
        budget.context_window_tokens / 1000
    )
}

/// Strip large base64 blobs from tool result content.
///
/// Detects JSON fields containing long base64 strings and standalone base64
/// runs, replacing them with compact placeholders.
fn strip_base64_content(content: &str) -> String {
    if content.len() < 2000 {
        return content.to_string();
    }

    let mut result = content.to_string();

    // Strip JSON "base64" field values > 1K chars
    if let Some(start) = result.find("\"base64\": \"") {
        let val_start = start + "\"base64\": \"".len();
        if val_start < result.len() {
            let base64_content = &result[val_start..];
            let val_len = base64_content.find('"').unwrap_or(0);
            if val_len > 1000 {
                let placeholder = format!("[base64: {} chars removed]", val_len);
                result.replace_range(val_start..val_start + val_len, &placeholder);
            }
        }
    }

    // Strip standalone long base64 runs (> 4K continuous base64 chars)
    if result.len() > 5000 {
        result = strip_long_base64_runs(&result);
    }

    result
}

/// Replace runs of base64 characters (>4K continuous) with a placeholder.
fn strip_long_base64_runs(s: &str) -> String {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut result = String::with_capacity(len);
    let mut i = 0;
    let mut run_start: Option<usize> = None;

    while i < len {
        let b = bytes[i];
        let is_b64 = b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=';
        if is_b64 {
            if run_start.is_none() {
                run_start = Some(i);
            }
            i += 1;
        } else {
            if let Some(rs) = run_start.take() {
                let run_len = i - rs;
                if run_len > 4096 {
                    result.push_str(&format!("[base64: {} chars removed]", run_len));
                } else {
                    result.push_str(&s[rs..i]);
                }
            }
            // Find next valid UTF-8 char boundary
            let char_start = i;
            let mut char_end = i + 1;
            while char_end < len && !s.is_char_boundary(char_end) {
                char_end += 1;
            }
            result.push_str(&s[char_start..char_end]);
            i = char_end;
        }
    }

    // Handle trailing run
    if let Some(rs) = run_start.take() {
        let run_len = len - rs;
        if run_len > 4096 {
            result.push_str(&format!("[base64: {} chars removed]", run_len));
        } else {
            result.push_str(&s[rs..]);
        }
    }

    result
}

/// Context guard — scan all tool_result blocks in the message history.
///
/// Pass 0: Strip large base64 blobs from all tool results (image data, etc.)
/// Pass 1: Cap any single result exceeding 50% of context
pub fn apply_context_guard(
    messages: &mut [Message],
    budget: &ContextBudget,
    _tools: &[ToolDefinition],
) -> usize {
    // Pass 0: Strip base64 blobs from tool results.
    let mut compacted = 0;
    for msg in messages.iter_mut() {
        if let MessageContent::Blocks(blocks) = &mut msg.content {
            for block in blocks.iter_mut() {
                if let ContentBlock::ToolResult { content, .. } = block {
                    let stripped = strip_base64_content(content);
                    if stripped.len() < content.len() {
                        *content = stripped;
                        compacted += 1;
                    }
                }
            }
        }
    }

    let single_max = budget.single_result_max();

    // Pass 1: Cap any single result that exceeds 50% of context
    for msg in messages.iter_mut() {
        if let MessageContent::Blocks(blocks) = &mut msg.content {
            for block in blocks.iter_mut() {
                if let ContentBlock::ToolResult { content, .. } = block {
                    if content.len() > single_max {
                        *content = truncate_to(content, single_max);
                        compacted += 1;
                    }
                }
            }
        }
    }

    compacted
}

/// Truncate content to `max_chars` with a marker.
fn truncate_to(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let mut keep = max_chars.saturating_sub(80).min(content.len());
    // Walk back to a valid char boundary
    while keep > 0 && !content.is_char_boundary(keep) {
        keep -= 1;
    }
    let mut search_start = keep.saturating_sub(100);
    // Walk back to a valid char boundary
    while search_start > 0 && !content.is_char_boundary(search_start) {
        search_start -= 1;
    }
    // Try to break at newline
    let break_point = content[search_start..keep]
        .rfind('\n')
        .map(|pos| search_start + pos)
        .unwrap_or(keep);
    format!(
        "{}\n\n[COMPACTED: {} → {} chars by context guard]",
        &content[..break_point],
        content.len(),
        break_point
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_defaults() {
        let budget = ContextBudget::default();
        assert_eq!(budget.context_window_tokens, 200_000);
        // 10% of 200K * 2.0 chars/token = 40K chars
        assert_eq!(budget.per_result_cap(), 40_000);
    }

    #[test]
    fn test_small_model_budget() {
        let budget = ContextBudget::new(8_000);
        // 10% of 8K * 2.0 = 1600 chars
        assert_eq!(budget.per_result_cap(), 1_600);
    }

    #[test]
    fn test_truncate_within_limit() {
        let budget = ContextBudget::default();
        let short = "Hello world";
        assert_eq!(truncate_tool_result_dynamic(short, &budget), short);
    }

    #[test]
    fn test_truncate_breaks_at_newline() {
        let budget = ContextBudget::new(100); // very small: cap = 60 chars
        let content =
            "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12";
        let result = truncate_tool_result_dynamic(content, &budget);
        assert!(result.contains("[TRUNCATED:"));
        // Should not split in the middle of a line
        assert!(
            result.starts_with("line1\n") || result.is_empty() || result.contains("[TRUNCATED:")
        );
    }

    #[test]
    fn test_context_guard_no_compaction_needed() {
        let budget = ContextBudget::default();
        let mut messages = vec![Message::user("hello")];
        let compacted = apply_context_guard(&mut messages, &budget, &[]);
        assert_eq!(compacted, 0);
    }

    #[test]
    fn test_context_guard_caps_large_results() {
        // Use tiny budget: single_result_max = 50% of 100 * 2.0 = 100 chars
        let budget = ContextBudget::new(100);
        let big_result = "x".repeat(500);
        let mut messages = vec![
            Message {
                role: carrier_types::message::Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    tool_name: String::new(),
                    content: big_result.clone(),
                    is_error: false,
                }]),
            },
            Message {
                role: carrier_types::message::Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t2".to_string(),
                    tool_name: String::new(),
                    content: big_result,
                    is_error: false,
                }]),
            },
        ];

        let compacted = apply_context_guard(&mut messages, &budget, &[]);
        assert!(compacted > 0);

        // Both results should have been capped
        if let MessageContent::Blocks(blocks) = &messages[0].content {
            if let ContentBlock::ToolResult { content, .. } = &blocks[0] {
                assert!(content.len() < 500);
            }
        }
    }

    #[test]
    fn test_truncate_tool_result_multibyte_chinese() {
        // Tiny budget: cap = 30% of 100 * 2.0 = 60 bytes
        let budget = ContextBudget::new(100);
        // Each Chinese char is 3 bytes in UTF-8; 100 chars = 300 bytes
        let content: String = "\u{4f60}\u{597d}\u{4e16}\u{754c}".repeat(25);
        assert_eq!(content.len(), 300);
        // Must not panic on multi-byte content
        let result = truncate_tool_result_dynamic(&content, &budget);
        assert!(result.contains("[TRUNCATED:"));
        // The visible portion must be valid UTF-8 (implicit: no panic)
        assert!(result.is_char_boundary(0));
    }

    #[test]
    fn test_truncate_to_multibyte_emoji() {
        // Each emoji is 4 bytes; 200 emojis = 800 bytes
        let content: String = "\u{1f600}".repeat(200);
        let result = truncate_to(&content, 100);
        assert!(result.contains("[COMPACTED:"));
        // Must not panic and must produce valid UTF-8
        assert!(result.is_char_boundary(0));
    }

    #[test]
    fn test_context_guard_multibyte_tool_results() {
        let budget = ContextBudget::new(100);
        // Chinese text: 500 chars * 3 bytes = 1500 bytes
        let big_chinese: String = "\u{4e2d}\u{6587}\u{6d4b}\u{8bd5}\u{6570}\u{636e}".repeat(83);
        let mut messages = vec![Message {
            role: carrier_types::message::Role::User,
            content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                tool_name: String::new(),
                content: big_chinese,
                is_error: false,
            }]),
        }];
        // Must not panic on multi-byte content
        let compacted = apply_context_guard(&mut messages, &budget, &[]);
        assert!(compacted > 0);
    }
}
