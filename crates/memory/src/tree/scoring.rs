//! Score-based chunk admission: 6 signals + optional LLM importance.

use super::chunker::approx_token_count;
use super::types::{
    ScoreSignals, SignalWeights, SourceKind, DEFAULT_DEFINITE_DROP, DEFAULT_DEFINITE_KEEP,
    DEFAULT_DROP_THRESHOLD,
};

/// Admission decision for a chunk.
#[derive(Debug, Clone)]
pub struct AdmissionDecision {
    pub signals: ScoreSignals,
    pub total: f32,
    pub admitted: bool,
    pub dropped: bool,
    pub reason: String,
}

/// Compute the 6 cheap signals for a chunk.
pub fn compute_cheap_signals(
    content: &str,
    source_kind: SourceKind,
    tags: &[String],
    entity_count: usize,
) -> ScoreSignals {
    let token_count_signal = signal_token_count(content);
    let unique_words = signal_unique_words(content);
    let metadata_weight = signal_metadata_weight(source_kind);
    let source_weight = signal_source_weight(source_kind, tags);
    let interaction = signal_interaction(tags);
    let entity_density = signal_entity_density(entity_count, content);

    ScoreSignals {
        token_count: token_count_signal,
        unique_words,
        metadata_weight,
        source_weight,
        interaction,
        entity_density,
        llm_importance: 0.0,
    }
}

/// Combine signals using weights to produce a total score.
pub fn combine_signals(signals: &ScoreSignals, weights: &SignalWeights) -> f32 {
    let numerator = signals.token_count * weights.token_count
        + signals.unique_words * weights.unique_words
        + signals.metadata_weight * weights.metadata_weight
        + signals.source_weight * weights.source_weight
        + signals.interaction * weights.interaction
        + signals.entity_density * weights.entity_density
        + signals.llm_importance * weights.llm_importance;

    let denominator = weights.token_count
        + weights.unique_words
        + weights.metadata_weight
        + weights.source_weight
        + weights.interaction
        + weights.entity_density
        + weights.llm_importance;

    if denominator == 0.0 {
        return 0.5;
    }

    (numerator / denominator).clamp(0.0, 1.0)
}

/// Make an admission decision based on cheap-only signals.
pub fn score_chunk(
    content: &str,
    source_kind: SourceKind,
    tags: &[String],
    entity_count: usize,
) -> AdmissionDecision {
    let signals = compute_cheap_signals(content, source_kind, tags, entity_count);
    let weights = SignalWeights::default();
    let total = combine_signals(&signals, &weights);

    // Guard: tiny chunks with no entities are always dropped
    let token_count = approx_token_count(content);
    if token_count < 10 && entity_count == 0 {
        return AdmissionDecision {
            signals,
            total,
            admitted: false,
            dropped: true,
            reason: "tiny_chunk_no_entities".to_string(),
        };
    }

    if total >= DEFAULT_DEFINITE_KEEP {
        AdmissionDecision {
            signals,
            total,
            admitted: true,
            dropped: false,
            reason: "definite_keep".to_string(),
        }
    } else if total <= DEFAULT_DEFINITE_DROP {
        AdmissionDecision {
            signals,
            total,
            admitted: false,
            dropped: true,
            reason: "definite_drop".to_string(),
        }
    } else if total < DEFAULT_DROP_THRESHOLD {
        AdmissionDecision {
            signals,
            total,
            admitted: false,
            dropped: true,
            reason: "below_drop_threshold".to_string(),
        }
    } else {
        // Borderline — admitted but could benefit from LLM scoring later
        AdmissionDecision {
            signals,
            total,
            admitted: true,
            dropped: false,
            reason: "borderline_admitted".to_string(),
        }
    }
}

// -- Individual signal functions --

/// Token count signal: trapezoidal shape.
fn signal_token_count(content: &str) -> f32 {
    let tokens = approx_token_count(content);
    if tokens < 10 {
        0.0
    } else if tokens < 30 {
        (tokens as f32 - 10.0) / 20.0
    } else if tokens <= 3000 {
        1.0
    } else if tokens <= 8000 {
        1.0 - 0.5 * (tokens as f32 - 3000.0) / 5000.0
    } else {
        0.5
    }
}

/// Unique words signal: type-token ratio.
fn signal_unique_words(content: &str) -> f32 {
    let words: Vec<&str> = content
        .split_whitespace()
        .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|w| !w.is_empty())
        .collect();

    if words.len() < 5 {
        return 0.5;
    }

    let mut unique = std::collections::HashSet::new();
    for word in &words {
        unique.insert(word.to_lowercase());
    }

    let ratio = unique.len() as f32 / words.len() as f32;
    if ratio <= 0.3 {
        0.0
    } else if ratio >= 0.7 {
        1.0
    } else {
        (ratio - 0.3) / 0.4
    }
}

/// Metadata weight: fixed per source kind.
fn signal_metadata_weight(source_kind: SourceKind) -> f32 {
    match source_kind {
        SourceKind::Chat => 0.5,
        SourceKind::Email => 0.8,
        SourceKind::Document => 0.9,
    }
}

/// Source weight: per-provider or per-kind defaults.
fn signal_source_weight(source_kind: SourceKind, tags: &[String]) -> f32 {
    // Check for provider tags first
    for tag in tags {
        let lower = tag.to_lowercase();
        if lower.starts_with("provider:") {
            let provider = lower.strip_prefix("provider:").unwrap_or("");
            return match provider {
                "gmail" => 0.8,
                "whatsapp" => 0.75,
                "telegram" => 0.6,
                "discord" => 0.5,
                "notion" => 0.75,
                "wechat" | "weixin" => 0.75,
                "feishu" => 0.7,
                "wecom" => 0.7,
                "dingtalk" => 0.6,
                _ => 0.6,
            };
        }
    }

    // Fall back to kind-level defaults
    match source_kind {
        SourceKind::Email => 0.75,
        SourceKind::Document => 0.7,
        SourceKind::Chat => 0.5,
    }
}

/// Interaction signal: based on engagement tags.
fn signal_interaction(tags: &[String]) -> f32 {
    let mut score = 0.0f32;
    for tag in tags {
        let lower = tag.to_lowercase();
        if lower == "sent" {
            score += 0.6;
        } else if lower == "reply" {
            score += 0.5;
        } else if lower == "dm" {
            score += 0.3;
        } else if lower == "mention" {
            score += 0.2;
        }
    }
    if score == 0.0 {
        0.5 // neutral
    } else {
        score.min(1.0)
    }
}

/// Entity density signal: entities per 100 tokens.
fn signal_entity_density(entity_count: usize, content: &str) -> f32 {
    let tokens = approx_token_count(content);
    if tokens == 0 {
        return 0.0;
    }
    (entity_count as f32 / tokens as f32 / 0.01).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_token_count() {
        assert!((signal_token_count("") - 0.0).abs() < 0.01);
        assert!((signal_token_count("short") - 0.0).abs() < 0.01); // ~2 tokens, below 10
        assert!((signal_token_count(&"hello ".repeat(1000)) - 1.0).abs() < 0.01);
        // ~250 tokens -> plateau
    }

    #[test]
    fn test_signal_unique_words() {
        // Highly repetitive
        let repetitive = "hello hello hello hello hello hello";
        assert!(signal_unique_words(repetitive) < 0.3);

        // Diverse
        let diverse =
            "the quick brown fox jumps over the lazy dog and then some more words here we go";
        assert!(signal_unique_words(diverse) > 0.5);

        // Too few words
        assert!((signal_unique_words("hi") - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_signal_metadata_weight() {
        assert!((signal_metadata_weight(SourceKind::Chat) - 0.5).abs() < 0.01);
        assert!((signal_metadata_weight(SourceKind::Email) - 0.8).abs() < 0.01);
        assert!((signal_metadata_weight(SourceKind::Document) - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_score_chunk_definite_keep() {
        let content = "Important project discussion with Alice about the new feature. ".repeat(20);
        let decision = score_chunk(&content, SourceKind::Email, &[], 3);
        assert!(decision.admitted);
        assert!(!decision.dropped);
    }

    #[test]
    fn test_score_chunk_tiny_no_entities() {
        let decision = score_chunk("hi", SourceKind::Chat, &[], 0);
        assert!(!decision.admitted);
        assert!(decision.dropped);
        assert_eq!(decision.reason, "tiny_chunk_no_entities");
    }

    #[test]
    fn test_combine_signals_default_weights() {
        let signals = ScoreSignals {
            token_count: 1.0,
            unique_words: 0.8,
            metadata_weight: 0.5,
            source_weight: 0.5,
            interaction: 0.5,
            entity_density: 0.3,
            llm_importance: 0.0,
        };
        let weights = SignalWeights::default();
        let total = combine_signals(&signals, &weights);
        assert!(total > 0.0 && total <= 1.0);
    }
}
