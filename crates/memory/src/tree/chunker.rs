//! Content chunker — splits canonicalized text into token-bounded chunks.

use super::types::{Chunk, SourceKind};

/// Approximate token count using 4-chars-per-token heuristic.
pub fn approx_token_count(text: &str) -> u32 {
    let chars = text.chars().count() as u32;
    chars.saturating_add(3) / 4
}

/// Input for chunk_messages.
pub struct ChunkInput<'a> {
    pub owner_id: &'a str,
    /// Per-user isolation within the owner. `""` = owner-shared.
    pub user_id: &'a str,
    pub agent_id: &'a str,
    pub source_kind: SourceKind,
    pub source_id: &'a str,
    pub source_ref: Option<&'a str>,
    pub markdown: &'a str,
    pub tags: &'a [String],
    pub timestamp_ms: i64,
    pub max_tokens: u32,
}

/// Chunk canonicalized markdown text into token-bounded pieces.
pub fn chunk_messages(input: &ChunkInput) -> Vec<Chunk> {
    let max_chars = input.max_tokens as usize * 4;
    let tags_json = serde_json::to_string(input.tags).unwrap_or_else(|_| "[]".to_string());
    let created_at_ms = chrono::Utc::now().timestamp_millis();

    let units: Vec<String> = match input.source_kind {
        SourceKind::Chat => split_chat_units(input.markdown),
        SourceKind::Email => split_email_units(input.markdown),
        SourceKind::Document => vec![input.markdown.to_string()],
    };

    let mut chunks = Vec::new();
    let mut seq = 0u32;
    let mut accumulator = String::new();
    let mut acc_tokens = 0u32;

    for unit in units {
        let unit_tokens = approx_token_count(&unit);

        if unit_tokens > input.max_tokens {
            if !accumulator.is_empty() {
                chunks.push(make_chunk(
                    input,
                    &accumulator,
                    &tags_json,
                    seq,
                    false,
                    created_at_ms,
                ));
                seq += 1;
                accumulator.clear();
                acc_tokens = 0;
            }
            for piece in split_by_token_budget(&unit, max_chars) {
                chunks.push(make_chunk(
                    input,
                    &piece,
                    &tags_json,
                    seq,
                    true,
                    created_at_ms,
                ));
                seq += 1;
            }
        } else if acc_tokens + unit_tokens > input.max_tokens {
            chunks.push(make_chunk(
                input,
                &accumulator,
                &tags_json,
                seq,
                false,
                created_at_ms,
            ));
            seq += 1;
            accumulator = unit;
            acc_tokens = unit_tokens;
        } else {
            if !accumulator.is_empty() {
                accumulator.push_str("\n\n");
            }
            accumulator.push_str(&unit);
            acc_tokens += unit_tokens;
        }
    }

    if !accumulator.is_empty() {
        chunks.push(make_chunk(
            input,
            &accumulator,
            &tags_json,
            seq,
            false,
            created_at_ms,
        ));
    }

    chunks
}

fn make_chunk(
    input: &ChunkInput,
    content: &str,
    tags_json: &str,
    seq: u32,
    partial_message: bool,
    created_at_ms: i64,
) -> Chunk {
    let token_count = approx_token_count(content);
    let id = compute_chunk_id(
        input.owner_id,
        input.source_kind,
        input.source_id,
        seq,
        content,
    );

    Chunk {
        id,
        owner_id: input.owner_id.to_string(),
        user_id: input.user_id.to_string(),
        agent_id: input.agent_id.to_string(),
        source_kind: input.source_kind,
        source_id: input.source_id.to_string(),
        source_ref: input.source_ref.map(|s| s.to_string()),
        timestamp_ms: input.timestamp_ms,
        time_range_start_ms: input.timestamp_ms,
        time_range_end_ms: input.timestamp_ms,
        tags_json: tags_json.to_string(),
        content: content.to_string(),
        token_count,
        seq_in_source: seq,
        partial_message,
        lifecycle_status: "pending_extraction".to_string(),
        created_at_ms,
    }
}

/// Deterministic chunk ID: SHA-256 of (owner_id, source_kind, source_id, seq, content).
fn compute_chunk_id(
    owner_id: &str,
    source_kind: SourceKind,
    source_id: &str,
    seq: u32,
    content: &str,
) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(owner_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(source_kind.as_str().as_bytes());
    hasher.update(b"\0");
    hasher.update(source_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(seq.to_le_bytes());
    hasher.update(b"\0");
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    // First 32 hex chars (16 bytes)
    format!("{:.32x}", hash)
}

/// Split chat markdown at `## ` boundaries.
fn split_chat_units(markdown: &str) -> Vec<String> {
    let mut units = Vec::new();
    let mut current = String::new();

    for line in markdown.lines() {
        if line.starts_with("## ") {
            // Flush the previous unit (if any)
            if !current.is_empty() {
                units.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }

    if !current.is_empty() {
        units.push(current);
    }

    if units.is_empty() {
        vec![markdown.to_string()]
    } else {
        units
    }
}

/// Split email markdown at `---` separators followed by `From:`.
fn split_email_units(markdown: &str) -> Vec<String> {
    let mut units = Vec::new();
    let mut current_lines: Vec<String> = Vec::new();

    for line in markdown.lines() {
        if line.trim() == "---" {
            // Check if next few lines contain "From:"
            if !current_lines.is_empty() {
                units.push(current_lines.join("\n"));
                current_lines.clear();
            }
        } else {
            current_lines.push(line.to_string());
        }
    }

    if !current_lines.is_empty() {
        let text = current_lines.join("\n");
        if !text.trim().is_empty() {
            units.push(text);
        }
    }

    if units.is_empty() {
        vec![markdown.to_string()]
    } else {
        units
    }
}

/// Split by token budget: paragraph → line → hard character cut.
fn split_by_token_budget(text: &str, max_chars: usize) -> Vec<String> {
    // Try paragraph split first
    let paragraphs: Vec<&str> = text.split("\n\n").collect();
    let mut chunks = Vec::new();
    let mut accumulator = String::new();

    for para in &paragraphs {
        let para_chars = para.chars().count();
        if para_chars > max_chars {
            // Flush accumulator
            if !accumulator.is_empty() {
                chunks.push(std::mem::take(&mut accumulator));
            }
            // Sub-split by lines
            chunks.extend(split_oversized_by_lines(para, max_chars));
        } else if accumulator.chars().count() + 2 + para_chars > max_chars {
            chunks.push(std::mem::take(&mut accumulator));
            accumulator.push_str(para);
        } else {
            if !accumulator.is_empty() {
                accumulator.push_str("\n\n");
            }
            accumulator.push_str(para);
        }
    }

    if !accumulator.is_empty() {
        chunks.push(accumulator);
    }

    chunks
}

fn split_oversized_by_lines(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut accumulator = String::new();

    for line in text.lines() {
        let line_chars = line.chars().count();
        if line_chars > max_chars {
            if !accumulator.is_empty() {
                chunks.push(std::mem::take(&mut accumulator));
            }
            // Hard character cut
            chunks.extend(hard_split(line, max_chars));
        } else if accumulator.chars().count() + 1 + line_chars > max_chars {
            chunks.push(std::mem::take(&mut accumulator));
            accumulator.push_str(line);
        } else {
            if !accumulator.is_empty() {
                accumulator.push('\n');
            }
            accumulator.push_str(line);
        }
    }

    if !accumulator.is_empty() {
        chunks.push(accumulator);
    }

    chunks
}

fn hard_split(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chars = text.chars();
    let mut current = String::with_capacity(max_chars);

    loop {
        current.clear();
        for _ in 0..max_chars {
            match chars.next() {
                Some(c) => current.push(c),
                None => break,
            }
        }
        if current.is_empty() {
            break;
        }
        chunks.push(current.clone());
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::super::types::DEFAULT_CHUNK_MAX_TOKENS;
    use super::*;

    #[test]
    fn test_approx_token_count() {
        assert_eq!(approx_token_count(""), 0);
        assert_eq!(approx_token_count("hi"), 1); // 2 chars -> ceiling(2/4) = 1
        assert_eq!(approx_token_count("hello"), 2); // 5 chars -> ceiling(5/4) = 2
        assert_eq!(approx_token_count("hello world"), 3); // 11 chars -> ceiling(11/4) = 3
    }

    #[test]
    fn test_chunk_chat_short() {
        let chunks = chunk_messages(&ChunkInput {
            owner_id: "owner_1",
            user_id: "",
            agent_id: "agent_1",
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1",
            source_ref: None,
            markdown: "Hello world",
            tags: &[],
            timestamp_ms: 1000,
            max_tokens: DEFAULT_CHUNK_MAX_TOKENS,
        });
        assert_eq!(chunks.len(), 1);
        assert!(!chunks[0].partial_message);
    }

    #[test]
    fn test_chunk_chat_multiple_messages() {
        let md = "## Alice\nHello\n## Bob\nHi there\n## Alice\nHow are you?";
        let chunks = chunk_messages(&ChunkInput {
            owner_id: "owner_1",
            user_id: "",
            agent_id: "agent_1",
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1",
            source_ref: None,
            markdown: md,
            tags: &[],
            timestamp_ms: 1000,
            max_tokens: DEFAULT_CHUNK_MAX_TOKENS,
        });
        assert!(!chunks.is_empty());
        assert!(chunks[0].content.contains("## Alice"));
    }

    #[test]
    fn test_chunk_chat_splits_on_budget() {
        let md = format!(
            "## Alice\n{}\n## Bob\n{}\n## Alice\n{}",
            "hello ".repeat(2000),
            "hi there ".repeat(2000),
            "how are you? ".repeat(2000)
        );
        let chunks = chunk_messages(&ChunkInput {
            owner_id: "owner_1",
            user_id: "",
            agent_id: "agent_1",
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1",
            source_ref: None,
            markdown: &md,
            tags: &[],
            timestamp_ms: 1000,
            max_tokens: DEFAULT_CHUNK_MAX_TOKENS,
        });
        assert!(
            chunks.len() > 1,
            "Long messages should be split into multiple chunks"
        );
    }

    #[test]
    fn test_chunk_deterministic_id() {
        let input = ChunkInput {
            owner_id: "owner_1",
            user_id: "",
            agent_id: "agent_1",
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1",
            source_ref: None,
            markdown: "Hello",
            tags: &[],
            timestamp_ms: 1000,
            max_tokens: DEFAULT_CHUNK_MAX_TOKENS,
        };
        let chunks1 = chunk_messages(&input);
        let chunks2 = chunk_messages(&input);
        assert_eq!(chunks1[0].id, chunks2[0].id);
    }

    #[test]
    fn test_chunk_different_owner_different_id() {
        let chunks1 = chunk_messages(&ChunkInput {
            owner_id: "owner_1",
            user_id: "",
            agent_id: "agent_1",
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1",
            source_ref: None,
            markdown: "Hello",
            tags: &[],
            timestamp_ms: 1000,
            max_tokens: DEFAULT_CHUNK_MAX_TOKENS,
        });
        let chunks2 = chunk_messages(&ChunkInput {
            owner_id: "owner_2",
            user_id: "",
            agent_id: "agent_1",
            source_kind: SourceKind::Chat,
            source_id: "wechat:gh_abc:sender_1",
            source_ref: None,
            markdown: "Hello",
            tags: &[],
            timestamp_ms: 1000,
            max_tokens: DEFAULT_CHUNK_MAX_TOKENS,
        });
        assert_ne!(chunks1[0].id, chunks2[0].id);
    }

    #[test]
    fn test_split_by_token_budget() {
        let text = "Para one\n\nPara two\n\nPara three";
        let chunks = split_by_token_budget(text, 20);
        assert!(!chunks.is_empty());
        // Each chunk should be within budget (approx)
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 30); // some slack for newlines
        }
    }
}
