//! Document generation tool: document_generate.
//!
//! Generates Office documents (docx/pptx/pdf) from Markdown content via Pandoc.
//! Replaces the office-* system flows, which relied on the LLM authoring a
//! throwaway Python script (python-docx / python-pptx / reportlab) per request.
//! Here the agent passes markdown + format; the tool renders via Pandoc — no
//! ad-hoc script generation, no shell elevation needed.

use crate::tool_context::ToolContext;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::tool::{PermissionLevel, ToolDefinition};

pub struct DocumentTools;

#[async_trait]
impl super::ToolModule for DocumentTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "document_generate".to_string(),
            description: "Generate an Office document (docx/pptx/pdf) from Markdown content via Pandoc. Use this to produce Word/PowerPoint/PDF deliverables directly from structured markdown (headings, lists, tables). Returns the output file path. For xlsx, use shell_exec with openpyxl instead.".to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "format": { "type": "string", "enum": ["docx", "pptx", "pdf"], "description": "Target document format" },
                    "content": { "type": "string", "description": "Markdown content of the document" },
                    "output_path": { "type": "string", "description": "Optional output filename (relative to the sender output/ dir, e.g. 'report.docx'). Auto-named if omitted." }
                },
                "required": ["format", "content"]
            }),
        }]
    }

    async fn execute(
        &self,
        name: &str,
        input: &Value,
        ctx: &ToolContext<'_>,
    ) -> Option<CarrierResult<String>> {
        match name {
            "document_generate" => Some(tool_document_generate(input, ctx).await),
            _ => None,
        }
    }

    fn permission_level(&self, _tool_name: &str) -> PermissionLevel {
        PermissionLevel::Write
    }
}

async fn tool_document_generate(input: &Value, ctx: &ToolContext<'_>) -> CarrierResult<String> {
    let format = input["format"].as_str().ok_or_else(|| {
        CarrierError::InvalidInput("Missing 'format' (docx/pptx/pdf)".to_string())
    })?;
    let content = input["content"]
        .as_str()
        .ok_or_else(|| CarrierError::InvalidInput("Missing 'content' (markdown)".to_string()))?;
    let raw_output_path = input["output_path"].as_str();

    let format_lower = format.to_lowercase();
    if !matches!(format_lower.as_str(), "docx" | "pptx" | "pdf") {
        return Err(CarrierError::InvalidInput(format!(
            "Unsupported format '{format}'. Supported: docx, pptx, pdf. (For xlsx generation, use shell_exec with openpyxl.)"
        )));
    }

    // Resolve output dir (sender output/) + filename.
    let sender = ctx.sender_id.unwrap_or("unknown");
    let agent = ctx.agent_name.unwrap_or("unknown");
    let oid = ctx.owner_id.unwrap_or(sender);
    let output_dir = if let Some(hd) = ctx.home_dir {
        let dir = carrier_types::config::sender_data_dir(hd, oid, agent, Some(sender)).join("output");
        let _ = std::fs::create_dir_all(&dir);
        dir
    } else {
        PathBuf::from("output")
    };

    // Filename: sanitize the user-supplied name (basename only), else auto.
    let filename = match raw_output_path {
        Some(name) => std::path::Path::new(name)
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("document.{format_lower}")),
        None => format!("document.{format_lower}"),
    };
    let output_path = output_dir.join(&filename);

    // Markdown source alongside the output (same stem, .src.md), removed after.
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("document");
    let temp_md = output_dir.join(format!("{stem}.src.md"));
    std::fs::write(&temp_md, content)
        .map_err(|e| CarrierError::Internal(format!("Failed to write temp markdown: {e}")))?;

    let mut cmd = tokio::process::Command::new("pandoc");
    cmd.arg(&temp_md)
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
        .map_err(|_| CarrierError::Internal("Pandoc timed out after 60 seconds".to_string()))?
        .map_err(|e| CarrierError::Internal(format!("Pandoc process error: {e}")))?;

    let _ = std::fs::remove_file(&temp_md);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(CarrierError::Internal(format!(
            "Pandoc generation failed (format={format_lower}): {stderr}"
        )));
    }
    if !output_path.exists() {
        return Err(CarrierError::Internal(
            "Pandoc completed but produced no output file".to_string(),
        ));
    }

    let out_size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);
    Ok(format!(
        "Generated {} document ({} bytes):\n{}",
        format_lower,
        out_size,
        output_path.display()
    ))
}
