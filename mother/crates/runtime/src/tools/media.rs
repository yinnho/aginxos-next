//! Media, Docker, process, and canvas tool module.
//!
//! Provides image analysis, media understanding, TTS/STT, image generation,
//! Docker sandbox, persistent process management, and canvas presentation tools.

use super::ToolModule;
use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use carrier_types::config::ExecPolicy;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::ToolDefinition;

/// Media, Docker, process, and canvas tools.
pub struct MediaTools;

#[async_trait]
impl ToolModule for MediaTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![
            // --- Media understanding tools ---
            ToolDefinition {
                name: "media_describe".to_string(),
                description: "Describe an image using a vision-capable LLM. Auto-selects the best available provider (Anthropic, OpenAI, or Gemini). Returns a text description of the image content.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the image file (relative to workspace)" },
                        "prompt": { "type": "string", "description": "Optional prompt to guide the description (e.g., 'Extract all text from this image')" }
                    },
                    "required": ["path"]
                }),
            },
            ToolDefinition {
                name: "media_transcribe".to_string(),
                description: "Transcribe audio to text using speech-to-text. Auto-selects the best available provider (Groq Whisper or OpenAI Whisper). Returns the transcript.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the audio file (relative to workspace). Supported: mp3, wav, ogg, flac, m4a, webm." },
                        "language": { "type": "string", "description": "Optional ISO-639-1 language code (e.g., 'en', 'es', 'ja')" }
                    },
                    "required": ["path"]
                }),
            },
            // --- Image generation tool ---
            ToolDefinition {
                name: "image_generate".to_string(),
                description: "Generate images from a text prompt. Uses the configured image modality. Images are saved under the user's output/ directory. The result always includes view_url / view_urls — paste those into the reply so the user can open the image in a browser. saved_to is the local filesystem path for tools that need a file path.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "prompt": { "type": "string", "description": "Text description of the image to generate" },
                        "size": { "type": "string", "description": "Image size. Minimum 768x768 (589824 pixels). Common values: '1024x1024' (default), '1024x1792', '1792x1024', '768x768'. Smaller sizes will be auto-upscaled to 768x768." },
                        "count": { "type": "integer", "description": "Number of images to generate (1-4, default: 1)" },
                        "aspect_ratio": { "type": "string", "description": "Image aspect ratio: '1:1', '16:9', '4:3', '3:2', '2:3', '3:4', '9:16', '21:9'" },
                        "prompt_optimizer": { "type": "boolean", "description": "Whether to auto-optimize the prompt (default: false)" }
                    },
                    "required": ["prompt"]
                }),
            },
            // --- Video generation tool ---
            ToolDefinition {
                name: "video_generate".to_string(),
                description: "Generate a short video from a text prompt. Uses AI video generation (e.g. Kling, Runway). Returns video_url (provider URL). When a local copy is saved, also returns view_url for the user to open in a browser.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "prompt": { "type": "string", "description": "Text description of the video to generate" },
                        "duration": { "type": "integer", "description": "Approximate duration in seconds (e.g. 5 for 5s, default: 5)" }
                    },
                    "required": ["prompt"]
                }),
            },
            // --- TTS/STT tools ---
            ToolDefinition {
                name: "text_to_speech".to_string(),
                description: "Convert text to speech audio. Auto-selects OpenAI or ElevenLabs. Saves audio to the user's output directory.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The text to convert to speech (max 4096 chars)" },
                        "voice": { "type": "string", "description": "Voice name: 'alloy', 'echo', 'fable', 'onyx', 'nova', 'shimmer' (default: 'alloy')" },
                        "format": { "type": "string", "description": "Output format: 'mp3', 'opus', 'aac', 'flac' (default: 'mp3')" }
                    },
                    "required": ["text"]
                }),
            },
            ToolDefinition {
                name: "speech_to_text".to_string(),
                description: "Transcribe audio to text using speech-to-text. Auto-selects Groq Whisper or OpenAI Whisper. Supported formats: mp3, wav, ogg, flac, m4a, webm.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Path to the audio file (relative to workspace)" },
                        "language": { "type": "string", "description": "Optional ISO-639-1 language code (e.g., 'en', 'es', 'ja')" }
                    },
                    "required": ["path"]
                }),
            },
            // --- Persistent process tools ---
            ToolDefinition {
                name: "process_start".to_string(),
                description: "Start a long-running process (REPL, server, watcher). Returns a process_id for subsequent poll/write/kill operations. Max 5 processes per agent.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "The executable to run (e.g. 'python', 'node', 'npm')" },
                        "args": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Command-line arguments (e.g. ['-i'] for interactive Python)"
                        }
                    },
                    "required": ["command"]
                }),
            },
            ToolDefinition {
                name: "process_poll".to_string(),
                description: "Read accumulated stdout/stderr from a running process. Non-blocking: returns whatever output has buffered since the last poll.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "process_id": { "type": "string", "description": "The process ID returned by process_start" }
                    },
                    "required": ["process_id"]
                }),
            },
            ToolDefinition {
                name: "process_write".to_string(),
                description: "Write data to a running process's stdin. A newline is appended automatically if not present.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "process_id": { "type": "string", "description": "The process ID returned by process_start" },
                        "data": { "type": "string", "description": "The data to write to stdin" }
                    },
                    "required": ["process_id", "data"]
                }),
            },
            ToolDefinition {
                name: "process_kill".to_string(),
                description: "Terminate a running process and clean up its resources.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "process_id": { "type": "string", "description": "The process ID returned by process_start" }
                    },
                    "required": ["process_id"]
                }),
            },
            ToolDefinition {
                name: "process_list".to_string(),
                description: "List all running processes for the current agent, including their IDs, commands, uptime, and alive status.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            // --- Canvas / A2UI tool ---
            ToolDefinition {
                name: "canvas_present".to_string(),
                description: "Present an interactive HTML canvas to the user. The HTML is sanitized (no scripts, no event handlers) and saved to the workspace. The dashboard will render it in a panel. Use for rich data visualizations, formatted reports, or interactive UI.".to_string(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "html": { "type": "string", "description": "The HTML content to present. Must not contain <script> tags, event handlers, or javascript: URLs." },
                        "title": { "type": "string", "description": "Optional title for the canvas panel" }
                    },
                    "required": ["html"]
                }),
            },
        ]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        match name {
            // Media understanding
            "media_describe" => Some(tool_media_describe(input, ctx).await),
            "media_transcribe" => Some(tool_media_transcribe(input, ctx.brain).await),

            // Image generation
            "image_generate" => Some(
                tool_image_generate(
                    input,
                    ctx.brain,
                    ctx.home_dir,
                    ctx.agent_name,
                    ctx.owner_id,
                    ctx.sender_id,
                    ctx.external_url,
                )
                .await,
            ),

            // Video generation
            "video_generate" => Some(tool_video_generate(input, ctx.brain).await),

            // TTS/STT
            "text_to_speech" => Some(
                tool_text_to_speech(
                    input,
                    ctx.brain,
                    ctx.home_dir,
                    ctx.agent_name,
                    ctx.owner_id,
                    ctx.sender_id,
                    ctx.external_url,
                )
                .await,
            ),
            "speech_to_text" => {
                Some(tool_speech_to_text(input, ctx.brain, ctx.workspace_root).await)
            }

            // Persistent process tools
            "process_start" => Some(
                tool_process_start(
                    input,
                    ctx.process_manager,
                    ctx.caller_agent_id,
                    ctx.exec_policy,
                    ctx.allowed_env_vars,
                )
                .await,
            ),
            "process_poll" => {
                Some(tool_process_poll(input, ctx.process_manager, ctx.caller_agent_id).await)
            }
            "process_write" => {
                Some(tool_process_write(input, ctx.process_manager, ctx.caller_agent_id).await)
            }
            "process_kill" => {
                Some(tool_process_kill(input, ctx.process_manager, ctx.caller_agent_id).await)
            }
            "process_list" => {
                Some(tool_process_list(ctx.process_manager, ctx.caller_agent_id).await)
            }

            // Canvas / A2UI
            "canvas_present" => Some(
                tool_canvas_present(
                    input,
                    ctx.workspace_root,
                    ctx.home_dir,
                    ctx.agent_name,
                    ctx.owner_id,
                    ctx.sender_id,
                    ctx.external_url,
                )
                .await,
            ),

            _ => None,
        }
    }

    fn permission_level(&self, tool_name: &str) -> carrier_types::tool::PermissionLevel {
        match tool_name {
            // image_analyze 已移交 agf 桥（M32 D3 批2），不在此列。
            "media_describe" | "media_transcribe" | "speech_to_text" => {
                carrier_types::tool::PermissionLevel::ReadOnly
            }
            "image_generate" | "video_generate" | "text_to_speech" | "canvas_present" => {
                carrier_types::tool::PermissionLevel::Write
            }
            "process_start" | "process_poll" | "process_write" | "process_list" => {
                carrier_types::tool::PermissionLevel::Execute
            }
            "process_kill" => carrier_types::tool::PermissionLevel::Dangerous,
            _ => carrier_types::tool::PermissionLevel::Dangerous,
        }
    }
}

// ---------------------------------------------------------------------------
// image_analyze 与其魔数/尺寸识别 helpers 已整体移交 `agf` CLI
//（M32 D3 批2，crates/agf/src/ops.rs）；本模块不再常驻该实现。
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Media understanding tools
// ---------------------------------------------------------------------------

/// Describe an image using a vision-capable LLM through the Brain fallback chain.
/// Prefer a public view_url so the provider fetches the image (no base64 payload).
async fn tool_media_describe(
    input: &serde_json::Value,
    ctx: &crate::tool_context::ToolContext<'_>,
) -> CarrierResult<String> {
    let brain = ctx.brain.ok_or(CarrierError::Config(
        "Brain not available. Check configuration.".to_string(),
    ))?;
    let path = input["path"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'path' parameter".to_string(),
    ))?;
    let prompt = input["prompt"]
        .as_str()
        .unwrap_or("Describe this image in detail.");

    // Resolve path (sender sandbox or /tmp screenshots)
    let resolved = if path.starts_with("/tmp/") {
        std::path::PathBuf::from(path)
    } else if let (Some(hd), Some(sid), Some(an)) = (ctx.home_dir, ctx.sender_id, ctx.agent_name) {
        // Prefer user-data paths: input/… output/…
        let normalized = path.replace('\\', "/");
        let rel = normalized.trim_start_matches('/');
        if rel.starts_with("input/") || rel.starts_with("output/") || rel.starts_with("memory/") {
            let oid = ctx.owner_id.unwrap_or(sid);
            carrier_types::config::sender_data_dir(hd, oid, an, Some(sid)).join(rel)
        } else {
            crate::tools::resolve_file_path_for_read(
                path,
                ctx.workspace_root,
                ctx.sender_id,
                ctx.agent_name,
            )?
        }
    } else {
        crate::tools::resolve_file_path_for_read(
            path,
            ctx.workspace_root,
            ctx.sender_id,
            ctx.agent_name,
        )?
    };

    if !resolved.is_file() {
        return Err(CarrierError::InvalidInput(format!(
            "Failed to read image file: {} (resolved {})",
            path,
            resolved.display()
        )));
    }

    let ext = resolved
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => {
            return Err(CarrierError::InvalidInput(format!(
                "Unsupported image format: .{ext}"
            )))
        }
    };

    // Prefer public URL for vision.
    let mut image_url: Option<String> = None;
    if let (Some(an), Some(sid)) = (ctx.agent_name, ctx.sender_id) {
        let rel = path.replace('\\', "/");
        let rel = rel.trim_start_matches('/');
        let under_sender = if rel.starts_with("input/")
            || rel.starts_with("output/")
            || rel.starts_with("memory/")
        {
            rel.to_string()
        } else if let Some(idx) = rel.find("input/") {
            rel[idx..].to_string()
        } else if let Some(idx) = rel.find("output/") {
            rel[idx..].to_string()
        } else {
            format!("input/{rel}")
        };
        image_url = crate::file_view::build_file_view_url(ctx.external_url, an, &under_sender, sid);
    }

    let image_block = if let Some(url) = image_url.clone() {
        carrier_types::message::ContentBlock::Image {
            media_type: mime.to_string(),
            data: String::new(),
            url: Some(url),
        }
    } else {
        // Fallback: base64 (legacy / no external_url)
        use base64::Engine;
        let data = tokio::fs::read(&resolved)
            .await
            .map_err(|e| CarrierError::Internal(format!("Failed to read image file: {e}")))?;
        let max_bytes = 5 * 1024 * 1024;
        if data.len() > max_bytes {
            return Err(CarrierError::InvalidInput(format!(
                "Image too large: {} bytes (max {} MB) and no public view_url available",
                data.len(),
                max_bytes / (1024 * 1024)
            )));
        }
        let base64_data = base64::engine::general_purpose::STANDARD.encode(&data);
        carrier_types::message::ContentBlock::Image {
            media_type: mime.to_string(),
            data: base64_data,
            url: None,
        }
    };

    let request = crate::llm_driver::CompletionRequest {
        model: String::new(),
        messages: vec![carrier_types::message::Message {
            role: carrier_types::message::Role::User,
            content: carrier_types::message::MessageContent::Blocks(vec![
                image_block,
                carrier_types::message::ContentBlock::Text {
                    text: prompt.to_string(),
                    provider_metadata: None,
                },
            ]),
        }],
        tools: vec![],
        max_tokens: 1024,
        temperature: 0.3,
        system: None,
        thinking: None,
        extra: Default::default(),
    };

    let response = brain
        .complete("vision", request)
        .await
        .map_err(|e| CarrierError::LlmDriver(format!("Vision LLM call failed: {e}")))?;

    let description = response.text();
    if description.is_empty() {
        return Err(CarrierError::LlmDriver(
            "Vision model returned empty response".to_string(),
        ));
    }

    let mut result = serde_json::json!({
        "description": description,
        "path": path,
        "via_url": image_url.is_some(),
        "usage": {
            "input_tokens": response.usage.input_tokens,
            "output_tokens": response.usage.output_tokens,
        },
    });
    if let Some(u) = image_url {
        result
            .as_object_mut()
            .unwrap()
            .insert("view_url".into(), serde_json::json!(u));
    }
    serde_json::to_string_pretty(&result).map_err(|e| CarrierError::Serialization(e.to_string()))
}

/// Transcribe audio to text via the Brain's audio modality.
async fn tool_media_transcribe(
    input: &serde_json::Value,
    brain: Option<&std::sync::Arc<dyn crate::llm_driver::Brain>>,
) -> CarrierResult<String> {
    use base64::Engine;
    let brain = brain.ok_or(CarrierError::Config(
        "Brain not available. Ensure audio modality is configured.".to_string(),
    ))?;
    let path = input["path"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'path' parameter".to_string(),
    ))?;
    // Allow /tmp/ paths for browser screenshots; validate relative paths normally
    if !path.starts_with("/tmp/") {
        let _ = crate::tools::validate_path(path)?;
    }

    // Read audio file
    let data = tokio::fs::read(path)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read audio file: {e}")))?;

    // Detect MIME type from extension
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mime = match ext.as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "webm" => "audio/webm",
        _ => {
            return Err(CarrierError::InvalidInput(format!(
                "Unsupported audio format: .{ext}"
            )))
        }
    };

    let audio_block = carrier_types::message::ContentBlock::Audio {
        media_type: mime.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&data),
    };

    let request = crate::llm_driver::CompletionRequest {
        model: String::new(),
        messages: vec![carrier_types::message::Message {
            role: carrier_types::message::Role::User,
            content: carrier_types::message::MessageContent::Blocks(vec![audio_block]),
        }],
        tools: vec![],
        max_tokens: 4096,
        temperature: 0.0,
        system: None,
        thinking: None,
        extra: serde_json::Value::Object(serde_json::Map::new()),
    };

    let response = brain.complete("audio", request).await.map_err(|e| {
        CarrierError::LlmDriver(format!("Audio transcription brain call failed: {e}"))
    })?;

    let transcript = response.text();
    let result = serde_json::json!({
        "transcript": transcript,
        "provider": "brain",
    });
    serde_json::to_string_pretty(&result).map_err(|e| CarrierError::Serialization(e.to_string()))
}

// ---------------------------------------------------------------------------
// Image generation tool
// ---------------------------------------------------------------------------

/// Generate images from a text prompt via the Brain's image modality.
async fn tool_image_generate(
    input: &serde_json::Value,
    brain: Option<&std::sync::Arc<dyn crate::llm_driver::Brain>>,
    home_dir: Option<&Path>,
    agent_name: Option<&str>,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
    external_url: Option<&str>,
) -> CarrierResult<String> {
    let brain = brain.ok_or(CarrierError::Config(
        "Brain not available. Ensure image modality is configured.".to_string(),
    ))?;
    let prompt = input["prompt"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'prompt' parameter".to_string(),
    ))?;

    let model = input["model"].as_str().unwrap_or("dall-e-3");
    let mut size = input["size"].as_str().unwrap_or("1024x1024").to_string();

    // Enforce minimum pixel count for providers that require it (e.g. DashScope: 589824 = 768x768)
    const MIN_PIXELS: u32 = 589824;
    if let Some((w, h)) = size.split_once('x').and_then(|(w, h)| {
        let w = w.parse::<u32>().ok()?;
        let h = h.parse::<u32>().ok()?;
        Some((w, h))
    }) {
        if w.saturating_mul(h) < MIN_PIXELS {
            tracing::warn!(requested = %size, "Image size below provider minimum; upscaling to 1024x1024");
            size = "1024x1024".to_string();
        }
    }

    let quality = input["quality"].as_str().unwrap_or("hd");
    let count = input["count"].as_u64().unwrap_or(1).min(4) as u8;
    let include_base64 = input["include_base64"].as_bool().unwrap_or(false);

    let mut extra = serde_json::Map::new();
    extra.insert("model".to_string(), serde_json::json!(model));
    extra.insert("size".to_string(), serde_json::json!(size));
    extra.insert("quality".to_string(), serde_json::json!(quality));
    extra.insert("n".to_string(), serde_json::json!(count));
    if let Some(ar) = input["aspect_ratio"].as_str() {
        extra.insert("aspect_ratio".to_string(), serde_json::json!(ar));
    }
    if let Some(po) = input["prompt_optimizer"].as_bool() {
        extra.insert("prompt_optimizer".to_string(), serde_json::json!(po));
    }

    let request = crate::llm_driver::CompletionRequest {
        model: String::new(),
        messages: vec![carrier_types::message::Message {
            role: carrier_types::message::Role::User,
            content: carrier_types::message::MessageContent::Text(prompt.to_string()),
        }],
        tools: vec![],
        max_tokens: 0,
        temperature: 0.0,
        system: None,
        thinking: None,
        extra: serde_json::Value::Object(extra),
    };

    let response = brain.complete("image", request).await.map_err(|e| {
        CarrierError::LlmDriver(format!(
            "Image generation failed: {e}. \
                 Do NOT retry image_generate with the same prompt. \
                 Tell the user the image generation service is currently unavailable \
                 and suggest trying again later."
        ))
    })?;

    let images = match response.media {
        Some(carrier_types::media::MediaOutput::Images { items }) => items,
        Some(carrier_types::media::MediaOutput::Image { data, format: _fmt }) => {
            vec![carrier_types::media::GeneratedImage {
                data_base64: {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD.encode(&data)
                },
                url: None,
            }]
        }
        _ => {
            return Err(CarrierError::LlmDriver(
                "Image generation returned no images".to_string(),
            ))
        }
    };

    if images.is_empty() {
        return Err(CarrierError::LlmDriver(
            "Image generation returned empty image list".to_string(),
        ));
    }

    // Save images to workspace output directory if available
    let mut saved_paths: Vec<String> = Vec::new();
    let mut rel_paths: Vec<String> = Vec::new();
    if let (Some(hd), Some(an)) = (home_dir, agent_name) {
        // Match file_write / files/view: workspaces/{agent}/senders/{owner}[/users/{sid}]/output
        let sid = sender_id.unwrap_or("shared");
        let oid = owner_id.unwrap_or(sid);
        let output_dir = carrier_types::config::sender_data_dir(hd, oid, an, Some(sid)).join("output");
        tokio::fs::create_dir_all(&output_dir)
            .await
            .map_err(|e| CarrierError::Internal(format!("Failed to create output dir: {e}")))?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();

        for (i, image) in images.iter().enumerate() {
            let filename = if images.len() == 1 {
                format!("image_{timestamp}.png")
            } else {
                format!("image_{timestamp}_{i}.png")
            };
            let path = output_dir.join(&filename);

            let decoded = if !image.data_base64.is_empty() {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(&image.data_base64)
                    .map_err(|e| {
                        CarrierError::Internal(format!("Failed to decode base64 image: {e}"))
                    })?
            } else if let Some(ref url) = image.url {
                // Download from URL (e.g. MiniMax returns temporary URLs)
                let resp = reqwest::Client::new()
                    .get(url)
                    .timeout(std::time::Duration::from_secs(60))
                    .send()
                    .await
                    .map_err(|e| {
                        CarrierError::Network(format!("Failed to download image from URL: {e}"))
                    })?;
                resp.bytes()
                    .await
                    .map_err(|e| {
                        CarrierError::Network(format!("Failed to read image response: {e}"))
                    })?
                    .to_vec()
            } else {
                return Err(CarrierError::Internal(
                    "Image has neither base64 data nor URL".to_string(),
                ));
            };

            tokio::fs::write(&path, &decoded)
                .await
                .map_err(|e| CarrierError::Internal(format!("Failed to write image: {e}")))?;

            // Absolute path for tools that need direct filesystem access (e.g. upload).
            saved_paths.push(path.to_string_lossy().to_string());
            rel_paths.push(format!("output/{filename}"));
        }
    }

    // Also save to the uploads temp dir so the web UI can serve them via
    // GET /api/uploads/{file_id}. Each image gets a UUID filename.
    let mut image_urls: Vec<String> = Vec::new();
    let mut temp_paths: Vec<String> = Vec::new();
    {
        let upload_dir = std::env::temp_dir().join("carrier_uploads");
        let _ = std::fs::create_dir_all(&upload_dir);
        for image in &images {
            let file_id = uuid::Uuid::new_v4().to_string();
            let decoded = if !image.data_base64.is_empty() {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD
                    .decode(&image.data_base64)
                    .ok()
            } else {
                None
            };
            // For URL-only images, they can be accessed directly — skip local upload
            if let Some(decoded) = decoded {
                let path = upload_dir.join(&file_id);
                if std::fs::write(&path, &decoded).is_ok() {
                    image_urls.push(format!("/api/uploads/{file_id}"));
                    // Return actual file path for MCP tools that need direct file access
                    temp_paths.push(path.to_string_lossy().to_string());
                }
            } else if let Some(ref url) = image.url {
                image_urls.push(url.clone());
            }
        }
    }

    // Include base64 of the first image so downstream tools (e.g. upload) can use it directly.
    // For URL-only providers (DashScope), download and encode to base64 on the fly.
    let base64_data = if let Some(first) = images.first() {
        if !first.data_base64.is_empty() {
            first.data_base64.clone()
        } else if let Some(ref url) = first.url {
            match reqwest::Client::new()
                .get(url)
                .timeout(std::time::Duration::from_secs(60))
                .send()
                .await
            {
                Ok(resp) => match resp.bytes().await {
                    Ok(bytes) => {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.encode(&bytes)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to read image bytes for base64 encoding");
                        String::new()
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to download image for base64 encoding");
                    String::new()
                }
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Build response - only include base64 when explicitly requested
    // to avoid response truncation (25500 char limit)
    let mut response = serde_json::Map::new();
    response.insert("images_generated".into(), serde_json::json!(images.len()));
    response.insert("saved_to".into(), serde_json::json!(saved_paths));
    response.insert("rel_paths".into(), serde_json::json!(rel_paths));
    response.insert("image_urls".into(), serde_json::json!(image_urls));
    response.insert("temp_paths".into(), serde_json::json!(temp_paths));
    response.insert("provider".into(), serde_json::json!("brain"));

    // System capability: public browser links for every saved image.
    if let (Some(an), Some(sid)) = (agent_name, sender_id) {
        let view_urls = crate::file_view::build_file_view_urls(external_url, an, &rel_paths, sid);
        if !view_urls.is_empty() {
            response.insert("view_urls".into(), serde_json::json!(view_urls.clone()));
            if let Some(first) = view_urls.first() {
                response.insert("view_url".into(), serde_json::json!(first));
            }
            response.insert(
                "note".into(),
                serde_json::json!(
                    "Paste view_url / view_urls into the user reply so they can open the image. saved_to is the local path for tools that need a filesystem path."
                ),
            );
        }
    }

    // Only include base64 if explicitly requested (for small images or debugging)
    if include_base64 {
        response.insert("base64".into(), serde_json::json!(base64_data));
    } else {
        response.insert("base64".into(), serde_json::json!(null));
        response.insert("base64_truncated".into(), serde_json::json!(true));
        if !response.contains_key("note") {
            response.insert("note".into(), serde_json::json!("base64 omitted to avoid response truncation. Use include_base64=true if needed, or use saved_to/temp_paths paths directly."));
        }
    }

    serde_json::to_string_pretty(&response).map_err(|e| CarrierError::Serialization(e.to_string()))
}

// ---------------------------------------------------------------------------
// Video generation tool
// ---------------------------------------------------------------------------

async fn tool_video_generate(
    input: &serde_json::Value,
    brain: Option<&std::sync::Arc<dyn crate::llm_driver::Brain>>,
) -> CarrierResult<String> {
    let brain = brain.ok_or(CarrierError::Config(
        "Brain not available. Ensure video modality is configured.".to_string(),
    ))?;
    let prompt = input["prompt"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'prompt' parameter".to_string(),
    ))?;

    let mut extra = serde_json::Map::new();
    if let Some(duration) = input["duration"].as_u64() {
        extra.insert("duration".to_string(), serde_json::json!(duration));
    }

    let request = crate::llm_driver::CompletionRequest {
        model: String::new(),
        messages: vec![carrier_types::message::Message {
            role: carrier_types::message::Role::User,
            content: carrier_types::message::MessageContent::Text(prompt.to_string()),
        }],
        tools: vec![],
        max_tokens: 0,
        temperature: 0.0,
        system: None,
        thinking: None,
        extra: serde_json::Value::Object(extra),
    };

    let response = brain.complete("video", request).await.map_err(|e| {
        CarrierError::LlmDriver(format!(
            "Video generation failed: {e}. \
                 Do NOT retry video_generate with the same prompt. \
                 Tell the user the video generation service is currently unavailable \
                 and suggest trying again later."
        ))
    })?;

    let video_url = match response.media {
        Some(carrier_types::media::MediaOutput::Video { url, .. }) => url,
        _ => {
            return Err(CarrierError::LlmDriver(
                "Video generation returned no video".to_string(),
            ))
        }
    };

    let mut result = serde_json::Map::new();
    result.insert("video_url".into(), serde_json::json!(video_url));
    result.insert("provider".into(), serde_json::json!("brain"));
    result.insert(
        "note".into(),
        serde_json::json!(
            "Use weixin_send_video to send this video to a WeChat user, or download it directly."
        ),
    );

    serde_json::to_string_pretty(&result).map_err(|e| CarrierError::Serialization(e.to_string()))
}

// ---------------------------------------------------------------------------
// TTS / STT tools
// ---------------------------------------------------------------------------

async fn tool_text_to_speech(
    input: &serde_json::Value,
    brain: Option<&std::sync::Arc<dyn crate::llm_driver::Brain>>,
    home_dir: Option<&Path>,
    agent_name: Option<&str>,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
    external_url: Option<&str>,
) -> CarrierResult<String> {
    let brain = brain.ok_or(CarrierError::Config(
        "Brain not available. Ensure tts modality is configured.".to_string(),
    ))?;
    let text = input["text"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'text' parameter".to_string(),
    ))?;
    let voice = input["voice"].as_str();
    let format = input["format"].as_str();

    let mut extra = serde_json::Map::new();
    if let Some(v) = voice {
        extra.insert("voice".to_string(), serde_json::json!(v));
    }
    if let Some(f) = format {
        extra.insert("format".to_string(), serde_json::json!(f));
    }

    let request = crate::llm_driver::CompletionRequest {
        model: String::new(),
        messages: vec![carrier_types::message::Message {
            role: carrier_types::message::Role::User,
            content: carrier_types::message::MessageContent::Text(text.to_string()),
        }],
        tools: vec![],
        max_tokens: 0,
        temperature: 0.0,
        system: None,
        thinking: None,
        extra: serde_json::Value::Object(extra),
    };

    let response = brain
        .complete("tts", request)
        .await
        .map_err(|e| CarrierError::LlmDriver(format!("TTS brain call failed: {e}")))?;

    let media = response
        .media
        .ok_or(CarrierError::LlmDriver("TTS returned no media".to_string()))?;
    let (audio_data, format, duration_ms) = match media {
        carrier_types::media::MediaOutput::Audio {
            data,
            format,
            duration_ms,
        } => (data, format, duration_ms),
        _ => {
            return Err(CarrierError::LlmDriver(
                "TTS returned non-audio media".to_string(),
            ))
        }
    };

    // Save audio to per-sender output directory
    let mut saved_path: Option<String> = None;
    let mut view_url: Option<String> = None;
    if let (Some(hd), Some(an)) = (home_dir, agent_name) {
        let sid = sender_id.ok_or(CarrierError::Internal(
            "Cannot save audio: no sender context".to_string(),
        ))?;
        let oid = owner_id.unwrap_or(sid);
        let rel_dir = carrier_types::config::sender_relative_path(oid, an, Some(sid), "output");
        let output_dir = hd.join(&rel_dir);
        tokio::fs::create_dir_all(&output_dir)
            .await
            .map_err(|e| CarrierError::Internal(format!("Failed to create output dir: {e}")))?;

        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let filename = format!("tts_{timestamp}.{format}");
        let path = output_dir.join(&filename);

        tokio::fs::write(&path, &audio_data)
            .await
            .map_err(|e| CarrierError::Internal(format!("Failed to write audio file: {e}")))?;

        let rel = format!("output/{filename}");
        saved_path = Some(rel.clone());
        view_url = crate::file_view::build_file_view_url(external_url, an, &rel, sid);
    }

    let mut response = serde_json::json!({
        "saved_to": saved_path,
        "format": format,
        "provider": "brain",
        "duration_estimate_ms": duration_ms,
        "size_bytes": audio_data.len(),
    });
    if let Some(url) = view_url {
        response
            .as_object_mut()
            .unwrap()
            .insert("view_url".into(), serde_json::json!(url));
    }

    serde_json::to_string_pretty(&response).map_err(|e| CarrierError::Serialization(e.to_string()))
}

async fn tool_speech_to_text(
    input: &serde_json::Value,
    brain: Option<&std::sync::Arc<dyn crate::llm_driver::Brain>>,
    workspace_root: Option<&Path>,
) -> CarrierResult<String> {
    use base64::Engine;
    let brain = brain.ok_or(CarrierError::Config(
        "Brain not available. Ensure audio modality is configured.".to_string(),
    ))?;
    let raw_path = input["path"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'path' parameter".to_string(),
    ))?;
    let language = input["language"].as_str();

    let resolved = crate::tools::resolve_file_path(raw_path, workspace_root)?;

    // Read the audio file
    let data = tokio::fs::read(&resolved)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read audio file: {e}")))?;

    // Determine MIME type from extension
    let ext = resolved
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");
    let mime_type = match ext {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "webm" => "audio/webm",
        _ => "audio/mpeg",
    };

    let audio_block = carrier_types::message::ContentBlock::Audio {
        media_type: mime_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&data),
    };

    let mut extra = serde_json::Map::new();
    if let Some(lang) = language {
        extra.insert("language".to_string(), serde_json::json!(lang));
    }

    let request = crate::llm_driver::CompletionRequest {
        model: String::new(),
        messages: vec![carrier_types::message::Message {
            role: carrier_types::message::Role::User,
            content: carrier_types::message::MessageContent::Blocks(vec![audio_block]),
        }],
        tools: vec![],
        max_tokens: 4096,
        temperature: 0.0,
        system: None,
        thinking: None,
        extra: serde_json::Value::Object(extra),
    };

    let response = brain
        .complete("audio", request)
        .await
        .map_err(|e| CarrierError::LlmDriver(format!("Speech-to-text brain call failed: {e}")))?;

    let transcript = response.text();
    let result = serde_json::json!({
        "transcript": transcript,
        "provider": "brain",
    });

    serde_json::to_string_pretty(&result).map_err(|e| CarrierError::Serialization(e.to_string()))
}

// ---------------------------------------------------------------------------
// Docker sandbox tool
// ---------------------------------------------------------------------------
// Persistent process tools
// ---------------------------------------------------------------------------

/// Start a long-running process (REPL, server, watcher).
async fn tool_process_start(
    input: &serde_json::Value,
    pm: Option<&crate::process_manager::ProcessManager>,
    caller_agent_id: Option<&str>,
    exec_policy: Option<&ExecPolicy>,
    allowed_env_vars: Option<&[String]>,
) -> CarrierResult<String> {
    let pm = pm.ok_or(CarrierError::Internal(
        "Process manager not available".to_string(),
    ))?;
    let agent_id = caller_agent_id.ok_or(CarrierError::Internal(
        "Missing caller agent identity".to_string(),
    ))?;
    let command = input["command"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'command' parameter".to_string(),
    ))?;
    let args: Vec<String> = input["args"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let proc_id = pm
        .start(agent_id, command, &args, exec_policy, allowed_env_vars)
        .await?;
    Ok(serde_json::json!({
        "process_id": proc_id,
        "status": "started"
    })
    .to_string())
}

/// Read accumulated stdout/stderr from a process (non-blocking drain).
async fn tool_process_poll(
    input: &serde_json::Value,
    pm: Option<&crate::process_manager::ProcessManager>,
    caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let pm = pm.ok_or(CarrierError::Internal(
        "Process manager not available".to_string(),
    ))?;
    let agent_id = caller_agent_id.ok_or(CarrierError::Internal(
        "Missing caller agent identity".to_string(),
    ))?;
    let proc_id = input["process_id"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'process_id' parameter".to_string(),
        ))?;
    // Ownership: verify the process belongs to the caller
    if !pm.list(agent_id).iter().any(|p| p.id == proc_id) {
        return Err(CarrierError::InvalidInput(
            "Process not found or does not belong to you".to_string(),
        ));
    }
    let (stdout, stderr) = pm.read(proc_id).await?;
    Ok(serde_json::json!({
        "stdout": stdout,
        "stderr": stderr,
    })
    .to_string())
}

/// Write data to a process's stdin.
async fn tool_process_write(
    input: &serde_json::Value,
    pm: Option<&crate::process_manager::ProcessManager>,
    caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let pm = pm.ok_or(CarrierError::Internal(
        "Process manager not available".to_string(),
    ))?;
    let agent_id = caller_agent_id.ok_or(CarrierError::Internal(
        "Missing caller agent identity".to_string(),
    ))?;
    let proc_id = input["process_id"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'process_id' parameter".to_string(),
        ))?;
    // Ownership: verify the process belongs to the caller
    if !pm.list(agent_id).iter().any(|p| p.id == proc_id) {
        return Err(CarrierError::InvalidInput(
            "Process not found or does not belong to you".to_string(),
        ));
    }
    let data = input["data"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'data' parameter".to_string(),
    ))?;
    // Always append newline if not present (common expectation for REPLs)
    let data = if data.ends_with('\n') {
        data.to_string()
    } else {
        format!("{data}\n")
    };
    pm.write(proc_id, &data).await?;
    Ok(r#"{"status": "written"}"#.to_string())
}

/// Terminate a process.
async fn tool_process_kill(
    input: &serde_json::Value,
    pm: Option<&crate::process_manager::ProcessManager>,
    caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let pm = pm.ok_or(CarrierError::Internal(
        "Process manager not available".to_string(),
    ))?;
    let agent_id = caller_agent_id.ok_or(CarrierError::Internal(
        "Missing caller agent identity".to_string(),
    ))?;
    let proc_id = input["process_id"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'process_id' parameter".to_string(),
        ))?;
    // Ownership: verify the process belongs to the caller
    if !pm.list(agent_id).iter().any(|p| p.id == proc_id) {
        return Err(CarrierError::InvalidInput(
            "Process not found or does not belong to you".to_string(),
        ));
    }
    pm.kill(proc_id).await?;
    Ok(r#"{"status": "killed"}"#.to_string())
}

/// List processes for the current agent.
async fn tool_process_list(
    pm: Option<&crate::process_manager::ProcessManager>,
    caller_agent_id: Option<&str>,
) -> CarrierResult<String> {
    let pm = pm.ok_or(CarrierError::Internal(
        "Process manager not available".to_string(),
    ))?;
    let agent_id = caller_agent_id.ok_or(CarrierError::Internal(
        "Missing caller agent identity".to_string(),
    ))?;
    let procs = pm.list(agent_id);
    let list: Vec<serde_json::Value> = procs
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "command": p.command,
                "alive": p.alive,
                "uptime_secs": p.uptime_secs,
            })
        })
        .collect();
    Ok(serde_json::Value::Array(list).to_string())
}

// ---------------------------------------------------------------------------
// Canvas / A2UI tool
// ---------------------------------------------------------------------------

/// Sanitize HTML for canvas presentation.
///
/// SECURITY: Strips dangerous elements and attributes to prevent XSS:
/// - Rejects <script>, <iframe>, <object>, <embed>, <applet> tags
/// - Strips all on* event attributes (onclick, onload, onerror, etc.)
/// - Strips javascript:, data:text/html, vbscript: URLs
/// - Enforces size limit
fn sanitize_canvas_html(html: &str, max_bytes: usize) -> CarrierResult<String> {
    if html.is_empty() {
        return Err(CarrierError::InvalidInput("Empty HTML content".to_string()));
    }
    if html.len() > max_bytes {
        return Err(CarrierError::InvalidInput(format!(
            "HTML too large: {} bytes (max {})",
            html.len(),
            max_bytes
        )));
    }

    let lower = html.to_lowercase();

    // Reject dangerous tags
    let dangerous_tags = [
        "<script", "</script", "<iframe", "</iframe", "<object", "</object", "<embed", "<applet",
        "</applet",
    ];
    for tag in &dangerous_tags {
        if lower.contains(tag) {
            return Err(CarrierError::InvalidInput(format!(
                "Forbidden HTML tag detected: {tag}"
            )));
        }
    }

    // Reject event handler attributes (on*)
    // Match patterns like: onclick=, onload=, onerror=, onmouseover=, etc.
    static EVENT_PATTERN: std::sync::LazyLock<regex_lite::Regex> =
        std::sync::LazyLock::new(|| regex_lite::Regex::new(r"(?i)\bon[a-z]+\s*=").unwrap());
    if EVENT_PATTERN.is_match(html) {
        return Err(CarrierError::InvalidInput(
            "Forbidden event handler attribute detected (on* attributes are not allowed)"
                .to_string(),
        ));
    }

    // Reject dangerous URL schemes
    let dangerous_schemes = ["javascript:", "vbscript:", "data:text/html"];
    for scheme in &dangerous_schemes {
        if lower.contains(scheme) {
            return Err(CarrierError::InvalidInput(format!(
                "Forbidden URL scheme detected: {scheme}"
            )));
        }
    }

    Ok(html.to_string())
}

/// Canvas presentation tool handler.
async fn tool_canvas_present(
    input: &serde_json::Value,
    workspace_root: Option<&Path>,
    home_dir: Option<&Path>,
    agent_name: Option<&str>,
    owner_id: Option<&str>,
    sender_id: Option<&str>,
    external_url: Option<&str>,
) -> CarrierResult<String> {
    let html = input["html"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'html' parameter".to_string(),
    ))?;
    let title = input["title"].as_str().unwrap_or("Canvas");

    // Use configured max from task-local (set by agent_loop from KernelConfig), or default 512KB.
    let max_bytes = crate::tool_runner::CANVAS_MAX_BYTES
        .try_with(|v| *v)
        .unwrap_or(512 * 1024);
    let sanitized = sanitize_canvas_html(html, max_bytes)?;

    // Generate canvas ID
    let canvas_id = uuid::Uuid::new_v4().to_string();

    // Save to per-sender output directory
    let (output_dir, rel_dir) =
        if let (Some(_root), Some(hd), Some(an)) = (workspace_root, home_dir, agent_name) {
            let sid = sender_id.ok_or(CarrierError::Internal(
                "Cannot save canvas: no sender context".to_string(),
            ))?;
            let oid = owner_id.unwrap_or(sid);
            let rel = carrier_types::config::sender_relative_path(oid, an, Some(sid), "output");
            (hd.join(&rel), rel)
        } else {
            return Err(CarrierError::Internal(
                "Cannot save canvas: no workspace".to_string(),
            ));
        };
    let _ = tokio::fs::create_dir_all(&output_dir).await;

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let filename = format!(
        "canvas_{timestamp}_{}.html",
        crate::str_utils::safe_truncate_str(&canvas_id, 8)
    );
    let filepath = output_dir.join(&filename);

    // Write the full HTML document
    let full_html = format!(
        "<!DOCTYPE html>\n<html>\n<head><meta charset=\"utf-8\"><title>{title}</title></head>\n<body>\n{sanitized}\n</body>\n</html>"
    );
    tokio::fs::write(&filepath, &full_html)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to save canvas: {e}")))?;

    let rel = format!("output/{filename}");
    let sid = sender_id.unwrap_or("");
    let mut response = serde_json::json!({
        "canvas_id": canvas_id,
        "title": title,
        "saved_to": format!("{}/{}", rel_dir, filename),
        "rel_path": rel,
        "size_bytes": full_html.len(),
    });
    if let Some(an) = agent_name {
        if let Some(url) = crate::file_view::build_file_view_url(external_url, an, &rel, sid) {
            response
                .as_object_mut()
                .unwrap()
                .insert("view_url".into(), serde_json::json!(url));
        }
    }

    serde_json::to_string_pretty(&response).map_err(|e| CarrierError::Serialization(e.to_string()))
}
