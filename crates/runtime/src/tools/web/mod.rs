//! web 工具实现 — aginxbrowser 本地 HTTP API 客户端（M31 D3 批1 外置成
//! aginx-web CLI；2026-09-26 CLI 退役，实现回迁母体进程内，行为同构）。
//!
//! browser_* / web_search / web_fetch 三组无状态 HTTP 工具直连
//! aginxbrowser 的 HTTP API（默认 `http://127.0.0.1:8089` —— L0 镜像烤入
//! 的两个 svc 单元之一，设备上恒在；`AGINXBROWSER_URL` 可覆盖，host 调试
//! 指远端实例）。不再 spawn 外置 CLI：少包即工具全死的形态（09-26 晨报
//! 卡死根因）随之消灭。web_bridge.rs 只留 definitions + 派发到这里。
//!
//! 安全管线同旧：taint 闸（URL 带密钥即拦）、SSRF 逐跳校验、风控站
//! （微信/知乎/JD/GitHub）不降级 reqwest。

pub mod browser;
pub mod fetch;
pub mod search;
pub mod web_cache;
pub mod web_content;

use carrier_types::error::CarrierResult;
use serde_json::Value;

pub const USER_AGENT: &str = concat!("aginx/", env!("CARGO_PKG_VERSION"));

/// Default aginxbrowser endpoint (the baked L0 unit). Override via
/// `AGINXBROWSER_URL` env var.
pub const AGINXBROWSER_DEFAULT_URL: &str = "http://127.0.0.1:8089";

/// Default timeout for aginxbrowser HTTP requests (seconds).
pub const AGINXBROWSER_TIMEOUT_SECS: u64 = 60;

/// Read the aginxbrowser URL from `AGINXBROWSER_URL` env var, falling back
/// to the default (the baked unit on the device).
pub fn aginxbrowser_url() -> String {
    carrier_types::env::get_env("AGINXBROWSER_URL")
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| AGINXBROWSER_DEFAULT_URL.to_string())
}

/// 本模块承载的全部工具名（与 web_bridge 的 definitions 一一对应）。
pub const TOOL_NAMES: &[&str] = &[
    "browser_navigate",
    "browser_read_page",
    "browser_click",
    "browser_evaluate",
    "browser_type",
    "browser_scroll",
    "browser_back",
    "browser_screenshot",
    "browser_wait",
    "browser_close",
    "web_search",
    "web_fetch",
];

/// 工具派发 — web_bridge 的执行面。`None` = 不是本模块的工具。
pub async fn execute_tool(name: &str, input: &Value) -> Option<CarrierResult<String>> {
    match name {
        "browser_navigate" | "browser_read_page" => Some(browser::navigate(input).await),
        "browser_click" => Some(browser::click(input).await),
        "browser_evaluate" => Some(browser::evaluate(input).await),
        "browser_type" => Some(browser::r#type(input).await),
        "browser_scroll" => Some(browser::scroll(input).await),
        "browser_back" => Some(browser::back(input).await),
        "browser_screenshot" => Some(browser::screenshot(input).await),
        "browser_wait" => Some(browser::wait(input).await),
        "browser_close" => Some(Ok(
            "Browser session closed (AginxBrowser is stateless).".to_string()
        )),
        "web_search" => Some(search::web_search(input).await),
        "web_fetch" => Some(fetch::web_fetch_tool(input).await),
        _ => None,
    }
}

/// URL 里的疑似密钥（工具入参携带 secret）→ 明确报错。
pub fn check_taint_net_fetch(url: &str) -> Option<String> {
    use carrier_types::taint::{TaintLabel, TaintSink, TaintedValue};
    use std::collections::HashSet;
    let exfil_patterns = [
        "api_key=",
        "apikey=",
        "token=",
        "secret=",
        "password=",
        "Authorization:",
    ];
    let lower = url.to_lowercase();
    for pattern in &exfil_patterns {
        if lower.contains(&pattern.to_lowercase()) {
            let mut labels = HashSet::new();
            labels.insert(TaintLabel::Secret);
            let tainted = TaintedValue::new(url, labels, "llm_tool_call");
            if let Err(violation) = tainted.check_sink(&TaintSink::net_fetch()) {
                tracing::warn!(
                    url = &url[..url.len().min(80)],
                    %violation,
                    "Net fetch taint check failed"
                );
                return Some(violation.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unknown_tool_is_none() {
        assert!(execute_tool("nope", &serde_json::json!({})).await.is_none());
    }

    #[tokio::test]
    async fn close_is_local_noop() {
        let r = execute_tool("browser_close", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(r.unwrap().contains("stateless"));
    }

    #[test]
    fn taint_blocks_keyed_url() {
        assert!(check_taint_net_fetch("https://x.io/p?api_key=sk-123").is_some());
        assert!(check_taint_net_fetch("https://x.io/p?token=abc").is_some());
        assert!(check_taint_net_fetch("https://x.io/p?q=1").is_none());
    }

    #[test]
    fn tool_names_are_unique() {
        let mut v = TOOL_NAMES.to_vec();
        v.sort_unstable();
        v.dedup();
        assert_eq!(v.len(), TOOL_NAMES.len());
    }
}
