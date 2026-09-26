//! web_fetch 引擎（M31 从 carrier-runtime web_fetch.rs 外置；2026-09-26
//! 回迁，行为同构）：SSRF 防护 → GET 缓存 → 风控站 aginxbrowser 兜底 →
//! reqwest 直连（逐跳 SSRF 校验的手动重定向）→ HTML→Markdown → 截断 →
//! wrap_external_content。
//!
//! 缓存回到常驻 kernel 进程内（once_cell Lazy）：回迁后跨调用复用
//! （M31 CLI 时代每次调用一个进程、缓存形同虚设的代价消失）。

use carrier_types::config::WebFetchConfig;
use carrier_types::error::{CarrierError, CarrierResult};
use carrier_types::ssrf;
use carrier_types::truncate_str;
use serde_json::Value;
use std::sync::Arc;
use tracing::{debug, warn};

use super::web_cache::WebCache;
use super::web_content::{html_to_markdown, wrap_external_content};
use super::{aginxbrowser_url, USER_AGENT};

/// 外挂 aginxbrowser 需要兜底抓取的站点（JS 渲染或风控）。命中即走
/// 浏览器，其余走 reqwest 直连。这是 web_fetch 的私有路由策略，不暴露为配置。
const AGINXBROWSER_HOSTS: &[&str] = &[
    "mp.weixin.qq.com",   // 微信公众号文章（风控）
    "zhuanlan.zhihu.com", // 知乎专栏（JS 渲染）
    "search.jd.com",      // 京东搜索（动态）
    "github.com",         // GitHub（动态 + 需代理）
];

/// 目标 URL 是否属于已知需要浏览器渲染/过风控的站点。
fn should_use_aginxbrowser(url: &str) -> bool {
    let host = ssrf::extract_host(url); // 返回 "host:port"
    AGINXBROWSER_HOSTS.iter().any(|h| host.contains(h)) // .contains 兼容带端口后缀
}

/// 引擎默认配置（types WebFetchConfig::default：50k chars / 10MB / 30s /
/// readability 开）。kernel 的 config.web.fetch 覆盖在 v1 用默认值。
fn default_engine_config() -> WebFetchConfig {
    WebFetchConfig::default()
}

/// 常驻进程内 GET 缓存（15 分钟 TTL，跨调用复用）。
static SHARED_CACHE: once_cell::sync::Lazy<Arc<WebCache>> = once_cell::sync::Lazy::new(|| {
    Arc::new(WebCache::new(std::time::Duration::from_secs(15 * 60)))
});

/// 工具入口 `web_fetch`：taint 闸 + 引擎。
pub async fn web_fetch_tool(input: &Value) -> CarrierResult<String> {
    let url = input["url"].as_str().unwrap_or("");

    // Taint check — block URLs containing API keys/tokens/secrets
    if let Some(violation) = super::check_taint_net_fetch(url) {
        return Err(CarrierError::Network(format!(
            "Taint violation: {violation}"
        )));
    }

    let method = input["method"].as_str().unwrap_or("GET");
    let headers = input.get("headers").and_then(|v| v.as_object());
    let body = input["body"].as_str();

    let engine = WebFetchEngine::new(default_engine_config(), SHARED_CACHE.clone());
    engine
        .fetch_with_options(url, method, headers, body)
        .await
}

/// Enhanced web fetch engine with SSRF protection and readability extraction.
pub struct WebFetchEngine {
    config: WebFetchConfig,
    client: reqwest::Client,
    cache: Arc<WebCache>,
}

impl WebFetchEngine {
    /// Create a new fetch engine from config with a shared cache.
    pub fn new(config: WebFetchConfig, cache: Arc<WebCache>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .gzip(true)
            .deflate(true)
            .brotli(true)
            .build()
            .unwrap_or_default();
        Self {
            config,
            client,
            cache,
        }
    }

    pub fn config(&self) -> &WebFetchConfig {
        &self.config
    }

    /// Fetch a URL with full security pipeline (GET only, for backwards compat).
    pub async fn fetch(&self, url: &str) -> CarrierResult<String> {
        self.fetch_with_options(url, "GET", None, None).await
    }

    /// Fetch a URL with configurable HTTP method, headers, and body.
    pub async fn fetch_with_options(
        &self,
        url: &str,
        method: &str,
        headers: Option<&serde_json::Map<String, serde_json::Value>>,
        body: Option<&str>,
    ) -> CarrierResult<String> {
        let method_upper = method.to_uppercase();

        // Step 1: SSRF protection — BEFORE any network I/O
        ssrf::check_ssrf(url)?;

        // Step 2: Cache lookup (only for GET)
        let cache_key = format!("fetch:{}:{}", method_upper, url);
        if method_upper == "GET" {
            if let Some(cached) = self.cache.get(&cache_key) {
                debug!(url, "Fetch cache hit");
                return Ok(cached);
            }
        }

        // Step 2b: 风控站走 aginxbrowser —— 命中即浏览器抓取（GET 专属；
        // POST/PUT 等 API 调用永远走 reqwest）。风控站（微信/知乎/JD/
        // github）失败时【不降级 reqwest】——reqwest 对这些站必失败（JS
        // 渲染/风控），降级只给 agent 外壳假数据，导致它误以为"没读全"
        // 反复重试（实测 ai-writer web_fetch 循环）。失败直接报错。
        if method_upper == "GET" && should_use_aginxbrowser(url) {
            match self.fetch_via_aginxbrowser(url).await {
                Ok(content) => {
                    let truncated = if content.len() > self.config.max_chars {
                        format!(
                            "{}... [truncated, {} total chars]",
                            truncate_str(&content, self.config.max_chars),
                            content.len()
                        )
                    } else {
                        content
                    };
                    let result = format!(
                        "HTTP 200 (via AginxBrowser)\n\n{}",
                        wrap_external_content(url, &truncated)
                    );
                    self.cache.put(cache_key.clone(), result.clone());
                    return Ok(result);
                }
                Err(e) => {
                    warn!(
                        url,
                        error = %e,
                        "AginxBrowser fetch failed for risk-controlled site; NOT falling back to reqwest (would return shell/garbage and cause retry loops)"
                    );
                    return Err(CarrierError::Network(format!(
                        "此 URL 属于需要浏览器渲染的风控站点（{}），AginxBrowser 抓取失败：{e}。\
                         reqwest 直连拿不到正文（只会返回外壳/乱码），故不降级。\
                         可能原因：临时链接(tempkey)已失效、被反爬、或 AginxBrowser 未就绪。\
                         请改用公开文章 URL，或稍后重试。",
                        ssrf::extract_host(url)
                    )));
                }
            }
        }

        // Step 3: Build request with configured method
        let mut req = match method_upper.as_str() {
            "POST" => self.client.post(url),
            "PUT" => self.client.put(url),
            "PATCH" => self.client.patch(url),
            "DELETE" => self.client.delete(url),
            _ => self.client.get(url),
        };
        req = req.header(
            "User-Agent",
            format!("Mozilla/5.0 (compatible; {USER_AGENT})"),
        );

        // Add custom headers
        if let Some(hdrs) = headers {
            for (k, v) in hdrs {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }
        }

        // Add body for non-GET methods
        if let Some(b) = body {
            // Auto-detect JSON body
            if b.trim_start().starts_with('{') || b.trim_start().starts_with('[') {
                req = req.header("Content-Type", "application/json");
            }
            req = req.body(b.to_string());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CarrierError::Network(format!("HTTP request failed: {e}")))?;

        // Step 3b: Handle redirects manually with SSRF validation on each hop
        let (final_resp, final_url) = self.follow_redirects(resp, url).await?;
        let status = final_resp.status();

        // Check response size
        if let Some(len) = final_resp.content_length() {
            if len > self.config.max_response_bytes as u64 {
                return Err(CarrierError::Network(format!(
                    "Response too large: {} bytes (max {})",
                    len, self.config.max_response_bytes
                )));
            }
        }

        let content_type = final_resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let resp_body = final_resp
            .text()
            .await
            .map_err(|e| CarrierError::Network(format!("Failed to read response body: {e}")))?;

        // Step 4: For GET requests, detect HTML and convert to Markdown.
        // For non-GET (API calls), return raw body — don't mangle JSON/XML responses.
        let processed = if method_upper == "GET"
            && self.config.readability
            && is_html(&content_type, &resp_body)
        {
            let markdown = html_to_markdown(&resp_body);
            if markdown.trim().is_empty() {
                resp_body
            } else {
                markdown
            }
        } else {
            resp_body
        };

        // Step 5: Truncate (char-boundary-safe to avoid panics on multi-byte UTF-8)
        let truncated = if processed.len() > self.config.max_chars {
            format!(
                "{}... [truncated, {} total chars]",
                truncate_str(&processed, self.config.max_chars),
                processed.len()
            )
        } else {
            processed
        };

        // Step 6: Wrap with external content markers
        let result = format!(
            "HTTP {status}\n\n{}",
            wrap_external_content(&final_url, &truncated)
        );

        // Step 7: Cache (only GET responses)
        if method_upper == "GET" {
            self.cache.put(cache_key, result.clone());
        }

        Ok(result)
    }

    /// 调 aginxbrowser 的 /fetch，返回 markdown 正文。失败返回 Err（风控站
    /// 调用方不降级 reqwest）。
    async fn fetch_via_aginxbrowser(&self, url: &str) -> CarrierResult<String> {
        let base = aginxbrowser_url();
        let body = serde_json::json!({
            "url": url,
            "format": "markdown",
            "wait_secs": 4, // 等 JS 渲染（微信/动态页必需）
        });
        let resp: serde_json::Value = self
            .client
            .post(format!("{}/fetch", base.trim_end_matches('/')))
            .json(&body)
            .send()
            .await
            .map_err(|e| CarrierError::Network(format!("AginxBrowser request failed: {e}")))?
            .json()
            .await
            .map_err(|e| CarrierError::Serialization(format!("AginxBrowser parse failed: {e}")))?;
        resp.get("content")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                CarrierError::Network("AginxBrowser response missing/empty content".into())
            })
    }

    /// Follow HTTP redirects with SSRF validation on each hop.
    /// Limits redirect chain to 5 hops to prevent redirect loops.
    async fn follow_redirects(
        &self,
        mut resp: reqwest::Response,
        original_url: &str,
    ) -> CarrierResult<(reqwest::Response, String)> {
        let mut current_url = original_url.to_string();
        let max_hops = 5;

        for _ in 0..max_hops {
            let status = resp.status().as_u16();
            if !(status == 301 || status == 302 || status == 303 || status == 307 || status == 308)
            {
                return Ok((resp, current_url));
            }

            let location = match resp.headers().get("location").and_then(|v| v.to_str().ok()) {
                Some(loc) => loc.to_string(),
                None => return Ok((resp, current_url)),
            };

            // Resolve relative URLs
            let next_url = if location.starts_with("http://") || location.starts_with("https://") {
                location
            } else if location.starts_with('/') {
                let base = ssrf::extract_host(&current_url);
                let scheme = if current_url.starts_with("https") {
                    "https"
                } else {
                    "http"
                };
                format!("{scheme}://{base}{location}")
            } else {
                format!("{current_url}/{location}")
            };

            ssrf::check_ssrf(&next_url)?;

            debug!(from = %current_url, to = %next_url, "Following redirect");

            let req = self.client.get(&next_url).header(
                "User-Agent",
                format!("Mozilla/5.0 (compatible; {USER_AGENT})"),
            );

            resp = req
                .send()
                .await
                .map_err(|e| CarrierError::Network(format!("Redirect request failed: {e}")))?;
            current_url = next_url;
        }

        Err(CarrierError::Network(
            "Too many redirects (max 5)".to_string(),
        ))
    }
}

/// Detect if content is HTML based on Content-Type header or body sniffing.
fn is_html(content_type: &str, body: &str) -> bool {
    if content_type.contains("text/html") || content_type.contains("application/xhtml") {
        return true;
    }
    let trimmed = body.trim_start();
    trimmed.starts_with("<!DOCTYPE")
        || trimmed.starts_with("<!doctype")
        || trimmed.starts_with("<html")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssrf_blocks_localhost() {
        assert!(ssrf::check_ssrf("http://localhost/admin").is_err());
        assert!(ssrf::check_ssrf("http://localhost:8080/api").is_err());
    }

    #[test]
    fn test_ssrf_blocks_private_ip() {
        use std::net::IpAddr;
        assert!(ssrf::is_private_ip(&"10.0.0.1".parse::<IpAddr>().unwrap()));
        assert!(ssrf::is_private_ip(
            &"172.16.0.1".parse::<IpAddr>().unwrap()
        ));
        assert!(ssrf::is_private_ip(
            &"192.168.1.1".parse::<IpAddr>().unwrap()
        ));
        assert!(ssrf::is_private_ip(
            &"169.254.169.254".parse::<IpAddr>().unwrap()
        ));
    }

    #[test]
    fn test_ssrf_blocks_metadata() {
        assert!(ssrf::check_ssrf("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(ssrf::check_ssrf("http://metadata.google.internal/computeMetadata/v1/").is_err());
    }

    #[test]
    fn test_ssrf_allows_public() {
        assert!(!ssrf::is_private_ip(
            &"8.8.8.8".parse::<std::net::IpAddr>().unwrap()
        ));
        assert!(!ssrf::is_private_ip(
            &"1.1.1.1".parse::<std::net::IpAddr>().unwrap()
        ));
    }

    #[test]
    fn test_ssrf_blocks_non_http() {
        assert!(ssrf::check_ssrf("file:///etc/passwd").is_err());
        assert!(ssrf::check_ssrf("ftp://internal.corp/data").is_err());
        assert!(ssrf::check_ssrf("gopher://evil.com").is_err());
    }

    #[test]
    fn test_ssrf_blocks_cloud_metadata() {
        assert!(ssrf::check_ssrf("http://100.100.100.200/latest/meta-data/").is_err());
        assert!(ssrf::check_ssrf("http://192.0.0.192/metadata/instance").is_err());
    }

    #[test]
    fn test_ssrf_blocks_zero_ip() {
        assert!(ssrf::check_ssrf("http://0.0.0.0/").is_err());
    }

    #[test]
    fn test_ssrf_blocks_ipv6_localhost() {
        assert!(ssrf::check_ssrf("http://[::1]/admin").is_err());
        assert!(ssrf::check_ssrf("http://[::1]:8080/api").is_err());
    }

    #[test]
    fn test_extract_host_ipv6() {
        let h = ssrf::extract_host("http://[::1]:8080/path");
        assert_eq!(h, "[::1]:8080");

        let h2 = ssrf::extract_host("https://[::1]/path");
        assert_eq!(h2, "[::1]:443");

        let h3 = ssrf::extract_host("http://[::1]/path");
        assert_eq!(h3, "[::1]:80");
    }

    #[test]
    fn risk_host_routing() {
        assert!(should_use_aginxbrowser("https://mp.weixin.qq.com/s/abc"));
        assert!(should_use_aginxbrowser("https://github.com:443/a/b"));
        assert!(!should_use_aginxbrowser("https://example.com/x"));
    }

    #[tokio::test]
    async fn taint_gate_fires_before_engine() {
        // 密钥 URL 在引擎前被拦（SSRF/网络根本不会发生）
        let r = web_fetch_tool(&serde_json::json!({
            "url": "https://x.io/p?api_key=sk-123"
        }))
        .await;
        assert!(r.is_err());
        let msg = format!("{}", r.unwrap_err());
        assert!(msg.contains("Taint violation"), "got: {msg}");
    }
}
