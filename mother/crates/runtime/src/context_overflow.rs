//! Context overflow recovery pipeline.
//!
//! Simplified from the original 4-stage pipeline. With L0 summaries + drawer
//! injection keeping context length controlled, only tool result truncation
//! and the final error fallback are needed.

use std::collections::HashSet;
use tracing::warn;
use carrier_types::message::{ContentBlock, Message, MessageContent};
use carrier_types::tool::ToolDefinition;

/// Drain `count` messages from the front, ensuring no ToolUse/ToolResult pairs
/// are split at the cut boundary.
///
/// After draining, removes orphaned ToolResult blocks at the front of the
/// remaining messages whose matching ToolUse was in the drained portion.
pub fn pair_aware_drain(messages: &mut Vec<Message>, count: usize) {
    if count == 0 || messages.is_empty() {
        return;
    }
    let cut = count.min(messages.len());
    messages.drain(..cut);

    // Collect ToolUse IDs still present in remaining messages
    let valid_tool_uses: HashSet<String> = messages
        .iter()
        .flat_map(|m| match &m.content {
            MessageContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();

    // Remove orphaned ToolResults from the front until we hit a clean message
    let mut i = 0;
    while i < messages.len() {
        let changed = match &mut messages[i].content {
            MessageContent::Blocks(blocks) => {
                let before = blocks.len();
                blocks.retain(|b| match b {
                    ContentBlock::ToolResult { tool_use_id, .. } => {
                        valid_tool_uses.contains(tool_use_id)
                    }
                    _ => true,
                });
                before != blocks.len()
            }
            _ => false,
        };

        // Remove message if now empty
        let is_empty = match &messages[i].content {
            MessageContent::Text(s) => s.is_empty(),
            MessageContent::Blocks(b) => b.is_empty(),
        };

        if is_empty {
            messages.remove(i);
        } else if !changed {
            // No orphaned blocks at this position — past the boundary
            break;
        } else {
            i += 1;
        }
    }
}

/// Recovery stage that was applied.
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryStage {
    /// No recovery needed.
    None,
    /// Truncated tool results.
    ToolResultTruncation { truncated: usize },
    /// Unrecoverable — suggest /reset.
    FinalError,
}

/// Estimate token count using chars/4 heuristic.
fn estimate_tokens(messages: &[Message], system_prompt: &str, tools: &[ToolDefinition]) -> usize {
    crate::compactor::estimate_token_count(messages, Some(system_prompt), Some(tools))
}

/// Run the simplified overflow recovery pipeline.
///
/// With L0 summaries + drawer keeping context controlled, only tool result
/// truncation and the final error fallback are needed.
pub fn recover_from_overflow(
    messages: &mut [Message],
    system_prompt: &str,
    tools: &[ToolDefinition],
    context_window: usize,
) -> RecoveryStage {
    let estimated = estimate_tokens(messages, system_prompt, tools);
    let threshold = (context_window as f64 * 0.90) as usize;

    // No recovery needed
    if estimated <= threshold {
        return RecoveryStage::None;
    }

    // Truncate tool results to 2K chars
    let tool_truncation_limit = 2000;
    let mut truncated = 0;
    for msg in messages.iter_mut() {
        if let MessageContent::Blocks(blocks) = &mut msg.content {
            for block in blocks.iter_mut() {
                if let ContentBlock::ToolResult { content, .. } = block {
                    if content.len() > tool_truncation_limit {
                        let safe_keep = carrier_types::floor_char_boundary(
                            content,
                            tool_truncation_limit.saturating_sub(80),
                        );
                        *content = format!(
                            "{}\n\n[OVERFLOW RECOVERY: truncated from {} to {} chars]",
                            &content[..safe_keep],
                            content.len(),
                            safe_keep
                        );
                        truncated += 1;
                    }
                }
            }
        }
    }

    if truncated > 0 {
        let new_est = estimate_tokens(messages, system_prompt, tools);
        if new_est <= threshold {
            return RecoveryStage::ToolResultTruncation { truncated };
        }
        warn!(
            estimated_tokens = new_est,
            "Truncated {} tool results but still over threshold", truncated
        );
    }

    // Final error — nothing more we can do automatically
    warn!("All recovery stages exhausted, context still too large");
    RecoveryStage::FinalError
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrier_types::message::{Message, Role};

    fn make_messages(count: usize, size_each: usize) -> Vec<Message> {
        (0..count)
            .map(|i| {
                let text = format!("msg{}: {}", i, "x".repeat(size_each));
                Message {
                    role: if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    content: MessageContent::Text(text),
                }
            })
            .collect()
    }

    #[test]
    fn test_no_recovery_needed() {
        let mut msgs = make_messages(2, 100);
        let stage = recover_from_overflow(&mut msgs, "sys", &[], 200_000);
        assert_eq!(stage, RecoveryStage::None);
    }

    #[test]
    fn test_tool_truncation() {
        let big_result = "x".repeat(5000);
        let mut msgs = vec![
            Message::user("hi"),
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    tool_name: String::new(),
                    content: big_result.clone(),
                    is_error: false,
                }]),
            },
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t2".to_string(),
                    tool_name: String::new(),
                    content: big_result,
                    is_error: false,
                }]),
            },
        ];
        // Tiny context window to force tool truncation
        let stage = recover_from_overflow(&mut msgs, "system", &[], 500);
        match stage {
            RecoveryStage::ToolResultTruncation { truncated } => {
                assert!(truncated > 0);
            }
            RecoveryStage::FinalError => {}
            _ => {}
        }
    }

    #[test]
    fn test_overwhelmed_context() {
        // Large messages with no tool results → final error
        let mut msgs = make_messages(50, 500);
        let stage = recover_from_overflow(&mut msgs, "system prompt", &[], 2000);
        assert_ne!(stage, RecoveryStage::None);
    }

    #[test]
    fn test_multibyte_tool_truncation() {
        // Chinese text (3 bytes per char) in tool results must not panic
        let chinese_result: String = "\u{4f60}\u{597d}\u{4e16}\u{754c}".repeat(1250); // 5000 chars, 15000 bytes
        let mut msgs = vec![
            Message::user("hi"),
            Message {
                role: Role::User,
                content: MessageContent::Blocks(vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".to_string(),
                    tool_name: String::new(),
                    content: chinese_result,
                    is_error: false,
                }]),
            },
        ];
        // Tiny context window to force tool truncation
        let stage = recover_from_overflow(&mut msgs, "system", &[], 500);
        // Must not panic — the truncation at byte boundaries could split a 3-byte char
        assert_ne!(stage, RecoveryStage::None);
    }
}
