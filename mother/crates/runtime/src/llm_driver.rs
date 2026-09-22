//! LLM driver trait and types.
//!
//! Abstracts over multiple LLM providers (Anthropic, OpenAI, Ollama, etc.).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use carrier_types::brain::EndpointReport;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::media::MediaOutput;
use carrier_types::message::{ContentBlock, Message, StopReason, TokenUsage};
use carrier_types::tool::{ToolCall, ToolDefinition};

/// Error type for LLM driver operations.
#[derive(Error, Debug)]
pub enum LlmError {
    /// HTTP request failed.
    #[error("HTTP error: {0}")]
    Http(String),
    /// API returned an error.
    #[error("API error ({status}): {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message from the API.
        message: String,
    },
    /// Rate limited — should retry after delay.
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited {
        /// How long to wait before retrying.
        retry_after_ms: u64,
    },
    /// Response parsing failed.
    #[error("Parse error: {0}")]
    Parse(String),
    /// No API key configured.
    #[error("Missing API key: {0}")]
    MissingApiKey(String),
    /// Model overloaded.
    #[error("Model overloaded, retry after {retry_after_ms}ms")]
    Overloaded {
        /// How long to wait before retrying.
        retry_after_ms: u64,
    },
    /// Authentication failed (invalid/missing API key).
    #[error("Authentication failed: {0}")]
    AuthenticationFailed(String),
    /// Model not found.
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    /// Configuration error.
    #[error("Configuration error: {0}")]
    Config(String),
}

impl LlmError {
    /// Returns true if this error type is worth retrying on a different driver.
    ///
    /// Non-retryable errors (auth failure, model not found, missing key) will
    /// fail identically on every driver in a fallback chain, so there's no
    /// point trying the next one.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            LlmError::RateLimited { .. }
                | LlmError::Overloaded { .. }
                | LlmError::Http(_)
                | LlmError::Api { .. }
                | LlmError::Parse(_)
        )
    }
}

/// A request to an LLM for completion.
#[derive(Debug, Clone, Default)]
pub struct CompletionRequest {
    /// Model identifier.
    pub model: String,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Available tools the model can use.
    pub tools: Vec<ToolDefinition>,
    /// Maximum tokens to generate.
    pub max_tokens: u32,
    /// Sampling temperature.
    pub temperature: f32,
    /// System prompt (extracted from messages for APIs that need it separately).
    pub system: Option<String>,
    /// Extended thinking configuration (if supported by the model).
    pub thinking: Option<carrier_types::config::ThinkingConfig>,
    /// Modality-specific extra parameters (voice, size, resolution, etc.).
    /// Ignored by standard LLM drivers; used by media drivers.
    pub extra: serde_json::Value,
}

/// A response from an LLM completion.
#[derive(Debug, Clone, Default)]
pub struct CompletionResponse {
    /// The content blocks in the response.
    pub content: Vec<ContentBlock>,
    /// Why the model stopped generating.
    pub stop_reason: StopReason,
    /// Tool calls extracted from the response.
    pub tool_calls: Vec<ToolCall>,
    /// Token usage statistics.
    pub usage: TokenUsage,
    /// Media output (audio, image, video) from non-LLM drivers.
    /// Set by TTS/image/video drivers; `None` for standard text completions.
    pub media: Option<MediaOutput>,
}

impl CompletionResponse {
    /// Extract text content from the response.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                ContentBlock::Thinking { .. } => None,
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

/// Events emitted during streaming LLM completion.
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Incremental text content.
    TextDelta { text: String },
    /// A tool use block has started.
    ToolUseStart { id: String, name: String },
    /// Incremental JSON input for an in-progress tool use.
    ToolInputDelta { text: String },
    /// A tool use block is complete with parsed input.
    ToolUseEnd {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Incremental thinking/reasoning text.
    ThinkingDelta { text: String },
    /// The entire response is complete.
    ContentComplete {
        stop_reason: StopReason,
        usage: TokenUsage,
    },
    /// Agent lifecycle phase change (for UX indicators).
    PhaseChange {
        phase: String,
        detail: Option<String>,
    },
    /// Tool execution completed with result (emitted by agent loop, not LLM driver).
    ToolExecutionResult {
        id: String,
        name: String,
        result_preview: String,
        is_error: bool,
    },
}

/// Trait for LLM drivers.
#[async_trait]
pub trait LlmDriver: Send + Sync {
    /// Send a completion request and get a response.
    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse, LlmError>;

    /// Stream a completion request, sending incremental events to the channel.
    /// Returns the full response when complete. Default wraps `complete()`.
    async fn stream(
        &self,
        request: CompletionRequest,
        tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> Result<CompletionResponse, LlmError> {
        let response = self.complete(request).await?;
        let text = response.text();
        if !text.is_empty() {
            let _ = tx.send(StreamEvent::TextDelta { text }).await;
        }
        let _ = tx
            .send(StreamEvent::ContentComplete {
                stop_reason: response.stop_reason,
                usage: response.usage,
            })
            .await;
        Ok(response)
    }
}

/// Brain trait — the carrier's independent LLM brain.
///
/// Pure query service: provides endpoint information and health tracking.
/// The runtime handles all execution and fallback logic.
///
/// Implemented by `kernel::brain::Brain`.
#[async_trait]
pub trait Brain: Send + Sync {
    // --- New query interface ---

    /// List all available modalities with descriptions.
    fn list_modalities(&self) -> Vec<carrier_types::brain::ModalityInfo> {
        vec![]
    }

    /// Get the ordered list of resolved endpoints for a modality.
    /// Returns primary first, then fallbacks in order.
    /// Returns an empty Vec if the modality is unknown.
    fn endpoints_for(&self, _modality: &str) -> Vec<carrier_types::brain::ResolvedEndpoint> {
        vec![]
    }

    /// Get a driver for a specific endpoint. Returns None if the endpoint
    /// has no driver (initialization failed at boot).
    fn driver_for_endpoint(&self, _endpoint_id: &str) -> Option<Arc<dyn LlmDriver>> {
        None
    }

    /// Report the result of an endpoint call. Non-blocking.
    fn report(&self, _report: carrier_types::brain::EndpointReport) {}

    /// Get current Brain status snapshot.
    fn status(&self) -> carrier_types::brain::BrainStatus {
        carrier_types::brain::BrainStatus {
            modalities: vec![],
            endpoints: vec![],
            drivers_ready: 0,
        }
    }

    /// Resolve credentials for a provider (for skill credential injection).
    fn credentials_for(&self, _provider: &str) -> Option<carrier_types::brain::ProviderCredentials> {
        None
    }

    /// Execute a completion through the Brain's endpoint resolution and fallback chain.
    ///
    /// Resolves endpoints for the given modality, tries them in order (primary first,
    /// then fallbacks), reports success/failure for circuit-breaker health tracking,
    /// and returns the first successful response.
    ///
    /// This is the primary API for tools that need to call an LLM for a subtask
    /// (vision understanding, transcription, image generation, etc.).
    async fn complete(
        &self,
        modality: &str,
        mut request: CompletionRequest,
    ) -> CarrierResult<CompletionResponse> {
        let endpoints = self.endpoints_for(modality);
        if endpoints.is_empty() {
            return Err(CarrierError::LlmDriver(format!(
                "No endpoints available for modality '{modality}' (driver creation failed or circuit-broken)"
            )));
        }

        let mut last_error: Option<String> = None;
        for (i, ep) in endpoints.iter().enumerate() {
            let Some(driver) = self.driver_for_endpoint(&ep.id) else {
                continue;
            };
            request.model = ep.model.clone();
            let start = std::time::Instant::now();
            match driver.complete(request.clone()).await {
                Ok(response) => {
                    let latency = start.elapsed().as_millis() as u64;
                    self.report(EndpointReport {
                        endpoint_id: ep.id.clone(),
                        success: true,
                        latency_ms: latency,
                        error: None,
                    });
                    return Ok(response);
                }
                Err(e) => {
                    let latency = start.elapsed().as_millis() as u64;
                    let err_str = format!("{e}");
                    self.report(EndpointReport {
                        endpoint_id: ep.id.clone(),
                        success: false,
                        latency_ms: latency,
                        error: Some(err_str.clone()),
                    });
                    let remaining = endpoints.len() - i - 1;
                    tracing::warn!(
                        endpoint = %ep.id,
                        modality = %modality,
                        error = %e,
                        remaining_fallbacks = remaining,
                        "Brain endpoint failed"
                    );
                    last_error = Some(err_str);
                }
            }
        }

        Err(CarrierError::LlmDriver(last_error.unwrap_or_else(|| {
            format!("All endpoints exhausted for modality '{modality}'")
        })))
    }

    // --- Legacy methods ---

    /// Get the model name for a given modality (the routing tag sent to the backend).
    fn model_for(&self, modality: &str) -> String;

    /// Check if a modality is available.
    fn has_modality(&self, modality: &str) -> bool;

    /// Poll an async task for completion (video gen, etc.).
    /// Returns the updated `CompletionResponse` with `media` set to the final
    /// result once the task completes, or an error if it fails/times out.
    /// Default: not supported.
    async fn task_status(
        &self,
        _endpoint_id: &str,
        _task_id: &str,
    ) -> CarrierResult<CompletionResponse> {
        Err(CarrierError::LlmDriver(
            "Async task polling not supported".to_string(),
        ))
    }
}

/// Configuration for creating an LLM driver.
#[derive(Clone, Serialize, Deserialize)]
pub struct DriverConfig {
    /// API key.
    pub api_key: Option<String>,
    /// Base URL — the complete API endpoint URL (no path suffix appended by drivers).
    pub base_url: Option<String>,
}

/// SECURITY: Custom Debug impl redacts the API key.
impl std::fmt::Debug for DriverConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DriverConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// Create an LLM driver based on configuration.
///
/// All drivers are HTTP API drivers — `UnifiedHttpDriver` (OpenAI format via
/// aginxbrain). The CLI subprocess drivers (claude-code, qwen-code) were removed
/// once aginxbrain began proxying every LLM call.
pub fn create_driver(config: &DriverConfig) -> Result<Arc<dyn LlmDriver>, LlmError> {
    // All HTTP API drivers — UnifiedHttpDriver (OpenAI format)
    // Validate base_url for HTTP drivers
    let base_url = config
        .base_url
        .clone()
        .ok_or_else(|| LlmError::Config("base_url required for HTTP driver".to_string()))?;

    let api_key = config.api_key.clone().unwrap_or_default();

    Ok(Arc::new(crate::llm_driver_impl::UnifiedHttpDriver::new(
        api_key, base_url,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completion_response_text() {
        let response = CompletionResponse {
            content: vec![
                ContentBlock::Text {
                    text: "Hello ".to_string(),
                    provider_metadata: None,
                },
                ContentBlock::Text {
                    text: "world!".to_string(),
                    provider_metadata: None,
                },
            ],
            stop_reason: StopReason::EndTurn,
            tool_calls: vec![],
            usage: TokenUsage::default(),
            media: None,
        };
        assert_eq!(response.text(), "Hello world!");
    }

    #[test]
    fn test_stream_event_clone() {
        let event = StreamEvent::TextDelta {
            text: "hello".to_string(),
        };
        let cloned = event.clone();
        assert!(matches!(cloned, StreamEvent::TextDelta { text } if text == "hello"));
    }

    #[test]
    fn test_stream_event_variants() {
        let events: Vec<StreamEvent> = vec![
            StreamEvent::TextDelta {
                text: "hi".to_string(),
            },
            StreamEvent::ToolUseStart {
                id: "t1".to_string(),
                name: "test_query".to_string(),
            },
            StreamEvent::ToolInputDelta {
                text: "{\"q".to_string(),
            },
            StreamEvent::ToolUseEnd {
                id: "t1".to_string(),
                name: "test_query".to_string(),
                input: serde_json::json!({"query": "rust"}),
            },
            StreamEvent::ContentComplete {
                stop_reason: StopReason::EndTurn,
                usage: TokenUsage {
                    input_tokens: 10,
                    output_tokens: 5,
                },
            },
        ];
        assert_eq!(events.len(), 5);
    }

    #[tokio::test]
    async fn test_default_stream_sends_events() {
        use tokio::sync::mpsc;

        struct FakeDriver;

        #[async_trait]
        impl LlmDriver for FakeDriver {
            async fn complete(
                &self,
                _request: CompletionRequest,
            ) -> Result<CompletionResponse, LlmError> {
                Ok(CompletionResponse {
                    content: vec![ContentBlock::Text {
                        text: "Hello!".to_string(),
                        provider_metadata: None,
                    }],
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: TokenUsage {
                        input_tokens: 5,
                        output_tokens: 3,
                    },
                    media: None,
                })
            }
        }

        let driver = FakeDriver;
        let (tx, mut rx) = mpsc::channel(16);
        let request = CompletionRequest {
            model: "test".to_string(),
            messages: vec![],
            tools: vec![],
            max_tokens: 100,
            temperature: 0.0,
            system: None,
            thinking: None,
            extra: Default::default(),
        };

        let response = driver.stream(request, tx).await.unwrap();
        assert_eq!(response.text(), "Hello!");

        // Should receive TextDelta then ContentComplete
        let ev1 = rx.recv().await.unwrap();
        assert!(matches!(ev1, StreamEvent::TextDelta { text } if text == "Hello!"));

        let ev2 = rx.recv().await.unwrap();
        assert!(matches!(
            ev2,
            StreamEvent::ContentComplete {
                stop_reason: StopReason::EndTurn,
                ..
            }
        ));
    }
}
