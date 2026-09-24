//! Shared SSRF (Server-Side Request Forgery) protection.
//!
//! Used by runtime (web_fetch, host_functions, MCP), kernel (describe_content,
//! cron webhook), and clone (hub URL validation). Single source of truth for
//! private IP ranges, blocked hostnames, and DNS resolution checks.

use std::net::{IpAddr, ToSocketAddrs};

use crate::error::{CarrierError, CarrierResult};

/// Comma-separated hostnames that are allowed to resolve to private IPs.
/// Set via the `AGINX_CARRIER_SSRF_ALLOWLIST` environment variable.
/// Example: `github.com,api.github.com`.
fn ssrf_allowlist() -> Vec<String> {
    std::env::var("AGINX_CARRIER_SSRF_ALLOWLIST")
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Check if a URL targets a private/internal network resource.
/// Blocks localhost, cloud metadata endpoints, and private IPs.
/// Must run BEFORE any network I/O.
pub fn check_ssrf(url: &str) -> CarrierResult<()> {
    check_ssrf_with_ip(url).map(|_| ())
}

/// Check SSRF and return the resolved IP address.
/// Callers should use reqwest's `.resolve()` to pin this IP,
/// preventing DNS rebinding (TOCTOU) attacks.
pub fn check_ssrf_with_ip(url: &str) -> CarrierResult<IpAddr> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(CarrierError::Network(
            "Only http:// and https:// URLs are allowed".to_string(),
        ));
    }

    let host = extract_host(url);
    let hostname = if host.starts_with('[') {
        host.find(']').map(|i| &host[..=i]).unwrap_or(&host)
    } else {
        host.split(':').next().unwrap_or(&host)
    };

    let blocked = [
        "localhost",
        "ip6-localhost",
        "metadata.google.internal",
        "metadata.aws.internal",
        "instance-data",
        "169.254.169.254",
        "100.100.100.200",
        "192.0.0.192",
        "0.0.0.0",
        "::1",
        "[::1]",
    ];
    if blocked.contains(&hostname) {
        return Err(CarrierError::Network(format!(
            "SSRF blocked: {hostname} is a restricted hostname"
        )));
    }

    // Allowlisted hostnames bypass private-IP checks. Still must resolve.
    let allowlisted = ssrf_allowlist().contains(&hostname.to_lowercase());

    let port = if url.starts_with("https") { 443 } else { 80 };
    let socket_addr = format!("{hostname}:{port}");
    let addrs: Vec<std::net::SocketAddr> = socket_addr
        .to_socket_addrs()
        .map_err(|e| {
            CarrierError::Network(format!("SSRF blocked: cannot resolve {hostname}: {e}"))
        })?
        .collect();

    if addrs.is_empty() {
        return Err(CarrierError::Network(format!(
            "SSRF: no DNS results for {hostname}"
        )));
    }

    if !allowlisted {
        for addr in &addrs {
            let ip = addr.ip();
            if ip.is_loopback() || ip.is_unspecified() || is_private_ip(&ip) {
                return Err(CarrierError::Network(format!(
                    "SSRF blocked: {hostname} resolves to private IP {ip}"
                )));
            }
        }
    }

    // Return the first resolved IP for callers to pin via .resolve()
    Ok(addrs[0].ip())
}

/// Check if an IP address is in a private/internal range.
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            matches!(
                octets,
                [10, ..] | [172, 16..=31, ..] | [192, 168, ..] | [169, 254, ..]
            )
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            (segments[0] & 0xfe00) == 0xfc00 || (segments[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Extract host:port from a URL. Handles IPv6 bracket notation.
pub fn extract_host(url: &str) -> String {
    if let Some(after_scheme) = url.split("://").nth(1) {
        let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
        if host_port.starts_with('[') {
            if let Some(bracket_end) = host_port.find(']') {
                let ipv6_host = &host_port[..=bracket_end];
                let after_bracket = &host_port[bracket_end + 1..];
                if let Some(port) = after_bracket.strip_prefix(':') {
                    return format!("{ipv6_host}:{port}");
                }
                let default_port = if url.starts_with("https") { 443 } else { 80 };
                return format!("{ipv6_host}:{default_port}");
            }
        }
        if host_port.contains(':') {
            host_port.to_string()
        } else if url.starts_with("https") {
            format!("{host_port}:443")
        } else {
            format!("{host_port}:80")
        }
    } else {
        url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssrf_blocks_localhost() {
        assert!(check_ssrf("http://localhost/path").is_err());
        assert!(check_ssrf("https://127.0.0.1/path").is_err());
    }

    #[test]
    fn ssrf_blocks_cloud_metadata() {
        assert!(check_ssrf("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(check_ssrf("http://metadata.google.internal/").is_err());
        assert!(check_ssrf("http://100.100.100.200/").is_err());
    }

    #[test]
    fn ssrf_blocks_private_ip() {
        assert!(check_ssrf("http://10.0.0.1/").is_err());
        assert!(check_ssrf("http://172.16.0.1/").is_err());
        assert!(check_ssrf("http://192.168.1.1/").is_err());
    }

    #[test]
    fn ssrf_blocks_ipv6_localhost() {
        assert!(check_ssrf("http://[::1]:8080/").is_err());
    }

    #[test]
    fn ssrf_allows_public() {
        assert!(check_ssrf("https://api.openai.com/v1/chat").is_ok());
    }

    #[test]
    fn ssrf_rejects_non_http() {
        assert!(check_ssrf("ftp://example.com/").is_err());
        assert!(check_ssrf("file:///etc/passwd").is_err());
    }

    #[test]
    fn ssrf_rejects_unresolvable() {
        // DNS resolution failure should be rejected (no silent pass-through)
        assert!(check_ssrf("http://this-domain-does-not-exist-xyz123.invalid/").is_err());
    }

    #[test]
    fn ssrf_with_ip_returns_resolved() {
        // check_ssrf_with_ip should return a resolved IP for public domains
        let result = check_ssrf_with_ip("https://api.openai.com/v1/chat");
        assert!(result.is_ok());
        let ip = result.unwrap();
        assert!(!ip.is_loopback());
        assert!(!is_private_ip(&ip));
    }

    #[test]
    fn extract_host_ipv6() {
        assert_eq!(extract_host("https://[::1]:8080/path"), "[::1]:8080");
        assert_eq!(extract_host("https://[::1]/path"), "[::1]:443");
    }

    #[test]
    fn extract_host_regular() {
        assert_eq!(extract_host("https://example.com/path"), "example.com:443");
        assert_eq!(
            extract_host("http://example.com:8080/path"),
            "example.com:8080"
        );
    }
}
