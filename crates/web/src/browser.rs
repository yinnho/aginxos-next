//! browser_* 工具实现 — AginxBrowser HTTP API 客户端（M31 从
//! carrier-runtime tools/browser.rs 整体搬来，行为同构）。
//!
//! - navigate/read_page: fetch 页面（markdown/html/text，CSS selector 提取）
//! - click: JS element.click() 后回读页面文本
//! - evaluate: 页面上跑任意 JS
//! - type/scroll/wait: evaluate 模拟
//! - back/close: AginxBrowser 无状态 → 提示文本
//! - screenshot: 不支持（轻量引擎无渲染），明确报错

use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;

use crate::{aginxbrowser_url, AGINXBROWSER_TIMEOUT_SECS};

/// Shared AginBrowser HTTP request — POST to a given path, return JSON response.
async fn do_aginxbrowser_request(path: &str, req_body: Value) -> CarrierResult<Value> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(AGINXBROWSER_TIMEOUT_SECS))
        .build()
        .map_err(|e| CarrierError::Network(format!("Failed to create HTTP client: {e}")))?;

    let url = format!("{}/{}", aginxbrowser_url(), path);
    let resp = client
        .post(&url)
        .json(&req_body)
        .send()
        .await
        .map_err(|e| CarrierError::Network(format!("AginxBrowser request failed: {e}")))?;

    let status = resp.status();
    let body = resp.json::<Value>().await.map_err(|e| {
        CarrierError::Serialization(format!("Failed to parse AginxBrowser response: {e}"))
    })?;

    if !status.is_success() {
        let err = body["error"].as_str().unwrap_or("Unknown error");
        return Err(CarrierError::Network(format!(
            "AginxBrowser error ({}): {}",
            status, err
        )));
    }

    Ok(body)
}

async fn do_fetch_request(req_body: Value) -> CarrierResult<Value> {
    do_aginxbrowser_request("fetch", req_body).await
}

async fn do_eval_request(req_body: Value) -> CarrierResult<Value> {
    do_aginxbrowser_request("eval", req_body).await
}

pub async fn navigate(input: &Value) -> CarrierResult<String> {
    let url = input["url"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'url' parameter".to_string(),
    ))?;
    let format = input["format"].as_str().unwrap_or("markdown");
    let selector = input["selector"].as_str();
    let wait_secs = input["wait_secs"].as_u64();
    let use_proxy = input["use_proxy"].as_bool().unwrap_or(false);

    let mut req_body = serde_json::json!({
        "url": url,
        "format": format,
        "use_proxy": use_proxy,
    });
    if let Some(s) = selector {
        req_body["selector"] = s.into();
    }
    if let Some(w) = wait_secs {
        req_body["wait_secs"] = w.into();
    }

    let resp = do_fetch_request(req_body).await?;

    let title = resp["title"].as_str().unwrap_or("");
    let content = resp["content"].as_str().unwrap_or("");
    let final_url = resp["url"].as_str().unwrap_or(url);

    let result = if !title.is_empty() {
        format!("Title: {}\nURL: {}\n\n{}", title, final_url, content)
    } else {
        format!("URL: {}\n\n{}", final_url, content)
    };

    Ok(result)
}

pub async fn click(input: &Value) -> CarrierResult<String> {
    let url = input["url"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'url' parameter".to_string(),
    ))?;
    let selector = input["selector"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'selector' parameter".to_string(),
        ))?;
    let wait_secs = input["wait_secs"].as_u64();

    let mut req_body = serde_json::json!({
        "url": url,
        "selector": selector,
    });
    if let Some(w) = wait_secs {
        req_body["wait_secs"] = w.into();
    }

    let resp = do_aginxbrowser_request("click", req_body).await?;

    let clicked = resp["clicked"].as_bool().unwrap_or(false);
    let text_after = resp["text_after"].as_str().unwrap_or("");
    let final_url = resp["url"].as_str().unwrap_or(url);

    let result = if clicked {
        format!(
            "Clicked element '{}'.\nURL: {}\n\nPage text after click:\n{}",
            selector, final_url, text_after
        )
    } else {
        format!(
            "Element '{}' not found on page.\nURL: {}",
            selector, final_url
        )
    };

    Ok(result)
}

pub async fn evaluate(input: &Value) -> CarrierResult<String> {
    let url = input["url"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'url' parameter".to_string(),
    ))?;
    let script = input["script"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'script' parameter".to_string(),
    ))?;
    let wait_secs = input["wait_secs"].as_u64();

    let mut req_body = serde_json::json!({
        "url": url,
        "script": script,
    });
    if let Some(w) = wait_secs {
        req_body["wait_secs"] = w.into();
    }

    let resp = do_eval_request(req_body).await?;

    let result = &resp["result"];
    let final_url = resp["url"].as_str().unwrap_or(url);

    let result_str = serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string());

    Ok(format!("URL: {}\n\nResult:\n{}", final_url, result_str))
}

pub async fn r#type(input: &Value) -> CarrierResult<String> {
    let url = input["url"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'url' parameter".to_string(),
    ))?;
    let selector = input["selector"]
        .as_str()
        .ok_or(CarrierError::InvalidInput(
            "Missing 'selector' parameter".to_string(),
        ))?;
    let text = input["text"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'text' parameter".to_string(),
    ))?;

    let script = format!(
        r#"(function() {{
            var el = document.querySelector('{}');
            if (!el) return {{error: "Element not found"}};
            el.focus();
            el.value = '{}';
            el.dispatchEvent(new Event('input', {{bubbles: true}}));
            el.dispatchEvent(new Event('change', {{bubbles: true}}));
            return {{success: true, value: el.value}};
        }})()"#,
        selector.replace("'", "\\'"),
        text.replace("'", "\\'")
    );

    let req_body = serde_json::json!({
        "url": url,
        "script": script,
    });

    let resp = do_eval_request(req_body).await?;
    let result = &resp["result"];

    if result.get("error").is_some() {
        return Err(CarrierError::Internal(
            result["error"]
                .as_str()
                .unwrap_or("Type failed")
                .to_string(),
        ));
    }

    Ok(format!(
        "Typed '{}' into '{}'. Result: {}",
        text, selector, result
    ))
}

pub async fn scroll(input: &Value) -> CarrierResult<String> {
    let url = input["url"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'url' parameter".to_string(),
    ))?;
    let direction = input["direction"].as_str().unwrap_or("down");
    let amount = input["amount"].as_u64().unwrap_or(500);

    let delta_y = if direction == "up" {
        -(amount as i64)
    } else {
        amount as i64
    };

    let script = format!(
        "window.scrollBy(0, {}); ({{scrollY: window.scrollY, scrollHeight: document.body.scrollHeight}})",
        delta_y
    );

    let req_body = serde_json::json!({
        "url": url,
        "script": script,
    });

    let resp = do_eval_request(req_body).await?;
    let result = &resp["result"];

    Ok(format!(
        "Scrolled {} by {}px. Result: {}",
        direction, amount, result
    ))
}

pub async fn back(_input: &Value) -> CarrierResult<String> {
    Ok(
        "browser_back: AginxBrowser is stateless and does not maintain navigation history. \
Use browser_navigate with the target URL instead."
            .to_string(),
    )
}

pub async fn screenshot(_input: &Value) -> CarrierResult<String> {
    Err(CarrierError::InvalidInput(
        "Screenshots are not supported by AginxBrowser. \
AginxBrowser uses a lightweight engine without a layout/paint renderer. \
Use browser_navigate to extract page content as text/markdown instead."
            .to_string(),
    ))
}

pub async fn wait(input: &Value) -> CarrierResult<String> {
    let url = input["url"].as_str().ok_or(CarrierError::InvalidInput(
        "Missing 'url' parameter".to_string(),
    ))?;
    let selector = input["selector"].as_str();
    let timeout_ms = input["timeout_ms"].as_u64().unwrap_or(5000);

    let script = if let Some(sel) = selector {
        format!(
            r#"(async function() {{
                const start = Date.now();
                while (Date.now() - start < {}) {{
                    if (document.querySelector('{}')) return {{found: true}};
                    await new Promise(r => setTimeout(r, 200));
                }}
                return {{found: false, timeout: true}};
            }})()"#,
            timeout_ms,
            sel.replace("'", "\\'")
        )
    } else {
        format!(
            "(async function() {{ await new Promise(r => setTimeout(r, {})); return {{waited: true}}; }})()",
            timeout_ms
        )
    };

    let req_body = serde_json::json!({
        "url": url,
        "script": script,
    });

    let resp = do_eval_request(req_body).await?;
    let result = &resp["result"];

    Ok(format!("Wait result: {}", result))
}
