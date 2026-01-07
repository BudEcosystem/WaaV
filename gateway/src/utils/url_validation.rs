//! URL validation utilities for SSRF protection
//!
//! This module provides validation for webhook URLs to prevent Server-Side Request Forgery
//! (SSRF) attacks. It ensures URLs:
//! - Use HTTPS protocol (no HTTP)
//! - Do not resolve to private/internal IP addresses
//! - Are properly formatted

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use thiserror::Error;
use tracing::warn;
use url::Url;

/// Errors that can occur during URL validation
#[derive(Debug, Error)]
pub enum UrlValidationError {
    #[error("Invalid URL format: {0}")]
    InvalidFormat(#[from] url::ParseError),

    #[error("URL scheme must be HTTPS, got: {0}")]
    HttpsRequired(String),

    #[error("URL must have a host")]
    MissingHost,

    #[error("URL resolves to private/internal IP address: {0}")]
    PrivateIpDetected(IpAddr),

    #[error("Failed to resolve hostname: {0}")]
    DnsResolutionFailed(String),

    #[error("URL host is a raw IP address which is not allowed")]
    RawIpNotAllowed,
}

/// Checks if an IPv4 address is private/internal
///
/// Private addresses include:
/// - Loopback (127.0.0.0/8)
/// - Private (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16)
/// - Link-local (169.254.0.0/16)
/// - Broadcast (255.255.255.255)
/// - Documentation (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24)
/// - Unspecified (0.0.0.0)
/// - Shared (100.64.0.0/10 - CGNAT)
pub fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    // Loopback (127.0.0.0/8)
    if ip.is_loopback() {
        return true;
    }
    // Private (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16)
    if ip.is_private() {
        return true;
    }
    // Link-local (169.254.0.0/16)
    if ip.is_link_local() {
        return true;
    }
    // Broadcast (255.255.255.255)
    if ip.is_broadcast() {
        return true;
    }
    // Unspecified (0.0.0.0)
    if ip.is_unspecified() {
        return true;
    }
    // Documentation addresses (TEST-NET-1, TEST-NET-2, TEST-NET-3)
    // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
    if ip.is_documentation() {
        return true;
    }
    // Shared address space (CGNAT) 100.64.0.0/10
    let octets = ip.octets();
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return true;
    }
    // Reserved for benchmarking 198.18.0.0/15
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return true;
    }
    false
}

/// Checks if an IPv6 address is private/internal
///
/// Private addresses include:
/// - Loopback (::1)
/// - Unspecified (::)
/// - Link-local (fe80::/10)
/// - Unique local (fc00::/7)
/// - Documentation (2001:db8::/32)
/// - IPv4-mapped private addresses
pub fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    // Loopback (::1)
    if ip.is_loopback() {
        return true;
    }
    // Unspecified (::)
    if ip.is_unspecified() {
        return true;
    }
    // Check segments for various private ranges
    let segments = ip.segments();

    // Link-local (fe80::/10) - first 10 bits are 1111111010
    if segments[0] & 0xFFC0 == 0xFE80 {
        return true;
    }

    // Unique local address (fc00::/7) - first 7 bits are 1111110
    if segments[0] & 0xFE00 == 0xFC00 {
        return true;
    }

    // Documentation (2001:db8::/32)
    if segments[0] == 0x2001 && segments[1] == 0x0DB8 {
        return true;
    }

    // IPv4-mapped IPv6 addresses (::ffff:0:0/96)
    // Check if it maps to a private IPv4
    if let Some(ipv4) = ip.to_ipv4_mapped() {
        return is_private_ipv4(&ipv4);
    }

    false
}

/// Checks if an IP address is private/internal
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => is_private_ipv4(ipv4),
        IpAddr::V6(ipv6) => is_private_ipv6(ipv6),
    }
}

/// Validates a webhook URL for SSRF protection
///
/// This function performs the following checks:
/// 1. URL must be valid and parseable
/// 2. URL scheme must be HTTPS
/// 3. URL must have a hostname
/// 4. Hostname must not be a raw IP address
/// 5. Hostname must not resolve to any private IP addresses
///
/// # Arguments
/// * `url` - The URL string to validate
///
/// # Returns
/// * `Ok(())` if the URL is valid and safe
/// * `Err(UrlValidationError)` if validation fails
///
/// # Example
/// ```rust,ignore
/// use waav_gateway::utils::url_validation::validate_webhook_url;
///
/// // Valid external URL
/// assert!(validate_webhook_url("https://webhook.example.com/events").is_ok());
///
/// // Invalid - HTTP not allowed
/// assert!(validate_webhook_url("http://webhook.example.com/events").is_err());
///
/// // Invalid - localhost not allowed
/// assert!(validate_webhook_url("https://localhost/events").is_err());
/// ```
pub fn validate_webhook_url(url: &str) -> Result<(), UrlValidationError> {
    // Parse the URL
    let parsed = Url::parse(url)?;

    // Require HTTPS
    if parsed.scheme() != "https" {
        return Err(UrlValidationError::HttpsRequired(
            parsed.scheme().to_string(),
        ));
    }

    // Get the host
    let host = parsed.host_str().ok_or(UrlValidationError::MissingHost)?;

    // Check if host is a raw IP address (not allowed)
    // Use parsed.host() which correctly identifies IPv4 and IPv6 addresses
    // (host_str() returns bracketed IPv6 like "[::1]" which fails parse)
    match parsed.host() {
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => {
            warn!(host = %host, "Webhook URL contains raw IP address");
            return Err(UrlValidationError::RawIpNotAllowed);
        }
        Some(url::Host::Domain(_)) => {} // Domain name, continue validation
        None => return Err(UrlValidationError::MissingHost),
    }

    // Resolve the hostname and check all resolved IPs
    let port = parsed.port().unwrap_or(443);
    let socket_addrs: Vec<_> = format!("{}:{}", host, port)
        .to_socket_addrs()
        .map_err(|e| UrlValidationError::DnsResolutionFailed(format!("{}: {}", host, e)))?
        .collect();

    if socket_addrs.is_empty() {
        return Err(UrlValidationError::DnsResolutionFailed(format!(
            "No addresses found for {}",
            host
        )));
    }

    // Check all resolved addresses for private IPs
    for addr in socket_addrs {
        if is_private_ip(&addr.ip()) {
            warn!(
                host = %host,
                resolved_ip = %addr.ip(),
                "Webhook URL resolves to private IP address (SSRF protection)"
            );
            return Err(UrlValidationError::PrivateIpDetected(addr.ip()));
        }
    }

    Ok(())
}

/// Validates a webhook URL allowing localhost in development mode
///
/// This is a less strict version of `validate_webhook_url` that allows
/// localhost for development/testing purposes. Use with caution.
///
/// # Arguments
/// * `url` - The URL string to validate
/// * `allow_localhost` - Whether to allow localhost addresses
pub fn validate_webhook_url_dev(url: &str, allow_localhost: bool) -> Result<(), UrlValidationError> {
    // Parse the URL
    let parsed = Url::parse(url)?;

    // Require HTTPS (or HTTP if localhost is allowed)
    let scheme = parsed.scheme();
    if scheme != "https" && scheme != "http" {
        return Err(UrlValidationError::HttpsRequired(scheme.to_string()));
    }

    // Get the host
    let host = parsed.host_str().ok_or(UrlValidationError::MissingHost)?;

    // In development mode with localhost allowed, skip IP checks for localhost
    // Note: host_str() returns bracketed IPv6, so check for both forms
    if allow_localhost
        && (host == "localhost"
            || host == "127.0.0.1"
            || host == "[::1]"
            || matches!(parsed.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
            || matches!(parsed.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback()))
    {
        return Ok(());
    }

    // For non-localhost, require HTTPS
    if scheme != "https" {
        return Err(UrlValidationError::HttpsRequired(scheme.to_string()));
    }

    // Check if host is a raw IP address (not allowed except localhost in dev)
    // Use parsed.host() which correctly identifies IPv4 and IPv6 addresses
    match parsed.host() {
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_)) => {
            warn!(host = %host, "Webhook URL contains raw IP address");
            return Err(UrlValidationError::RawIpNotAllowed);
        }
        Some(url::Host::Domain(_)) => {} // Domain name, continue validation
        None => return Err(UrlValidationError::MissingHost),
    }

    // Resolve the hostname and check all resolved IPs
    let port = parsed.port().unwrap_or(443);
    let socket_addrs: Vec<_> = format!("{}:{}", host, port)
        .to_socket_addrs()
        .map_err(|e| UrlValidationError::DnsResolutionFailed(format!("{}: {}", host, e)))?
        .collect();

    if socket_addrs.is_empty() {
        return Err(UrlValidationError::DnsResolutionFailed(format!(
            "No addresses found for {}",
            host
        )));
    }

    // Check all resolved addresses for private IPs
    for addr in socket_addrs {
        if is_private_ip(&addr.ip()) {
            warn!(
                host = %host,
                resolved_ip = %addr.ip(),
                "Webhook URL resolves to private IP address (SSRF protection)"
            );
            return Err(UrlValidationError::PrivateIpDetected(addr.ip()));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_private_ipv4_loopback() {
        assert!(is_private_ipv4(&Ipv4Addr::new(127, 0, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(127, 255, 255, 255)));
    }

    #[test]
    fn test_is_private_ipv4_private_ranges() {
        // 10.0.0.0/8
        assert!(is_private_ipv4(&Ipv4Addr::new(10, 0, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(10, 255, 255, 255)));

        // 172.16.0.0/12
        assert!(is_private_ipv4(&Ipv4Addr::new(172, 16, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(172, 31, 255, 255)));
        assert!(!is_private_ipv4(&Ipv4Addr::new(172, 32, 0, 1)));

        // 192.168.0.0/16
        assert!(is_private_ipv4(&Ipv4Addr::new(192, 168, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(192, 168, 255, 255)));
    }

    #[test]
    fn test_is_private_ipv4_link_local() {
        assert!(is_private_ipv4(&Ipv4Addr::new(169, 254, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(169, 254, 255, 255)));
    }

    #[test]
    fn test_is_private_ipv4_special() {
        // Broadcast
        assert!(is_private_ipv4(&Ipv4Addr::new(255, 255, 255, 255)));
        // Unspecified
        assert!(is_private_ipv4(&Ipv4Addr::new(0, 0, 0, 0)));
    }

    #[test]
    fn test_is_private_ipv4_documentation() {
        // TEST-NET-1
        assert!(is_private_ipv4(&Ipv4Addr::new(192, 0, 2, 1)));
        // TEST-NET-2
        assert!(is_private_ipv4(&Ipv4Addr::new(198, 51, 100, 1)));
        // TEST-NET-3
        assert!(is_private_ipv4(&Ipv4Addr::new(203, 0, 113, 1)));
    }

    #[test]
    fn test_is_private_ipv4_cgnat() {
        // CGNAT 100.64.0.0/10
        assert!(is_private_ipv4(&Ipv4Addr::new(100, 64, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(100, 127, 255, 255)));
        assert!(!is_private_ipv4(&Ipv4Addr::new(100, 128, 0, 1)));
    }

    #[test]
    fn test_is_private_ipv4_benchmarking() {
        // 198.18.0.0/15
        assert!(is_private_ipv4(&Ipv4Addr::new(198, 18, 0, 1)));
        assert!(is_private_ipv4(&Ipv4Addr::new(198, 19, 255, 255)));
        assert!(!is_private_ipv4(&Ipv4Addr::new(198, 20, 0, 1)));
    }

    #[test]
    fn test_is_private_ipv4_public() {
        // Well-known public IPs
        assert!(!is_private_ipv4(&Ipv4Addr::new(8, 8, 8, 8))); // Google DNS
        assert!(!is_private_ipv4(&Ipv4Addr::new(1, 1, 1, 1))); // Cloudflare DNS
        assert!(!is_private_ipv4(&Ipv4Addr::new(151, 101, 1, 140))); // Some CDN
    }

    #[test]
    fn test_is_private_ipv6_loopback() {
        assert!(is_private_ipv6(&Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)));
    }

    #[test]
    fn test_is_private_ipv6_unspecified() {
        assert!(is_private_ipv6(&Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 0)));
    }

    #[test]
    fn test_is_private_ipv6_link_local() {
        assert!(is_private_ipv6(&Ipv6Addr::new(0xFE80, 0, 0, 0, 0, 0, 0, 1)));
    }

    #[test]
    fn test_is_private_ipv6_unique_local() {
        // fc00::/7
        assert!(is_private_ipv6(&Ipv6Addr::new(0xFC00, 0, 0, 0, 0, 0, 0, 1)));
        assert!(is_private_ipv6(&Ipv6Addr::new(0xFD00, 0, 0, 0, 0, 0, 0, 1)));
    }

    #[test]
    fn test_is_private_ipv6_documentation() {
        // 2001:db8::/32
        assert!(is_private_ipv6(&Ipv6Addr::new(
            0x2001, 0x0DB8, 0, 0, 0, 0, 0, 1
        )));
    }

    #[test]
    fn test_is_private_ipv6_public() {
        // Google IPv6 DNS
        assert!(!is_private_ipv6(&Ipv6Addr::new(
            0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888
        )));
    }

    #[test]
    fn test_validate_webhook_url_invalid_format() {
        assert!(matches!(
            validate_webhook_url("not-a-url"),
            Err(UrlValidationError::InvalidFormat(_))
        ));
    }

    #[test]
    fn test_validate_webhook_url_http_not_allowed() {
        assert!(matches!(
            validate_webhook_url("http://example.com/webhook"),
            Err(UrlValidationError::HttpsRequired(_))
        ));
    }

    #[test]
    fn test_validate_webhook_url_raw_ip_not_allowed() {
        assert!(matches!(
            validate_webhook_url("https://192.0.2.1/webhook"),
            Err(UrlValidationError::RawIpNotAllowed)
        ));
        assert!(matches!(
            validate_webhook_url("https://[2001:db8::1]/webhook"),
            Err(UrlValidationError::RawIpNotAllowed)
        ));
    }

    #[test]
    fn test_validate_webhook_url_localhost_not_allowed() {
        // localhost resolves to 127.0.0.1 which is private
        let result = validate_webhook_url("https://localhost/webhook");
        assert!(
            matches!(result, Err(UrlValidationError::PrivateIpDetected(_))),
            "Expected PrivateIpDetected, got {:?}",
            result
        );
    }

    #[test]
    fn test_validate_webhook_url_dev_allows_localhost() {
        // In dev mode with allow_localhost=true, localhost should be allowed
        assert!(validate_webhook_url_dev("http://localhost:8080/webhook", true).is_ok());
        assert!(validate_webhook_url_dev("https://localhost/webhook", true).is_ok());
        assert!(validate_webhook_url_dev("http://127.0.0.1:8080/webhook", true).is_ok());
    }

    #[test]
    fn test_validate_webhook_url_dev_strict() {
        // In dev mode with allow_localhost=false, same rules as production
        let result = validate_webhook_url_dev("http://localhost:8080/webhook", false);
        assert!(result.is_err());
    }
}
