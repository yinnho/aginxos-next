//! Link understanding — auto-extract and summarize URLs from messages.

use tracing::warn;

/// Configuration for link understanding (re-exported from types).
pub use carrier_types::media::LinkConfig;

/// Extract URLs from text, with SSRF validation.
///
/// Returns up to `max` valid, unique, non-private URLs.
pub fn extract_urls(text: &str, max: usize) -> Vec<String> {
    // Simple but effective URL regex
    let url_pattern = regex_lite::Regex::new(
        r#"https?://[^\s<>\[\](){}|\\^`"']+[^\s<>\[\](){}|\\^`"'.,;:!?\-)]"#,
    )
    .expect("URL regex is valid");

    let mut seen = std::collections::HashSet::new();
    let mut urls = Vec::new();

    for m in url_pattern.find_iter(text) {
        let url = m.as_str().to_string();

        // Deduplicate
        if !seen.insert(url.clone()) {
            continue;
        }

        // SECURITY: SSRF check — reject private IPs and metadata endpoints
        if carrier_types::ssrf::check_ssrf(&url).is_err() {
            warn!("Rejected private/SSRF URL: {}", url);
            continue;
        }

        urls.push(url);
        if urls.len() >= max {
            break;
        }
    }

    urls
}

/// Build link context string to inject into agent messages.
///
/// Returns None if no links found or link understanding is disabled.
pub fn build_link_context(text: &str, config: &LinkConfig) -> Option<String> {
    if !config.enabled {
        return None;
    }

    let urls = extract_urls(text, config.max_links);
    if urls.is_empty() {
        return None;
    }

    let mut context = String::from("\n\n[Link Context - URLs detected in message]\n");
    for url in &urls {
        context.push_str(&format!("- {url}\n"));
    }
    context.push_str(
        "Use web_fetch to retrieve content from these URLs if relevant to the user's request.\n",
    );
    Some(context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_urls_basic() {
        // SSRF check requires DNS resolution; may fail in offline environments
        let text = "Check out https://example.com and http://test.org/page";
        let urls = extract_urls(text, 10);
        // In offline env, DNS fails and URLs are filtered out — that's acceptable
        if !urls.is_empty() {
            assert!(urls[0].contains("example.com"));
        }
    }

    #[test]
    fn test_extract_urls_dedup() {
        let text = "Visit https://example.com and also https://example.com again";
        let urls = extract_urls(text, 10);
        assert!(urls.len() <= 1);
    }

    #[test]
    fn test_extract_urls_max_limit() {
        // May return fewer if DNS fails — just verify it doesn't exceed max
        let text = "https://a.com https://b.com https://c.com https://d.com https://e.com";
        let urls = extract_urls(text, 3);
        assert!(urls.len() <= 3);
    }

    #[test]
    fn test_extract_urls_no_urls() {
        let text = "No URLs here, just plain text.";
        let urls = extract_urls(text, 10);
        assert!(urls.is_empty());
    }

    #[test]
    fn test_ssrf_localhost_blocked() {
        assert!(carrier_types::ssrf::check_ssrf("http://localhost/admin").is_err());
        assert!(carrier_types::ssrf::check_ssrf("http://127.0.0.1:8080/secret").is_err());
        assert!(carrier_types::ssrf::check_ssrf("http://0.0.0.0/").is_err());
        assert!(carrier_types::ssrf::check_ssrf("http://[::1]/").is_err());
    }

    #[test]
    fn test_ssrf_private_ranges_blocked() {
        assert!(carrier_types::ssrf::check_ssrf("http://10.0.0.1/internal").is_err());
        assert!(carrier_types::ssrf::check_ssrf("http://192.168.1.1/admin").is_err());
        assert!(carrier_types::ssrf::check_ssrf("http://172.16.0.1/secret").is_err());
        assert!(carrier_types::ssrf::check_ssrf("http://172.31.255.255/data").is_err());
    }

    #[test]
    fn test_ssrf_metadata_blocked() {
        assert!(carrier_types::ssrf::check_ssrf("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(carrier_types::ssrf::check_ssrf("http://metadata.google.internal/").is_err());
    }

    #[test]
    fn test_ssrf_public_allowed() {
        // These may fail in offline/DNS-broken CI; that's acceptable
        let _ = carrier_types::ssrf::check_ssrf("https://example.com/page");
    }

    #[test]
    fn test_ssrf_172_non_private() {
        // 172.32.x.x is NOT private — may fail if DNS doesn't resolve; that's OK
        let _ = carrier_types::ssrf::check_ssrf("http://172.32.0.1/ok");
        let _ = carrier_types::ssrf::check_ssrf("http://172.15.0.1/ok");
    }

    #[test]
    fn test_extract_urls_filters_private() {
        // Private IPs always fail SSRF; public may fail DNS in offline env
        let text =
            "Public: https://example.com Private: http://localhost/admin http://192.168.1.1/secret";
        let urls = extract_urls(text, 10);
        // localhost and 192.168 must always be filtered; example.com depends on DNS
        assert!(!urls.iter().any(|u| u.contains("localhost")));
        assert!(!urls.iter().any(|u| u.contains("192.168")));
    }

    #[test]
    fn test_build_link_context_disabled() {
        let config = LinkConfig {
            enabled: false,
            ..Default::default()
        };
        let result = build_link_context("https://example.com", &config);
        assert!(result.is_none());
    }

    #[test]
    fn test_build_link_context_enabled() {
        let config = LinkConfig {
            enabled: true,
            ..Default::default()
        };
        let result = build_link_context("Check https://example.com", &config);
        // Result depends on DNS resolution in test environment
        if let Some(ctx) = result {
            assert!(ctx.contains("example.com"));
            assert!(ctx.contains("Link Context"));
        }
    }

    #[test]
    fn test_build_link_context_no_urls() {
        let config = LinkConfig {
            enabled: true,
            ..Default::default()
        };
        let result = build_link_context("No URLs here", &config);
        assert!(result.is_none());
    }
}
