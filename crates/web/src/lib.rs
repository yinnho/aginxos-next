//! aginx-web — AginxBrowser 客户端 CLI 的库面（原 agb，M31 D3 批1；D13 改姓 2026-09-09）。
//!
//! browser_* / web_search / web_fetch 三组无状态 HTTP 工具的实现从
//! carrier-runtime 整体搬来（行为逐字节同构：同样的 HTTP 体、同样的
//! 输出格式、同样的安全管线 SSRF/taint/风控路由）。runtime 侧只留
//! `web_bridge`：同名 ToolDefinition + spawn `aginx-web tool <name>`。
//!
//! 两张脸：
//! - 人/流程脚本：`aginx-web navigate <url>`、`aginx-web search <q>`、`aginx-web fetch <url>` …
//! - 机读（runtime 桥用）：`aginx-web tool <name>`，stdin 收工具入参 JSON，
//!   stdout 出 D1 信封（`{"ok":true,"data":"…"}` / `{"ok":false,"error":…}`）。

pub mod browser;
pub mod fetch;
pub mod search;
pub mod web_cache;
pub mod web_content;

use carrier_types::error::{CarrierError, CarrierResult};
use serde_json::Value;

pub const USER_AGENT: &str = concat!("aginx-web/", env!("CARGO_PKG_VERSION"));

/// Default AginBrowser endpoint. Override via `AGINXBROWSER_URL` env var.
pub const AGINXBROWSER_DEFAULT_URL: &str = "http://127.0.0.1:8089";

/// Default timeout for AginBrowser HTTP requests (seconds).
pub const AGINXBROWSER_TIMEOUT_SECS: u64 = 60;

/// Read the AginBrowser URL from `AGINXBROWSER_URL` env var.
/// Returns `None` if not set or empty (e.g. web_search disables itself).
pub fn aginxbrowser_url_opt() -> Option<String> {
    // carrier_types::env::get_env so ~/.aginx/carrier/.env values take effect
    // (load_dotenv populates ENV_OVERRIDES, not std::env). Falls back to
    // std::env::var for systemd Environment= configs. CLI main 启动时先
    // load_dotenv()，桥接的子进程同样能读到。
    carrier_types::env::get_env("AGINXBROWSER_URL").filter(|s| !s.is_empty())
}

/// Read the AginBrowser URL from `AGINXBROWSER_URL` env var.
/// Returns the default URL if not set (e.g. browser_* tools are always enabled).
pub fn aginxbrowser_url() -> String {
    aginxbrowser_url_opt().unwrap_or_else(|| AGINXBROWSER_DEFAULT_URL.to_string())
}

/// 本 CLI 承载的全部工具名（与 runtime 桥的 definitions 一一对应）。
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

/// 工具派发 — `aginx-web tool <name>` 的库面。`None` = 不是本 CLI 的工具。
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

/// URL 里的疑似密钥（工具入参携带 secret）→ 明确报错。搬自 runtime
/// tools/web_fetch.rs 的 check_taint_net_fetch（行为同构）。
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

/// 人读/机读两脸共用的错误出口：Err → stderr 一行 + rc 1。
pub fn bail_human(e: &CarrierError) -> ! {
    eprintln!("aginx-web: {e}");
    std::process::exit(1);
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
