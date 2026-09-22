//! Brain configuration types — the carrier's LLM brain.
//!
//! Single-layer architecture: all LLM traffic goes through aginxbrain
//! (OpenAI-compatible proxy). The brain config just declares:
//! - Where to call (base_url)
//! - How to authenticate (api_key_env)
//! - Which modalities are supported (chat/reasoning/image/...)
//!
//! The modality name is sent as the `model` field to aginxbrain, which
//! routes it to the appropriate upstream provider and handles internal
//! fallback. OpenCarrier does NOT configure fallbacks — that is
//! aginxbrain's responsibility.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level brain configuration, deserialized from `brain.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainConfig {
    /// Complete API base URL (e.g. "https://brain.aginx.net/v1/chat/completions").
    /// All modalities share this single endpoint — aginxbrain routes by model tag.
    pub base_url: String,
    /// Environment variable name holding the API key.
    /// If empty, no authentication header is sent.
    #[serde(default)]
    pub api_key_env: String,
    /// Default modality when an agent doesn't specify one.
    #[serde(default = "default_modality")]
    pub default_modality: String,
    /// Supported modalities: name → description.
    /// The name doubles as the `model` routing tag sent to aginxbrain.
    #[serde(default)]
    pub modalities: HashMap<String, ModalityEntry>,
}

fn default_modality() -> String {
    "chat".to_string()
}

/// A single modality entry — just a description.
///
/// The modality's name (the HashMap key) is the routing tag sent to
/// aginxbrain as the `model` field. No primary/fallbacks here —
/// aginxbrain owns the fallback chain internally.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModalityEntry {
    /// Human-readable description of this modality.
    #[serde(default)]
    pub description: String,
}

// ---------------------------------------------------------------------------
// Brain query types — returned by the Brain trait methods
// ---------------------------------------------------------------------------

/// A resolved endpoint returned by `Brain::endpoints_for()`.
///
/// In the single-layer model there is exactly one endpoint (the shared
/// aginxbrain URL), so this is always a single-element vec. Kept as a
/// vec so the existing fallback-iteration code in call_with_fallback /
/// Brain::complete() works unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEndpoint {
    /// Endpoint id — equals the modality name (used as health-tracking key).
    pub id: String,
    /// Model name to set in CompletionRequest.model (== modality name).
    pub model: String,
    /// Provider name (always "aginxbrain" now, kept for logging).
    pub provider: String,
}

/// Information about a modality, returned by `Brain::list_modalities()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModalityInfo {
    /// Modality name (e.g., "chat", "fast", "vision").
    pub name: String,
    /// Human-readable description.
    pub description: String,
}

/// Feedback from the runtime to Brain after an endpoint call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointReport {
    /// Which modality was attempted (== endpoint id).
    pub endpoint_id: String,
    /// Whether the call succeeded.
    pub success: bool,
    /// Call latency in milliseconds.
    pub latency_ms: u64,
    /// Error message if the call failed.
    pub error: Option<String>,
}

/// Health status of a single modality.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointHealth {
    /// Modality name (== endpoint id).
    pub endpoint: String,
    /// Provider name (always "aginxbrain").
    pub provider: String,
    /// Model name (== modality name).
    pub model: String,
    /// Whether the shared driver was successfully created at boot.
    pub driver_ready: bool,
    /// Total successful calls (from report()).
    pub success_count: u64,
    /// Total failed calls (from report()).
    pub failure_count: u64,
    /// Average latency in ms (0 if no data).
    pub avg_latency_ms: u64,
    /// Consecutive failures (reset to 0 on success).
    pub consecutive_failures: u32,
    /// Whether the circuit-breaker has opened (modality taken out of rotation).
    pub circuit_open: bool,
}

/// Overall Brain status snapshot, returned by `Brain::status()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrainStatus {
    /// All modalities.
    pub modalities: Vec<ModalityInfo>,
    /// Health of all modalities.
    pub endpoints: Vec<EndpointHealth>,
    /// Whether the shared driver initialized successfully (0 or 1).
    pub drivers_ready: usize,
}

/// Resolved credentials for the brain, ready for injection into skill subprocess.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderCredentials {
    /// Provider name (always "aginxbrain").
    pub provider_name: String,
    /// Environment variable name → resolved value pairs.
    pub env_vars: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_brain_config_parse() {
        let json = r#"{
            "base_url": "https://brain.aginx.net/v1/chat/completions",
            "api_key_env": "AGINXBRAIN_API_KEY",
            "default_modality": "chat",
            "modalities": {
                "chat": { "description": "General chat" },
                "reasoning": { "description": "Reasoning" },
                "image": {}
            }
        }"#;

        let config: BrainConfig = serde_json::from_str(json).unwrap();

        assert_eq!(
            config.base_url,
            "https://brain.aginx.net/v1/chat/completions"
        );
        assert_eq!(config.api_key_env, "AGINXBRAIN_API_KEY");
        assert_eq!(config.default_modality, "chat");
        assert_eq!(config.modalities.len(), 3);
        assert_eq!(config.modalities["chat"].description, "General chat");
        assert!(config.modalities["image"].description.is_empty());
    }

    #[test]
    fn test_brain_config_minimal() {
        // Only base_url required; everything else defaults.
        let json = r#"{ "base_url": "http://localhost:8080/v1/chat" }"#;
        let config: BrainConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.base_url, "http://localhost:8080/v1/chat");
        assert!(config.api_key_env.is_empty());
        assert_eq!(config.default_modality, "chat");
        assert!(config.modalities.is_empty());
    }
}
