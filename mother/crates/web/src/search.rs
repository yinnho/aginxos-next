//! web_search 实现 — AginxBrowser /search 聚合（M31 从 carrier-runtime
//! tools/web_search.rs 整体搬来，行为同构）。
//!
//! /search 原生聚合（baidu/sogou 等）+ fetch_top>0 时自动抓正文
//! （一步"搜→读"）。需要 `AGINXBROWSER_URL`（env 或
//! ~/.aginx/carrier/.env）——未设时明确报"Search not available"。

use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;

use crate::{aginxbrowser_url_opt, AGINXBROWSER_TIMEOUT_SECS};

pub async fn web_search(input: &Value) -> CarrierResult<String> {
    let base = match aginxbrowser_url_opt() {
        Some(u) => u,
        None => {
            return Err(CarrierError::Internal(
                "Search not available: AGINXBROWSER_URL not set".into(),
            ))
        }
    };
    do_search(&base, input).await
}

/// POST AginxBrowser /search and format results as Markdown.
async fn do_search(base_url: &str, input: &Value) -> CarrierResult<String> {
    let q = input["q"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing required parameter: q".into(),
    ))?;

    let mut body = serde_json::json!({
        "q": q,
    });

    // Optional parameters — only include if provided
    if let Some(v) = input["fetch_top"].as_u64() {
        body["fetch_top"] = v.into();
    }
    if let Some(v) = input["categories"].as_str() {
        body["categories"] = v.into();
    }
    if let Some(v) = input["language"].as_str() {
        body["language"] = v.into();
    }
    if let Some(v) = input["max_results"].as_u64() {
        body["max_results"] = v.into();
    }
    if let Some(v) = input["max_chars_per"].as_u64() {
        body["max_chars_per"] = v.into();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            AGINXBROWSER_TIMEOUT_SECS + 30,
        )) // search+fetch needs more time
        .build()
        .map_err(|e| CarrierError::Network(format!("Failed to create HTTP client: {e}")))?;

    let url = format!("{}/search", base_url.trim_end_matches('/'));
    let resp =
        client.post(&url).json(&body).send().await.map_err(|e| {
            CarrierError::Network(format!("AginxBrowser search request failed: {e}"))
        })?;

    let status = resp.status();
    if status.as_u16() == 503 {
        return Err(CarrierError::Network(
            "Search backend unavailable. AginxBrowser /search returned 503.".into(),
        ));
    }
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        let truncated: String = text.chars().take(500).collect();
        return Err(CarrierError::Network(format!(
            "AginxBrowser search error ({}): {}",
            status, truncated
        )));
    }

    let data: Value = resp.json().await.map_err(|e| {
        CarrierError::Serialization(format!("Failed to parse search response: {e}"))
    })?;

    let results = data["results"]
        .as_array()
        .ok_or(CarrierError::Serialization(
            "Malformed search response: missing results array".into(),
        ))?;

    if results.is_empty() {
        return Ok("No results found.".to_string());
    }

    let mut output = String::new();
    for (i, r) in results.iter().enumerate() {
        let title = r["title"].as_str().unwrap_or("(untitled)");
        let link = r["url"].as_str().unwrap_or("");
        let snippet = r["snippet"]
            .as_str()
            .or_else(|| r["content"].as_str())
            .unwrap_or("");

        output.push_str(&format!("### {}. {}\n", i + 1, title));
        output.push_str(&format!("{}\n", link));
        if !snippet.is_empty() {
            output.push_str(&format!("{}\n", snippet));
        }

        // Image direct link. AginxBrowser /search with categories=images returns
        // image_url as a binary-downloadable link (distinct from the page `url`
        // above) so the caller can `curl -o` it without scraping a page.
        if let Some(img) = r["image_url"].as_str() {
            if !img.is_empty() {
                output.push_str(&format!("image_url: {}\n", img));
            }
        }

        // Full content (only present when fetch_top > 0 and index < fetch_top)
        if let Some(content) = r["content"].as_str() {
            if !content.is_empty() {
                output.push_str(&format!("\n**Full content:**\n{}\n", content));
                if r["content_truncated"].as_bool().unwrap_or(false) {
                    output.push_str("(content truncated)\n");
                }
            }
        }
        if let Some(err) = r["fetch_error"].as_str() {
            if !err.is_empty() {
                output.push_str(&format!("⚠️ Fetch error: {}\n", err));
            }
        }

        output.push_str("---\n");
    }

    let total = data["number_of_results"]
        .as_u64()
        .map(|n| format!("{} total results", n))
        .unwrap_or_default();

    let backend = data["search_backend"].as_str().unwrap_or("unknown");

    output.push_str(&format!(
        "\n{} shown, {}. Backend: {}\n",
        results.len(),
        total,
        backend
    ));

    // Truncate very long outputs (char-boundary-safe to avoid panic on multi-byte UTF-8)
    if output.len() > 60_000 {
        let truncated = carrier_types::truncate_str(&output, 50_000);
        output = format!("{}... [truncated, {} total chars]", truncated, output.len());
    }

    Ok(output)
}
