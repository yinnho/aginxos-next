//! LLM driver implementations.
//!
//! All LLM calls are HTTP API drivers handled by `UnifiedHttpDriver` in
//! `llm_driver_impl.rs` (OpenAI format via aginxbrain). The CLI subprocess
//! drivers (claude-code, qwen-code) and the fallback chain driver were removed
//! once aginxbrain began proxying every LLM call.

use crate::llm_driver::{DriverConfig, LlmDriver, LlmError};
use std::sync::Arc;

/// Create an LLM driver based on configuration.
///
/// Thin facade over `llm_driver::create_driver()`; kept as a stable import path.
pub fn create_driver(config: &DriverConfig) -> Result<Arc<dyn LlmDriver>, LlmError> {
    crate::llm_driver::create_driver(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_driver_with_key_and_url() {
        let config = DriverConfig {
            api_key: Some("test-key".to_string()),
            base_url: Some("https://brain.aginx.net/v1/chat/completions".to_string()),
        };
        let driver = create_driver(&config);
        assert!(driver.is_ok(), "HTTP driver with key + URL should succeed");
    }

    #[test]
    fn test_http_driver_no_key_succeeds() {
        // HTTP driver does not require API key (e.g. local aginxbrain)
        let config = DriverConfig {
            api_key: None,
            base_url: Some("http://localhost:8080/v1/chat/completions".to_string()),
        };
        let driver = create_driver(&config);
        assert!(
            driver.is_ok(),
            "HTTP driver without key should succeed (local providers)"
        );
    }

    #[test]
    fn test_http_driver_no_url_errors() {
        let config = DriverConfig {
            api_key: Some("test-key".to_string()),
            base_url: None,
        };
        let result = create_driver(&config);
        assert!(result.is_err(), "HTTP driver without URL should error");
        let err = result.err().unwrap().to_string();
        assert!(
            err.contains("base_url"),
            "Error should mention base_url: {}",
            err
        );
    }
}
