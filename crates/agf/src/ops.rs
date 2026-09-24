//! 工具语义实现 —— 从 runtime tools/filessystem.rs / media.rs 同构搬来
//! （M32 D3 批2）。与原实现的差异只有一处：路径不再在此解析，而是认
//! `_ctx.resolved.<param>`（runtime 桥预解析）或原样 CWD 相对。行为细节
//! —— 二进制拒绝文案、目录→file_list 纠偏、markitdown/pandoc 编排、
//! U+FFFD 拒写、input/ 只读、view_url 拼法 —— 逐字节保留。

use crate::{ctx_of, resolve_param};
use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;
use std::path::{Path, PathBuf};

fn str_param<'a>(input: &'a Value, key: &str) -> CarrierResult<&'a str> {
    input[key].as_str().ok_or_else(|| {
        CarrierError::InvalidInput(format!("Missing '{key}' parameter"))
    })
}

// ---------------------------------------------------------------------------
// file_read
// ---------------------------------------------------------------------------

/// Detect common binary file types from magic bytes.
/// Returns a human-readable kind (e.g. "PNG 图片") so we can tell the LLM
/// to use image_analyze instead of file_read.
fn detect_binary_kind(header: &[u8]) -> Option<&'static str> {
    if header.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("PNG 图片")
    } else if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("JPEG 图片")
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        Some("GIF 图片")
    } else if header.starts_with(b"RIFF") && header.len() > 11 && &header[8..12] == b"WEBP" {
        Some("WebP 图片")
    } else if header.len() > 4 && &header[4..8] == b"ftyp" {
        Some("视频文件")
    } else if header.starts_with(&[0x25, 0x50, 0x44, 0x46]) {
        Some("PDF 文档")
    } else if header.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        Some("ZIP 压缩包")
    } else {
        None
    }
}

/// Binary document formats file_read can't read as text, but markitdown can
/// extract. Images/video are intentionally NOT here - those go to image_analyze.
const DOCUMENT_EXTS: &[&str] = &[
    "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "odt", "ods", "odp", "rtf", "epub",
];

/// Return the lowercased extension if `path` is a document format markitdown
/// handles, else None.
fn document_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    if DOCUMENT_EXTS.contains(&ext.as_str()) {
        Some(ext)
    } else {
        None
    }
}

/// Extract text from a binary document (pdf/docx/xlsx/pptx/...) by shelling out
/// to `markitdown`, which converts many formats to markdown for LLM consumption.
/// Mirrors the `file_convert` (pandoc) shell-out pattern. Returns an error
/// (never falls back to a raw text read) when markitdown is absent or fails,
/// since the file is binary and unreadable as text.
async fn extract_document_with_markitdown(path: &Path, raw_path: &str) -> CarrierResult<String> {
    // Guard against huge files - markitdown + its parsers can be slow/memory-heavy.
    let size = tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if size > 50 * 1024 * 1024 {
        return Err(CarrierError::InvalidInput(format!(
            "文件 '{raw_path}' 太大（{size} bytes，上限 50MB），无法提取文本。"
        )));
    }

    let out = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        tokio::process::Command::new("markitdown").arg(path).output(),
    )
    .await;

    let output = match out {
        Ok(Ok(o)) => o,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CarrierError::Config(
                "未安装 markitdown，无法读取文档格式（pdf/docx/xlsx/pptx 等）。\
                 请管理员安装：pip install 'markitdown[all]'。"
                    .to_string(),
            ))
        }
        Ok(Err(e)) => return Err(CarrierError::Internal(format!("运行 markitdown 失败：{e}"))),
        Err(_) => {
            return Err(CarrierError::Internal(format!(
                "markitdown 提取 '{raw_path}' 超时（120s）。文件可能过大或格式异常。"
            )))
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CarrierError::Internal(format!(
            "markitdown 提取 '{raw_path}' 失败：{}",
            stderr.trim()
        )));
    }

    let md = String::from_utf8_lossy(&output.stdout).into_owned();
    if md.trim().is_empty() {
        return Err(CarrierError::InvalidInput(format!(
            "markitdown 从 '{raw_path}' 提取的内容为空（可能是扫描件/图片型 PDF 或受保护文档）。\
             图片型内容可用 image_analyze。"
        )));
    }
    // Truncate very large extractions (char-safe) so we don't blow the context.
    if md.len() > 200_000 {
        let head: String = md.chars().take(50_000).collect();
        Ok(format!(
            "{head}\n\n…（内容过长，已截断显示前 50000 字符，原文共 {n} 字节）",
            n = md.len()
        ))
    } else {
        Ok(md)
    }
}

/// Actionable error when file_read is asked to read a directory. A directory
/// path is the #1 trigger of file_read tool loops: the agent retries on
/// *different* dir paths, each producing a cryptic OS error and evading the
/// exact-match loop guard. Steer it to file_list — mirroring file_list's
/// reverse hint when it is given a file.
fn directory_read_hint(raw_path: &str) -> String {
    format!(
        "路径 '{raw_path}' 是一个目录，不是文件。file_read 只能读取文件内容，不能读目录。\n\
         修正方法：\n\
         - 想列出该目录下的文件 → 用 file_list(path=\"{raw_path}\")\n\
         - 想读取目录里的某个文件 → 用 file_read 并补上文件名（例如 {raw_path}/正文.md）"
    )
}

pub async fn file_read(input: &Value) -> CarrierResult<String> {
    let raw_path = str_param(input, "path")?;
    let resolved = resolve_param(input, "path", raw_path)?;

    tracing::info!(raw_path, resolved = %resolved.display(), "file_read resolved path");

    // Binary document formats (pdf/docx/xlsx/pptx/odt/...) - extract text via
    // markitdown so the agent can read user-sent documents, not just plain text.
    // Images/video are not documents and fall through to the binary-refuse path
    // (use image_analyze). markitdown not installed => clear error (no fallback
    // to a raw text read, since the file is binary).
    if document_extension(&resolved).is_some() {
        return extract_document_with_markitdown(&resolved, raw_path).await;
    }

    // Friendly error: detect binary files (images, etc.) before reading.
    // file_read only handles text; binary files should use image_analyze etc.
    if let Ok(metadata) = tokio::fs::metadata(&resolved).await {
        if metadata.is_file() {
            // Check magic bytes to detect common binary formats
            if let Ok(header) = tokio::fs::read(&resolved).await {
                let kind = detect_binary_kind(&header);
                if let Some(kind) = kind {
                    return Err(CarrierError::InvalidInput(format!(
                        "文件 '{raw_path}' 是二进制文件（{kind}），file_read 只能读取文本文件。\
                         如果是图片，请用 image_analyze 工具分析；如果是其他二进制文件，\
                         请直接使用它的路径/URL，不需要读取内容。"
                    )));
                }
            }
        } else if metadata.is_dir() {
            // Reading a directory is the #1 file_read loop trigger (see
            // directory_read_hint): without an actionable hint the agent retries
            // on different dir paths and evades the exact-match loop guard.
            return Err(CarrierError::InvalidInput(directory_read_hint(raw_path)));
        }
    }

    tokio::fs::read_to_string(&resolved).await.map_err(|e| {
        // Friendly message for UTF-8 decode failures on text files
        if e.to_string().contains("stream did not contain valid UTF-8")
            || e.to_string().contains("invalid utf-8")
        {
            CarrierError::InvalidInput(format!(
                "文件 '{raw_path}' 包含非 UTF-8 内容（可能是二进制文件）。\
                     file_read 只能读文本。如果是图片，请用 image_analyze；\
                     如果是文档，请确认文件格式或使用对应的解析工具。"
            ))
        } else {
            CarrierError::Internal(format!("Failed to read file: {e}"))
        }
    })
}

// ---------------------------------------------------------------------------
// file_write
// ---------------------------------------------------------------------------

pub async fn file_write(input: &Value) -> CarrierResult<String> {
    let raw_path = str_param(input, "path")?;

    // Reject replacement characters (U+FFFD) in paths: a corrupted filename
    // (e.g. LLM emitting broken UTF-8 for a Chinese name) is un-typeable by
    // the model afterwards, so any follow-up read/patch/delete of the file
    // fails and loops (2026-08-21 86bus incident).
    if raw_path.contains('\u{FFFD}') {
        return Err(CarrierError::InvalidInput(format!(
            "路径 '{raw_path}' 含损坏字符（U+FFFD），无法写入。请换一个干净的文件名（中文名或 ASCII 名，例如 output/material.md）重试。"
        )));
    }

    // input/ is the user's inbox (attachments they sent, saved by the channel
    // bridge). It's read-only from the agent's side - block writes here so a
    // file_write can't overwrite a received file. Direct output to output/.
    let normalized = raw_path.replace('\\', "/");
    if normalized == "input" || normalized.starts_with("input/") {
        return Err(CarrierError::InvalidInput(
            "input/ 是用户发来的文件收件箱（只读），请改用 output/ 前缀写文件。".to_string(),
        ));
    }

    let resolved = resolve_param(input, "path", raw_path)?;
    let content = str_param(input, "content")?;
    if let Some(parent) = resolved.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| CarrierError::Internal(format!("Failed to create directories: {e}")))?;
    }
    tokio::fs::write(&resolved, content)
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to write file: {e}")))?;

    // Public view URL so any clone can paste a clickable link (system capability).
    let mut msg = format!("Successfully wrote {} bytes to {}", content.len(), raw_path);
    if let Some(ctx) = ctx_of(input) {
        if let (Some(an), Some(sid)) = (ctx.agent_name.as_deref(), ctx.sender_id.as_deref()) {
            if let Some(rel) = crate::view_url::rel_path_for_user_write(raw_path) {
                if let Some(url) =
                    crate::view_url::build_file_view_url(ctx.external_url.as_deref(), an, &rel, sid)
                {
                    msg.push_str(&format!(
                        "\nview_url: {url}\n(将 view_url 贴给用户即可在浏览器中打开；勿把全文粘进聊天。)"
                    ));
                }
            }
        }
    }
    Ok(msg)
}

// ---------------------------------------------------------------------------
// file_list
// ---------------------------------------------------------------------------

pub async fn file_list(input: &Value) -> CarrierResult<String> {
    let raw_path = str_param(input, "path")?;
    let resolved = resolve_param(input, "path", raw_path)?;

    // For user-data paths (output/ memory/), treat missing directory as empty
    let is_user_data = raw_path.starts_with("output/")
        || raw_path == "output"
        || raw_path.starts_with("memory/")
        || raw_path == "memory";

    // Friendly error: if path points to a file (not a directory), tell the
    // LLM clearly instead of returning the cryptic OS "Not a directory" error.
    if let Ok(metadata) = tokio::fs::metadata(&resolved).await {
        if metadata.is_file() {
            return Err(CarrierError::InvalidInput(format!(
                "路径 '{raw_path}' 是一个文件，不是目录。file_list 只能列出目录内容。\n\
                 修正方法：\n\
                 - 想读取这个文件内容 → 用 file_read(path=\"{raw_path}\")\n\
                 - 想列出它所在的目录 → 用 file_list 并去掉文件名（例如列出上级目录）"
            )));
        }
    }

    let read_dir_result = tokio::fs::read_dir(&resolved).await;
    let mut entries = match read_dir_result {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && is_user_data => {
            return Ok("(empty directory)".to_string());
        }
        Err(e) => {
            return Err(CarrierError::Internal(format!(
                "Failed to list directory: {e}"
            )))
        }
    };
    let mut files = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| CarrierError::Internal(format!("Failed to read entry: {e}")))?
    {
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata().await;
        let suffix = match metadata {
            Ok(m) if m.is_dir() => "/",
            _ => "",
        };
        files.push(format!("{name}{suffix}"));
    }
    files.sort();
    if files.is_empty() {
        Ok("(empty directory)".to_string())
    } else {
        Ok(files.join("\n"))
    }
}

// ---------------------------------------------------------------------------
// file_convert
// ---------------------------------------------------------------------------

pub async fn file_convert(input: &Value) -> CarrierResult<String> {
    let raw_input_path = str_param(input, "input_path")?;
    let output_format = str_param(input, "output_format")?;
    let raw_output_path = input["output_path"].as_str();

    let input_path = resolve_param(input, "input_path", raw_input_path)?;
    if !input_path.exists() {
        return Err(CarrierError::InvalidInput(format!(
            "Input file not found: {}",
            input_path.display()
        )));
    }
    let metadata = std::fs::metadata(&input_path)
        .map_err(|e| CarrierError::Internal(format!("Cannot read input file metadata: {e}")))?;
    if metadata.len() > 50 * 1024 * 1024 {
        return Err(CarrierError::InvalidInput(format!(
            "Input file too large: {} bytes (max 50MB)",
            metadata.len()
        )));
    }

    let output_path = if let Some(op) = raw_output_path {
        resolve_param(input, "output_path", op)?
    } else {
        // Auto-generated output path — use top-level senders directory
        // (needs the identity the bridge injected in `_ctx`).
        let input_stem = input_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("converted");
        let output_dir = match ctx_of(input) {
            Some(ctx)
                if ctx.home_dir.is_some()
                    && ctx.agent_name.is_some()
                    && ctx.sender_id.is_some() =>
            {
                let hd = ctx.home_dir.unwrap();
                let sender = ctx.sender_id.as_deref().unwrap();
                let oid = ctx.owner_id.as_deref().unwrap_or(sender);
                let agent = ctx.agent_name.as_deref().unwrap();
                carrier_types::config::sender_data_dir(&hd, oid, agent, Some(sender)).join("output")
            }
            _ => PathBuf::from("output"),
        };
        let _ = std::fs::create_dir_all(&output_dir);
        output_dir.join(format!("{input_stem}.{output_format}"))
    };

    let mut cmd = tokio::process::Command::new("pandoc");
    cmd.arg(&input_path)
        .arg("-t")
        .arg(output_format)
        .arg("-o")
        .arg(&output_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = cmd.spawn().map_err(|e| {
        CarrierError::Internal(format!("Failed to run pandoc (is it installed?): {e}"))
    })?;

    let output = tokio::time::timeout(std::time::Duration::from_secs(60), child.wait_with_output())
        .await
        .map_err(|_| CarrierError::Internal("Pandoc timed out after 60 seconds".to_string()))
        .and_then(|r| {
            r.map_err(|e| CarrierError::Internal(format!("Pandoc process error: {e}")))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(CarrierError::Internal(format!(
            "Pandoc conversion failed: {stderr}"
        )));
    }

    if !output_path.exists() {
        return Err(CarrierError::Internal(
            "Pandoc completed but no output file was produced".to_string(),
        ));
    }

    let out_size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(format!(
        "Successfully converted {} -> {}\nInput: {} ({} bytes)\nOutput: {} ({} bytes)",
        raw_input_path,
        output_format,
        input_path.display(),
        metadata.len(),
        output_path.display(),
        out_size,
    ))
}

// ---------------------------------------------------------------------------
// image_analyze（自 media.rs 移交，M32）
// ---------------------------------------------------------------------------

/// Detect image format from magic bytes.
fn detect_image_format(data: &[u8]) -> String {
    if data.len() < 4 {
        return "unknown".to_string();
    }
    if data.starts_with(b"\x89PNG") {
        "png".to_string()
    } else if data.starts_with(b"\xFF\xD8\xFF") {
        "jpeg".to_string()
    } else if data.starts_with(b"GIF8") {
        "gif".to_string()
    } else if data.starts_with(b"RIFF") && data.len() > 12 && &data[8..12] == b"WEBP" {
        "webp".to_string()
    } else if data.starts_with(b"BM") {
        "bmp".to_string()
    } else if data.starts_with(b"\x00\x00\x01\x00") {
        "ico".to_string()
    } else {
        "unknown".to_string()
    }
}

/// Extract image dimensions from common formats.
fn extract_image_dimensions(data: &[u8], format: &str) -> Option<(u32, u32)> {
    match format {
        "png" => {
            // PNG: IHDR chunk starts at byte 16, width at 16-19, height at 20-23
            if data.len() >= 24 {
                let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
                let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
                Some((w, h))
            } else {
                None
            }
        }
        "gif" => {
            // GIF: width at bytes 6-7, height at 8-9 (little-endian)
            if data.len() >= 10 {
                let w = u16::from_le_bytes([data[6], data[7]]) as u32;
                let h = u16::from_le_bytes([data[8], data[9]]) as u32;
                Some((w, h))
            } else {
                None
            }
        }
        "bmp" => {
            // BMP: width at bytes 18-21, height at 22-25 (little-endian)
            if data.len() >= 26 {
                let w = u32::from_le_bytes([data[18], data[19], data[20], data[21]]);
                let h = u32::from_le_bytes([data[22], data[23], data[24], data[25]]);
                Some((w, h))
            } else {
                None
            }
        }
        "jpeg" => {
            // JPEG: scan for SOF0 marker (0xFF 0xC0) to find dimensions
            extract_jpeg_dimensions(data)
        }
        _ => None,
    }
}

/// Extract JPEG dimensions by scanning for SOF markers.
fn extract_jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut i = 2; // Skip SOI marker
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // SOF0-SOF3 markers contain dimensions
        if (0xC0..=0xC3).contains(&marker) && i + 9 < data.len() {
            let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
            let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
            return Some((w, h));
        }
        if i + 3 < data.len() {
            let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            i += 2 + seg_len;
        } else {
            break;
        }
    }
    None
}

/// Format file size in human-readable form.
fn format_file_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

pub async fn image_analyze(input: &Value) -> CarrierResult<String> {
    let path = str_param(input, "path")?;
    let prompt = input["prompt"].as_str().unwrap_or("");

    // 绝对路径（含 /tmp 截图）与桥预解析值直接用；其余按 CWD。
    let resolved = resolve_param(input, "path", path)?;

    let data = tokio::fs::read(&resolved).await.map_err(|e| {
        CarrierError::Internal(format!(
            "Failed to read image '{path}' ({}): {e}",
            resolved.display()
        ))
    })?;

    let file_size = data.len();

    // Detect image format from magic bytes
    let format = detect_image_format(&data);

    // Extract dimensions for common formats
    let dimensions = extract_image_dimensions(&data, &format);

    // Base64-encode (truncate for very large images in the response)
    let base64_preview = if file_size <= 512 * 1024 {
        // Under 512KB — include full base64
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&data)
    } else {
        // Over 512KB — include first 64KB preview
        use base64::Engine;
        let preview_bytes = &data[..64 * 1024];
        format!(
            "{}... [truncated, {} total bytes]",
            base64::engine::general_purpose::STANDARD.encode(preview_bytes),
            file_size
        )
    };

    let mut result = serde_json::json!({
        "path": path,
        "format": format,
        "file_size_bytes": file_size,
        "file_size_human": format_file_size(file_size),
    });

    if let Some((w, h)) = dimensions {
        result["width"] = serde_json::json!(w);
        result["height"] = serde_json::json!(h);
    }

    if !prompt.is_empty() {
        result["prompt"] = serde_json::json!(prompt);
        result["note"] = serde_json::json!(
            "Vision analysis requires a vision-capable LLM. The base64 data is included for downstream processing."
        );
    }

    result["base64_preview"] = serde_json::json!(base64_preview);

    serde_json::to_string_pretty(&result).map_err(|e| CarrierError::Serialization(e.to_string()))
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agf-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[tokio::test]
    async fn file_write_rejects_replacement_char_path() {
        // A corrupted filename (LLM emitting broken UTF-8) is un-typeable by
        // the model afterwards - every follow-up read/patch/delete fails and
        // loops (2026-08-21 86bus incident). Reject at write time.
        let input = serde_json::json!({
            "path": "output/p/\u{FFFD}\u{FFFD}材.md",
            "content": "x",
        });
        let err = file_write(&input).await.unwrap_err();
        assert!(err.to_string().contains("U+FFFD"), "{err}");
    }

    #[tokio::test]
    async fn file_write_blocks_input_inbox() {
        let input = serde_json::json!({"path": "input/收到.md", "content": "x"});
        let err = file_write(&input).await.unwrap_err();
        assert!(err.to_string().contains("只读"), "{err}");
    }

    #[tokio::test]
    async fn write_read_list_roundtrip_human_face() {
        let d = tmp_dir("roundtrip");
        let input = serde_json::json!({
            "path": (d.join("output/a.md").to_str().unwrap()),
            "content": "hello agf",
        });
        let msg = file_write(&input).await.unwrap();
        assert!(msg.starts_with("Successfully wrote 9 bytes to"), "{msg}");
        assert!(!msg.contains("view_url"), "human face has no view_url");

        let rd = file_read(&serde_json::json!({"path": d.join("output/a.md").to_str().unwrap()}))
            .await
            .unwrap();
        assert_eq!(rd, "hello agf");

        let ls = file_list(&serde_json::json!({"path": d.join("output").to_str().unwrap()}))
            .await
            .unwrap();
        assert!(ls.contains("a.md"), "{ls}");

        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn file_write_appends_view_url_from_ctx() {
        let d = tmp_dir("viewurl");
        let input = serde_json::json!({
            "path": "output/r.md",
            "content": "x",
            "_ctx": {
                "home_dir": d.to_str().unwrap(),
                "sender_id": "u1",
                "owner_id": null,
                "agent_name": "ag",
                "external_url": "https://x.example",
                "resolved": { "path": d.join("r.md").to_str().unwrap() }
            }
        });
        let msg = file_write(&input).await.unwrap();
        assert!(msg.contains("view_url: https://x.example/api/files/view/ag/output/r.md?sender_id=u1"), "{msg}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn file_read_directory_hint_steer_to_file_list() {
        let d = tmp_dir("dirhint");
        let err = file_read(&serde_json::json!({"path": d.to_str().unwrap()}))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("file_list"), "{msg}");
        assert!(msg.contains("file_read"), "{msg}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn file_list_file_hint_steer_to_file_read() {
        let d = tmp_dir("filehint");
        let f = d.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let err = file_list(&serde_json::json!({"path": f.to_str().unwrap()}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("file_read"), "{}", err);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[tokio::test]
    async fn file_read_missing_user_data_list_is_empty() {
        // output/memory 前缀目录不存在 → (empty directory)，不是报错
        let out = file_list(&serde_json::json!({"path": "output"})).await.unwrap();
        assert_eq!(out, "(empty directory)");
    }

    #[tokio::test]
    async fn file_read_rejects_binary_magic() {
        let d = tmp_dir("binary");
        let f = d.join("img.png");
        std::fs::write(&f, [0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3]).unwrap();
        let err = file_read(&serde_json::json!({"path": f.to_str().unwrap()}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("PNG 图片"), "{err}");
        assert!(err.to_string().contains("image_analyze"), "{err}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn document_extension_detects_formats() {
        assert_eq!(
            document_extension(Path::new("foo.pdf")),
            Some("pdf".to_string())
        );
        assert_eq!(
            document_extension(Path::new("销售.XLSX")),
            Some("xlsx".to_string())
        );
        assert_eq!(
            document_extension(Path::new("input/report.docx")),
            Some("docx".to_string())
        );
        assert_eq!(document_extension(Path::new("notes.md")), None);
        assert_eq!(document_extension(Path::new("noext")), None);
    }

    #[tokio::test]
    async fn image_analyze_png_dimensions() {
        let d = tmp_dir("inspect");
        let f = d.join("x.png");
        // Minimal PNG header with IHDR 2x3
        let mut png = vec![0x89u8, 0x50, 0x4E, 0x47];
        png.extend_from_slice(&[0; 12]); // pad to IHDR dims at 16..24
        png.extend_from_slice(&[0, 0, 0, 2]); // width=2
        png.extend_from_slice(&[0, 0, 0, 3]); // height=3
        std::fs::write(&f, png).unwrap();
        let out = image_analyze(&serde_json::json!({"path": f.to_str().unwrap()}))
            .await
            .unwrap();
        assert!(out.contains("\"width\": 2"), "{out}");
        assert!(out.contains("\"height\": 3"), "{out}");
        assert!(out.contains("\"format\": \"png\""), "{out}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn directory_read_hint_contents() {
        let msg = directory_read_hint("output/pipeline-20260725-x");
        assert!(msg.contains("file_list"), "{msg}");
        assert!(msg.contains("file_read"), "{msg}");
        assert!(msg.contains("output/pipeline-20260725-x"), "{msg}");
    }
}
