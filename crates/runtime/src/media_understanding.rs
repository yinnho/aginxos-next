//! Media understanding engine — image description, audio transcription, video analysis.
//!
//! Auto-cascades through available providers based on configured API keys.

use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::info;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::media::{MediaAttachment, MediaConfig, MediaSource, MediaType, MediaUnderstanding};

/// Media understanding engine.
pub struct MediaEngine {
    config: MediaConfig,
    semaphore: Arc<Semaphore>,
}

impl MediaEngine {
    pub fn new(config: MediaConfig) -> Self {
        let max = config.max_concurrency.clamp(1, 8);
        Self {
            config,
            semaphore: Arc::new(Semaphore::new(max)),
        }
    }

    /// Transcribe audio using speech-to-text.
    /// Auto-cascade: Groq (whisper-large-v3-turbo) -> OpenAI (whisper-1).
    pub async fn transcribe_audio(
        &self,
        attachment: &MediaAttachment,
    ) -> CarrierResult<MediaUnderstanding> {
        attachment.validate()?;
        if attachment.media_type != MediaType::Audio {
            return Err(CarrierError::InvalidInput(
                "Expected audio attachment".into(),
            ));
        }

        let provider = self
            .config
            .audio_provider
            .as_deref()
            .or_else(|| detect_audio_provider())
            .ok_or_else(|| CarrierError::Config("No audio transcription provider configured. Set GROQ_API_KEY or OPENAI_API_KEY".to_string()))?;

        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| CarrierError::Internal(e.to_string()))?;

        // Parakeet MLX — local transcription via uv + Python
        if provider == "parakeet-mlx" {
            return transcribe_with_parakeet_mlx(attachment).await;
        }

        // Derive a proper filename with extension from mime_type
        // (Whisper APIs require an extension to detect format)
        let ext = match attachment.mime_type.as_str() {
            "audio/wav" => "wav",
            "audio/mpeg" | "audio/mp3" => "mp3",
            "audio/ogg" => "ogg",
            "audio/webm" => "webm",
            "audio/mp4" | "audio/m4a" => "m4a",
            "audio/flac" => "flac",
            _ => "wav",
        };

        // Read audio bytes from source
        let audio_bytes = match &attachment.source {
            MediaSource::FilePath { path } => tokio::fs::read(path).await.map_err(|e| {
                CarrierError::Internal(format!("Failed to read audio file '{}': {}", path, e))
            })?,
            MediaSource::Base64 { data, .. } => {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .map_err(|e| {
                        CarrierError::Serialization(format!("Failed to decode base64 audio: {}", e))
                    })?
            }
            MediaSource::Url { url } => {
                return Err(CarrierError::InvalidInput(format!(
                    "URL-based audio source not supported for transcription: {}",
                    url
                )));
            }
        };
        let filename = format!("audio.{}", ext);

        let model = default_audio_model(provider);

        // Build API request
        let (api_url, api_key) = match provider {
            "groq" => (
                "https://api.groq.com/openai/v1/audio/transcriptions",
                std::env::var("GROQ_API_KEY")
                    .map_err(|_| CarrierError::Config("GROQ_API_KEY not set".to_string()))?,
            ),
            "openai" => (
                "https://api.openai.com/v1/audio/transcriptions",
                std::env::var("OPENAI_API_KEY")
                    .map_err(|_| CarrierError::Config("OPENAI_API_KEY not set".to_string()))?,
            ),
            other => {
                return Err(CarrierError::Config(format!(
                    "Unsupported audio provider: {}",
                    other
                )))
            }
        };

        info!(provider, model, filename = %filename, size = audio_bytes.len(), "Sending audio for transcription");

        let file_part = reqwest::multipart::Part::bytes(audio_bytes)
            .file_name(filename)
            .mime_str(&attachment.mime_type)
            .map_err(|e| CarrierError::Internal(format!("Failed to set MIME type: {}", e)))?;

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", model.to_string())
            .text("response_format", "text");

        let client = reqwest::Client::new();
        let resp = client
            .post(api_url)
            .bearer_auth(&api_key)
            .multipart(form)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| CarrierError::Network(format!("Transcription request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(CarrierError::Network(format!(
                "Transcription API error ({}): {}",
                status, body
            )));
        }

        let transcription = resp.text().await.map_err(|e| {
            CarrierError::Network(format!("Failed to read transcription response: {}", e))
        })?;

        let transcription = transcription.trim().to_string();
        if transcription.is_empty() {
            return Err(CarrierError::LlmDriver(
                "Transcription returned empty text".into(),
            ));
        }

        info!(
            provider,
            model,
            chars = transcription.len(),
            "Audio transcription complete"
        );

        Ok(MediaUnderstanding {
            media_type: MediaType::Audio,
            description: transcription,
            provider: provider.to_string(),
            model: model.to_string(),
        })
    }
}

/// Transcribe audio using Parakeet MLX (local, via uv + Python).
async fn transcribe_with_parakeet_mlx(
    attachment: &MediaAttachment,
) -> CarrierResult<MediaUnderstanding> {
    use tokio::time::{timeout, Duration};

    // Materialize audio to a temp file if needed
    let (audio_path, is_temp) = match &attachment.source {
        MediaSource::FilePath { path } => (std::path::PathBuf::from(path), false),
        MediaSource::Base64 { data, mime_type } => {
            use base64::Engine;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| {
                    CarrierError::Serialization(format!("Failed to decode base64 audio: {e}"))
                })?;
            let ext = match mime_type.as_str() {
                "audio/wav" | "audio/x-wav" => "wav",
                "audio/mpeg" | "audio/mp3" => "mp3",
                "audio/ogg" => "ogg",
                "audio/webm" => "webm",
                "audio/mp4" | "audio/m4a" => "m4a",
                "audio/flac" => "flac",
                _ => "wav",
            };
            let path = std::env::temp_dir().join(format!(
                "carrier_parakeet_{}.{}",
                uuid::Uuid::new_v4(),
                ext
            ));
            tokio::fs::write(&path, decoded)
                .await
                .map_err(|e| CarrierError::Internal(format!("Failed to write temp audio: {e}")))?;
            (path, true)
        }
        MediaSource::Url { url } => {
            return Err(CarrierError::InvalidInput(format!(
                "URL audio not supported for parakeet-mlx: {url}"
            )));
        }
    };

    let script = r#"
import json, sys
from parakeet_mlx import from_pretrained
model = from_pretrained("mlx-community/parakeet-tdt-0.6b-v3")
result = model.transcribe(sys.argv[1])
print(json.dumps({"text": result.text, "model": "mlx-community/parakeet-tdt-0.6b-v3"}))
"#;

    let mut cmd = tokio::process::Command::new("uv");
    cmd.args([
        "run",
        "--with",
        "parakeet-mlx",
        "python3",
        "-c",
        script,
        &audio_path.to_string_lossy(),
    ]);
    cmd.env("PYTHONUNBUFFERED", "1");
    cmd.kill_on_drop(true);

    let output = timeout(Duration::from_secs(900), cmd.output())
        .await
        .map_err(|_| CarrierError::Internal("parakeet-mlx timed out after 15 minutes".to_string()))?
        .map_err(|e| CarrierError::Internal(format!("Failed to launch parakeet-mlx: {e}")))?;

    if is_temp {
        let _ = tokio::fs::remove_file(&audio_path).await;
    }

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CarrierError::LlmDriver(format!(
            "parakeet-mlx failed: {}",
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| CarrierError::Internal(format!("parakeet-mlx non-UTF8: {e}")))?;
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| CarrierError::LlmDriver(format!("parakeet-mlx parse failed: {e}")))?;

    let text = parsed["text"]
        .as_str()
        .ok_or_else(|| CarrierError::LlmDriver("missing text field".to_string()))?
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(CarrierError::LlmDriver(
            "parakeet-mlx returned empty transcription".into(),
        ));
    }

    Ok(MediaUnderstanding {
        media_type: MediaType::Audio,
        description: text,
        provider: "parakeet-mlx".to_string(),
        model: parsed["model"]
            .as_str()
            .unwrap_or("parakeet-tdt-0.6b-v3")
            .to_string(),
    })
}

/// Detect which audio transcription provider is available.
fn detect_audio_provider() -> Option<&'static str> {
    // Explicit opt-in for local Parakeet MLX transcription
    if std::env::var("AGINX_CARRIER_ENABLE_PARAKEET_MLX").is_ok() {
        return Some("parakeet-mlx");
    }
    if std::env::var("GROQ_API_KEY").is_ok() {
        return Some("groq");
    }
    if std::env::var("OPENAI_API_KEY").is_ok() {
        return Some("openai");
    }
    None
}

/// Get the default audio model for a provider.
fn default_audio_model(provider: &str) -> &str {
    match provider {
        "parakeet-mlx" => "mlx-community/parakeet-tdt-0.6b-v3",
        "groq" => "whisper-large-v3-turbo",
        "openai" => "whisper-1",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use carrier_types::media::MediaSource;

    #[test]
    fn test_engine_creation() {
        let config = MediaConfig::default();
        let engine = MediaEngine::new(config);
        assert_eq!(engine.config.max_concurrency, 2);
    }

    #[test]
    fn test_engine_max_concurrency_clamped() {
        let config = MediaConfig {
            max_concurrency: 100,
            ..Default::default()
        };
        let engine = MediaEngine::new(config);
        // Semaphore was clamped to 8
        assert!(engine.semaphore.available_permits() <= 8);
    }

    #[tokio::test]
    async fn test_transcribe_audio_wrong_type() {
        let engine = MediaEngine::new(MediaConfig::default());
        let attachment = MediaAttachment {
            media_type: MediaType::Image,
            mime_type: "image/png".into(),
            source: MediaSource::FilePath {
                path: "test.png".into(),
            },
            size_bytes: 1024,
        };
        let result = engine.transcribe_audio(&attachment).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_default_audio_models() {
        assert_eq!(default_audio_model("groq"), "whisper-large-v3-turbo");
        assert_eq!(default_audio_model("openai"), "whisper-1");
    }

    #[tokio::test]
    async fn test_transcribe_audio_rejects_image_type() {
        let engine = MediaEngine::new(MediaConfig::default());
        let attachment = MediaAttachment {
            media_type: MediaType::Image,
            mime_type: "image/png".into(),
            source: MediaSource::FilePath {
                path: "test.png".into(),
            },
            size_bytes: 1024,
        };
        let result = engine.transcribe_audio(&attachment).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Expected audio"));
    }

    #[tokio::test]
    async fn test_transcribe_audio_no_provider() {
        // With no API keys set, should fail with provider error
        let engine = MediaEngine::new(MediaConfig::default());
        let attachment = MediaAttachment {
            media_type: MediaType::Audio,
            mime_type: "audio/webm".into(),
            source: MediaSource::FilePath {
                path: "test.webm".into(),
            },
            size_bytes: 1024,
        };
        let result = engine.transcribe_audio(&attachment).await;
        // Either fails with "No audio transcription provider" or file read error
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_audio_url_source_rejected() {
        // URL source should be rejected
        let config = MediaConfig {
            audio_provider: Some("groq".to_string()),
            ..Default::default()
        };
        let engine = MediaEngine::new(config);
        let attachment = MediaAttachment {
            media_type: MediaType::Audio,
            mime_type: "audio/mpeg".into(),
            source: MediaSource::Url {
                url: "https://example.com/audio.mp3".into(),
            },
            size_bytes: 1024,
        };
        let result = engine.transcribe_audio(&attachment).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("URL-based audio source not supported"));
    }

    #[tokio::test]
    async fn test_transcribe_audio_file_not_found() {
        let config = MediaConfig {
            audio_provider: Some("groq".to_string()),
            ..Default::default()
        };
        let engine = MediaEngine::new(config);
        let attachment = MediaAttachment {
            media_type: MediaType::Audio,
            mime_type: "audio/webm".into(),
            source: MediaSource::FilePath {
                path: "/nonexistent/path/audio.webm".into(),
            },
            size_bytes: 1024,
        };
        let result = engine.transcribe_audio(&attachment).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Failed to read audio file"));
    }
}
