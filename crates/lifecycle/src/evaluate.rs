//! Clone quality evaluation — deterministic metrics + LLM-based assessment.
//!
//! Evaluates a clone's quality from two angles:
//! 1. **Deterministic metrics** (zero cost): file counts, sizes, coverage ratios
//! 2. **LLM assessment** (needs kernel to call LLM): generate test questions
//!    from knowledge, judge answer quality
//!
//! The kernel calls `compute_deterministic_metrics()` for instant results,
//! then optionally uses `build_test_questions_prompt()` → LLM → more evaluation.

use serde::Serialize;
use std::fs;
use std::path::Path;

/// Deterministic quality metrics (computed without LLM).
#[derive(Debug, Clone, Serialize)]
pub struct QualityMetrics {
    /// Number of knowledge files.
    pub knowledge_files: usize,
    /// Total bytes of knowledge content.
    pub knowledge_total_bytes: usize,
    /// Number of flows.
    pub flow_count: usize,
    /// Whether SOUL.md exists.
    pub has_soul: bool,
    /// Whether system_prompt.md exists.
    pub has_system_prompt: bool,
    /// Whether MEMORY.md exists.
    pub has_memory: bool,
    /// system_prompt.md length in bytes.
    pub system_prompt_len: usize,
    /// Number of knowledge files missing frontmatter.
    pub files_missing_frontmatter: usize,
    /// Number of knowledge files missing description.
    pub files_missing_description: usize,
    /// Number of files with EXTRACTED confidence.
    pub confidence_extracted: usize,
    /// Number of files with INFERRED confidence.
    pub confidence_inferred: usize,
    /// Number of files with AMBIGUOUS confidence.
    pub confidence_ambiguous: usize,
    /// Overall deterministic score (0-100).
    pub score: u32,
    /// Letter grade.
    pub grade: String,
}

/// Result of an LLM-based evaluation question.
#[derive(Debug, Clone, Serialize)]
pub struct EvalQuestion {
    /// The test question.
    pub question: String,
    /// Score given by LLM judge (0-10).
    pub score: f32,
    /// Judge's feedback.
    pub feedback: String,
}

/// Full evaluation report combining deterministic and LLM results.
#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    /// Deterministic metrics.
    pub metrics: QualityMetrics,
    /// LLM test results (empty if LLM evaluation was not run).
    pub questions: Vec<EvalQuestion>,
    /// Average LLM score (0-10), if questions were run.
    pub avg_llm_score: Option<f32>,
}

/// Compute deterministic quality metrics for a clone workspace.
pub fn compute_deterministic_metrics(workspace: &Path) -> QualityMetrics {
    let knowledge_dir = workspace.join("knowledge");
    let flows_dir = workspace.join("flows");

    // Knowledge files
    let mut knowledge_files = 0usize;
    let mut knowledge_total_bytes = 0usize;
    let mut files_missing_frontmatter = 0usize;
    let mut files_missing_description = 0usize;
    let mut confidence_extracted = 0usize;
    let mut confidence_inferred = 0usize;
    let mut confidence_ambiguous = 0usize;

    if knowledge_dir.exists() {
        if let Ok(entries) = fs::read_dir(&knowledge_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().map(|e| e == "md").unwrap_or(false) {
                    knowledge_files += 1;
                    if let Ok(content) = fs::read_to_string(&path) {
                        knowledge_total_bytes += content.len();
                        if !content.starts_with("---") {
                            files_missing_frontmatter += 1;
                        } else {
                            if !content.contains("description:") {
                                files_missing_description += 1;
                            }
                            // Count confidence levels
                            if content.contains("confidence: EXTRACTED") {
                                confidence_extracted += 1;
                            } else if content.contains("confidence: INFERRED") {
                                confidence_inferred += 1;
                            } else if content.contains("confidence: AMBIGUOUS") {
                                confidence_ambiguous += 1;
                            } else {
                                // No confidence field — treat as EXTRACTED (legacy)
                                confidence_extracted += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // Skills
    let flow_count = if flows_dir.exists() {
        fs::read_dir(&flows_dir)
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().extension().map(|ext| ext == "md").unwrap_or(false)
                            || e.path().is_dir()
                                && (e.path().join("flow.md").exists()
                                    || e.path().join("SKILL.md").exists())
                    })
                    .count()
            })
            .unwrap_or(0)
    } else {
        0
    };

    // Identity files
    let has_soul = workspace.join("SOUL.md").exists();
    let has_system_prompt = workspace.join("system_prompt.md").exists();
    let has_memory = workspace.join("MEMORY.md").exists();
    let system_prompt_len = fs::read_to_string(workspace.join("system_prompt.md"))
        .map(|c| c.len())
        .unwrap_or(0);

    // Score calculation (0-100)
    let mut score = 0u32;

    // Identity completeness (max 40 points)
    if has_soul {
        score += 15;
    }
    if has_system_prompt {
        score += 15;
    }
    if has_memory {
        score += 10;
    }

    // Knowledge richness (max 30 points)
    if knowledge_files > 0 {
        score += 10;
    }
    if knowledge_files >= 3 {
        score += 10;
    }
    if knowledge_total_bytes > 500 {
        score += 10;
    }

    // Skills (max 15 points)
    if flow_count > 0 {
        score += 5;
    }
    if flow_count >= 3 {
        score += 5;
    }
    if flow_count >= 5 {
        score += 5;
    }

    // Knowledge quality (max 15 points)
    if knowledge_files > 0 && files_missing_frontmatter == 0 {
        score += 8;
    }
    if knowledge_files > 0 && files_missing_description == 0 {
        score += 7;
    }

    // Confidence bonus (max 5 points) — reward verified knowledge
    if knowledge_files > 0 {
        let extracted_ratio = confidence_extracted as f32 / knowledge_files as f32;
        if extracted_ratio >= 0.8 {
            score += 5;
        } else if extracted_ratio >= 0.5 {
            score += 3;
        } else if confidence_ambiguous == 0 {
            score += 1;
        }
    }

    let grade = match score {
        s if s >= 80 => "优秀".to_string(),
        s if s >= 60 => "良好".to_string(),
        s if s >= 40 => "及格".to_string(),
        _ => "需改进".to_string(),
    };

    QualityMetrics {
        knowledge_files,
        knowledge_total_bytes,
        flow_count,
        has_soul,
        has_system_prompt,
        has_memory,
        system_prompt_len,
        files_missing_frontmatter,
        files_missing_description,
        confidence_extracted,
        confidence_inferred,
        confidence_ambiguous,
        score,
        grade,
    }
}

/// Build the LLM prompt for generating test questions from knowledge files.
///
/// Returns (system_prompt, user_prompt).
pub fn build_test_questions_prompt(knowledge_content: &str) -> (String, String) {
    let system = r#"你是测试问题生成器。根据提供的知识内容，生成 5 个能验证这些知识是否被正确理解和应用的测试问题。

要求：
1. 问题应该测试对知识的理解，而不是简单复述
2. 包含 2 个直接知识问题 + 2 个应用推理问题 + 1 个边界/错误情况问题
3. 问题要具体，有明确答案

只返回问题列表，每行一个问题，不要编号或其他格式。"#.to_string();

    let user = if knowledge_content.len() > 6000 {
        let end = knowledge_content.floor_char_boundary(6000);
        format!("{}...\n(已截断)", &knowledge_content[..end])
    } else {
        knowledge_content.to_string()
    };

    (system, user)
}

/// Parse the LLM test questions response into a list of questions.
pub fn parse_test_questions(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| {
            l.trim()
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
                .to_string()
        })
        .filter(|l| !l.is_empty() && l.len() > 5)
        .take(5)
        .collect()
}

/// Build the LLM prompt for judging an answer's quality.
pub fn build_judge_prompt(question: &str, answer: &str) -> (String, String) {
    let system = r#"你是回答质量评判器。评估以下回答的质量。

返回格式（严格 JSON）：
{"score": 7.5, "feedback": "简短评价"}

评分标准：
- 8-10: 准确、完整、有洞察
- 6-8: 基本正确，有小瑕疵
- 4-6: 部分正确，有遗漏或偏差
- 0-4: 错误或无意义

只返回 JSON，不要其他内容。"#
        .to_string();

    let user = format!("问题: {}\n\n回答: {}", question, answer);
    (system, user)
}

/// Parse the LLM judge response into (score, feedback).
pub fn parse_judge_response(text: &str) -> (f32, String) {
    let json_text = extract_json(text);

    match serde_json::from_str::<serde_json::Value>(&json_text) {
        Ok(v) => {
            let score = v["score"].as_f64().unwrap_or(5.0) as f32;
            let feedback = v["feedback"].as_str().unwrap_or("无法解析评价").to_string();
            (score, feedback)
        }
        Err(_) => {
            // Try to extract score from text
            let score = text
                .lines()
                .find_map(|line| {
                    line.split(':')
                        .nth(1)
                        .and_then(|s| s.trim().parse::<f32>().ok())
                })
                .unwrap_or(5.0);
            (score, text.chars().take(100).collect())
        }
    }
}

/// Read all knowledge content for evaluation (compiled truth only).
pub fn read_knowledge_for_eval(workspace: &Path) -> String {
    let knowledge_dir = workspace.join("knowledge");
    if !knowledge_dir.exists() {
        return String::new();
    }

    let mut content = String::new();
    if let Ok(entries) = fs::read_dir(&knowledge_dir) {
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
            if let Ok(file_content) = fs::read_to_string(&path) {
                let body = strip_frontmatter(&file_content);
                if !body.trim().is_empty() {
                    content.push_str(&format!("### {}\n{}\n\n", name, body));
                }
            }
        }
    }
    content
}

/// Strip YAML frontmatter from content.
fn strip_frontmatter(content: &str) -> &str {
    let rest = match content.strip_prefix("---") {
        Some(r) => r,
        None => return content,
    };
    match rest.find("---") {
        Some(end) => &rest[end + 3..],
        None => content,
    }
}

/// Extract JSON object from text.
fn extract_json(text: &str) -> String {
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

    fn setup_workspace(tmp: &TempDir) -> &Path {
        let ws = tmp.path();
        fs::create_dir_all(ws.join("knowledge")).unwrap();
        fs::create_dir_all(ws.join("flows")).unwrap();
        ws
    }

    #[test]
    fn test_metrics_empty_workspace() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        let metrics = compute_deterministic_metrics(ws);
        assert_eq!(metrics.knowledge_files, 0);
        assert!(!metrics.has_soul);
        assert!(!metrics.has_system_prompt);
        assert!(metrics.score < 40);
    }

    #[test]
    fn test_metrics_full_workspace() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(ws.join("SOUL.md"), "soul content").unwrap();
        fs::write(ws.join("system_prompt.md"), "be helpful").unwrap();
        fs::write(ws.join("MEMORY.md"), "# Index").unwrap();

        fs::write(
            ws.join("knowledge/policy.md"),
            "---\nname: policy\ndescription: refund policy\n---\n\nRefund within 7 days.",
        )
        .unwrap();
        fs::write(
            ws.join("knowledge/faq.md"),
            "---\nname: faq\ndescription: common questions\n---\n\nFAQ content here.",
        )
        .unwrap();
        fs::write(ws.join("flows/greet.md"), "---\n---\nGreet the user").unwrap();

        let metrics = compute_deterministic_metrics(ws);
        assert!(metrics.has_soul);
        assert!(metrics.has_system_prompt);
        assert!(metrics.has_memory);
        assert_eq!(metrics.knowledge_files, 2);
        assert_eq!(metrics.flow_count, 1);
        assert_eq!(metrics.files_missing_frontmatter, 0);
        assert!(metrics.score >= 60);
    }

    #[test]
    fn test_build_test_questions_prompt() {
        let (sys, user) = build_test_questions_prompt("Some knowledge content here");
        assert!(sys.contains("测试问题"));
        assert!(user.contains("knowledge"));
    }

    #[test]
    fn test_parse_test_questions() {
        let text = "1. What is the refund policy?\n2. How do returns work?\n3. Edge case question?\n4. Another question?\n5. Last one?";
        let questions = parse_test_questions(text);
        assert_eq!(questions.len(), 5);
        assert_eq!(questions[0], "What is the refund policy?");
    }

    #[test]
    fn test_parse_test_questions_filters_short() {
        let text = "ok\n\nWhat is the full refund policy for items over $100?";
        let questions = parse_test_questions(text);
        assert_eq!(questions.len(), 1);
    }

    #[test]
    fn test_build_judge_prompt() {
        let (sys, user) = build_judge_prompt("What is X?", "X is Y.");
        assert!(sys.contains("JSON"));
        assert!(user.contains("What is X?"));
    }

    #[test]
    fn test_parse_judge_response_valid() {
        let json = r#"{"score": 8.5, "feedback": "准确完整"}"#;
        let (score, feedback) = parse_judge_response(json);
        assert!((score - 8.5).abs() < 0.01);
        assert_eq!(feedback, "准确完整");
    }

    #[test]
    fn test_parse_judge_response_invalid() {
        let (score, _) = parse_judge_response("not json");
        assert_eq!(score, 5.0); // default
    }

    #[test]
    fn test_read_knowledge_for_eval() {
        let tmp = TempDir::new().unwrap();
        let ws = setup_workspace(&tmp);

        fs::write(
            ws.join("knowledge/test.md"),
            "---\nname: test\n---\n\nContent here.",
        )
        .unwrap();

        let content = read_knowledge_for_eval(ws);
        assert!(content.contains("Content here"));
        assert!(!content.contains("name: test")); // frontmatter stripped
    }
}
