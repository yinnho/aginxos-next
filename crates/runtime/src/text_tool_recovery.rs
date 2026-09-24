//! Text-based tool-call handling for the agent loop.
//!
//! aginxbrain proxies every LLM call and normalizes raw provider dialects
//! (Groq/Llama/DeepSeek/Qwen/Ollama/...) into OpenAI `tool_calls` upstream, so
//! the agent loop only ever sees structured tool calls in the normal case. This
//! module covers the two residual cases that are genuinely provider-independent:
//!
//! - [`detect_text_tool_mentions`] — some models still *narrate* a tool call as
//!   `[Called name]` in the content instead of emitting structured tool_use.
//!   We detect that narration and nudge the model to retry with structured
//!   tool_use (we do **not** scrape arguments or execute text-described calls).
//! - [`strip_tool_call_artifacts`] — strip `[Called ...]` from final response
//!   text so users never see the raw narration.
//!
//! The previous 13 provider-dialect text parsers were removed once aginxbrain
//! began normalizing every response to OpenAI format (B9).

use carrier_types::tool_compat::normalize_tool_name;

/// Detect tool names the model narrated as `[Called name]` in the response text
/// instead of emitting structured tool_use.
///
/// Returns deduplicated, [`normalize_tool_name`]-normalized names in first-seen
/// order. Empty when the model made no such narration (the common case — caller
/// treats that as a normal end turn).
///
/// This is intentionally minimal: `[Called ...]` is the narration convention
/// this codebase already strips and nudges around, and it is provider-independent.
/// If production telemetry shows the detector missing real cases, extend here
/// rather than reviving dialect-specific parsers.
/// Narration markers some models emit as text instead of structured tool_use.
/// `[Called ` is the English convention; `[调用 `/`[执行 ` are the Chinese
/// equivalents (调用=invoke, 执行=execute) seen from CN-leaning models routed
/// via aginxbrain (Qwen/Kimi) - e.g. car-finder-v2 emitted `[调用 sqlite_query]`
/// which the English-only detector missed, sending raw text to the user with no
/// tool executed. Each marker expects a tool name then `]`.
const NARRATION_MARKERS: &[&str] = &["[Called ", "[调用 ", "[执行 "];

/// User-facing reply substituted when the model keeps narrating tool calls as
/// text even after the final no-tools attempt — narration text is never
/// relayed to the user (08-21 86bus: the model parroted the old synthetic
/// "我需要调用工具：x。" precedent verbatim as its final answer).
pub const NARRATION_FALLBACK_REPLY: &str =
    "抱歉，这轮我想调用的工具一直没能正确执行，请稍后重发一次消息，或换个说法告诉我。";

pub fn detect_text_tool_mentions(text: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for marker in NARRATION_MARKERS {
        let mut search_from = 0;
        while let Some(pos) = text[search_from..].find(marker) {
            let abs = search_from + pos;
            let after = &text[abs + marker.len()..];
            match after.find(']') {
                Some(close) => {
                    let name = normalize_tool_name(after[..close].trim()).to_string();
                    if !name.is_empty() && seen.insert(name.clone()) {
                        out.push(name);
                    }
                    search_from = abs + marker.len() + close + 1;
                }
                None => break,
            }
        }
    }
    out
}

/// Strip `[Called ...]` patterns from response text so users never see
/// raw tool-call syntax, even when text-based recovery gave up.
pub fn strip_tool_call_artifacts(text: &str) -> String {
    let mut result = text.to_string();
    for marker in NARRATION_MARKERS {
        let mut search_from = 0;
        while let Some(pos) = result[search_from..].find(marker) {
            let abs_pos = search_from + pos;
            let after = &result[abs_pos + marker.len()..];
            if let Some(close) = after.find(']') {
                result.replace_range(abs_pos..abs_pos + marker.len() + close + 1, "");
                // Don't advance - re-scan from same position since text shifted
                search_from = abs_pos;
            } else {
                break;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_single_mention() {
        let mentions = detect_text_tool_mentions("还没，正在执行流程。[Called knowledge_read]");
        assert_eq!(mentions, vec!["knowledge_read".to_string()]);
    }

    #[test]
    fn test_detect_multiple_mentions() {
        let mentions = detect_text_tool_mentions(
            "先读知识库。[Called knowledge_read]然后搜索。[Called web_search]",
        );
        assert_eq!(
            mentions,
            vec!["knowledge_read".to_string(), "web_search".to_string()]
        );
    }

    #[test]
    fn test_detect_dedup() {
        let mentions = detect_text_tool_mentions("[Called web_search] 再 [Called web_search]");
        assert_eq!(mentions, vec!["web_search".to_string()]);
    }

    #[test]
    fn test_detect_normalizes_name() {
        // Trailing punctuation from free text is stripped by normalize_tool_name.
        let mentions = detect_text_tool_mentions("[Called web_search,]");
        assert_eq!(mentions, vec!["web_search".to_string()]);
    }

    #[test]
    fn test_detect_no_mention() {
        assert!(detect_text_tool_mentions("这是一条普通回复，没有工具调用。").is_empty());
    }

    #[test]
    fn test_detect_unclosed_ignored() {
        assert!(detect_text_tool_mentions("这里有个 [Called tool 没有闭合").is_empty());
    }

    #[test]
    fn test_strip_single_called() {
        assert_eq!(
            strip_tool_call_artifacts("还没，正在执行排版和发布流程。[Called knowledge_read]"),
            "还没，正在执行排版和发布流程。"
        );
    }

    #[test]
    fn test_strip_multiple_called() {
        assert_eq!(
            strip_tool_call_artifacts(
                "我需要先搜索一下。[Called tool_search] 然后再读。[Called knowledge_read]"
            ),
            "我需要先搜索一下。 然后再读。"
        );
    }

    #[test]
    fn test_strip_no_called() {
        assert_eq!(
            strip_tool_call_artifacts("这是一条普通回复，没有工具调用。"),
            "这是一条普通回复，没有工具调用。"
        );
    }

    #[test]
    fn test_strip_unclosed_bracket_ignored() {
        assert_eq!(
            strip_tool_call_artifacts("这里有个 [Called tool 没有闭合"),
            "这里有个 [Called tool 没有闭合"
        );
    }

    #[test]
    fn test_detect_chinese_diaoyong() {
        // Regression: car-finder-v2 emitted `[调用 sqlite_query]` as text.
        // The English-only `[Called ` detector missed it, so no recovery fired
        // and the raw text reached the user with no tool executed.
        let mentions = detect_text_tool_mentions("正在查库。[调用 sqlite_query]");
        assert_eq!(mentions, vec!["sqlite_query".to_string()]);
    }

    #[test]
    fn test_fallback_reply_is_clean() {
        // The fallback itself must never contain narration markers (and thus
        // never get stripped into garbage).
        assert!(!NARRATION_FALLBACK_REPLY.is_empty());
        assert!(detect_text_tool_mentions(NARRATION_FALLBACK_REPLY).is_empty());
        assert_eq!(
            strip_tool_call_artifacts(NARRATION_FALLBACK_REPLY),
            NARRATION_FALLBACK_REPLY
        );
    }

    #[test]
    fn test_detect_chinese_zhixing() {
        let mentions = detect_text_tool_mentions("[执行 web_search] 然后处理。");
        assert_eq!(mentions, vec!["web_search".to_string()]);
    }

    #[test]
    fn test_detect_mixed_en_cn() {
        let mentions = detect_text_tool_mentions("[Called knowledge_read] 再 [调用 sqlite_query]");
        assert_eq!(
            mentions,
            vec!["knowledge_read".to_string(), "sqlite_query".to_string()]
        );
    }

    #[test]
    fn test_strip_chinese_diaoyong() {
        assert_eq!(
            strip_tool_call_artifacts("正在查库。[调用 sqlite_query]"),
            "正在查库。"
        );
    }
}
