//! Canonical network-target validation (SSRF protection).
//!
//! Single source of truth for validating client-supplied URLs before the
//! gateway dials them. Three call sites used to carry rule-for-rule copies of
//! this logic and are now thin wrappers over [`validate_url_for_ssrf`]:
//!
//! - `dag::nodes::endpoint::validate_url_for_ssrf` (HTTP/WS/gRPC endpoint
//!   nodes, webhook output, DAG LLM `base_url`) — `DAGError::ConfigError`.
//! - `core::tts::websocket::validate_ws_endpoint_for_ssrf` (streaming-TTS
//!   `endpoint_override`) — `TTSError::InvalidConfiguration`.
//! - `core::conversation::validate_llm_url` (conversation LLM `base_url`) —
//!   `Result<(), String>`.
//!
//! The rule set is the UNION of the protections the three copies had, plus two
//! audit-gap closures (decimal IPv4 literals, IPv6 multicast):
//!
//! 1. Scheme allowlist (caller-supplied, e.g. `["http", "https"]`).
//! 2. Blocked hostnames: localhost forms + cloud-metadata endpoints
//!    (`169.254.169.254`, `metadata.google.internal`, `metadata.azure.com`, …).
//! 3. Private/special IPv4 literals: loopback, RFC1918, link-local (cloud
//!    metadata), CGNAT `100.64/10`, documentation TEST-NETs, benchmarking
//!    `198.18/15`, `0.0.0.0/8`, broadcast — including DECIMAL/integer literals
//!    (`http://3232235777` == `192.168.1.1`).
//! 4. Private/special IPv6 literals (bracketed): loopback, unspecified,
//!    link-local `fe80::/10`, unique-local `fc00::/7`, documentation
//!    `2001:db8::/32`, multicast `ff00::/8`, and IPv4-MAPPED addresses checked
//!    against the IPv4 rules.
//! 5. Resolve-then-validate for DNS names (kills DNS-rebind / TOCTOU): every
//!    address the name currently resolves to must pass the IP rules. Hosts
//!    that do not resolve are allowed through (they may resolve later; the
//!    transport applies its own checks at dial time).
//!
//! ## Test escape hatch
//!
//! `WAAV_ALLOW_LOOPBACK_ENDPOINTS=1` (opt-in, OFF by default) skips the host
//! checks — the scheme allowlist still applies — so in-process mock servers
//! (DAG data-plane e2e, conversation-loop tests, `deepgram_aura_ws_e2e`) can
//! target `127.0.0.1`. Never set it in production.
//!
//! This module is feature-free: it must be available to non-DAG builds.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Hostnames rejected outright (case-insensitive), before any IP parsing or
/// DNS resolution. Union of the lists the three former copies carried, plus
/// `metadata.azure.com`.
const BLOCKED_HOSTNAMES: &[&str] = &[
    "localhost",
    "localhost.localdomain",
    "127.0.0.1",
    "::1",
    "0.0.0.0",
    "[::1]",
    "[::ffff:127.0.0.1]",
    // Cloud metadata endpoints (AWS/Azure/GCP share the link-local IP).
    "169.254.169.254",
    "metadata.google.internal",
    "metadata.gcp.internal",
    "metadata.azure.com",
    // Common internal hostnames.
    "internal",
    "intranet",
];

/// Whether loopback/private endpoint targets are explicitly permitted.
///
/// OFF by default. Opt-in via `WAAV_ALLOW_LOOPBACK_ENDPOINTS=1` so in-process
/// mock servers can target `127.0.0.1` without weakening production SSRF
/// protection. Never set in production.
pub(crate) fn loopback_endpoints_allowed() -> bool {
    loopback_flag_enabled(
        std::env::var("WAAV_ALLOW_LOOPBACK_ENDPOINTS")
            .ok()
            .as_deref(),
    )
}

/// Pure parser for the escape-hatch flag value (unit-testable without touching
/// process-global env state; see the note on [`tests`]).
fn loopback_flag_enabled(value: Option<&str>) -> bool {
    matches!(value, Some("1") | Some("true") | Some("TRUE"))
}

/// Validate a URL for SSRF (Server-Side Request Forgery) protection.
///
/// `allowed_schemes` must be lowercase (the URL's scheme is lowercased before
/// comparison). Returns `Ok(())` for safe URLs, or a human-readable reason why
/// the URL was rejected. See the module docs for the full rule set.
pub fn validate_url_for_ssrf(url: &str, allowed_schemes: &[&str]) -> Result<(), String> {
    validate_url_for_ssrf_inner(url, allowed_schemes, loopback_endpoints_allowed())
}

/// Rule engine behind [`validate_url_for_ssrf`], with the loopback escape
/// hatch injected so the gate can be unit-tested deterministically. The env
/// var is read by callers/tests in several modules WITHOUT synchronization, so
/// the unit tests for the gate must not set it (see [`tests`]).
fn validate_url_for_ssrf_inner(
    url: &str,
    allowed_schemes: &[&str],
    loopback_allowed: bool,
) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL '{}': {}", url, e))?;

    // Scheme allowlist — applies even when the loopback escape hatch is on.
    let scheme = parsed.scheme().to_lowercase();
    if !allowed_schemes.contains(&scheme.as_str()) {
        return Err(format!(
            "URL scheme '{}' not allowed. Use {}",
            scheme,
            allowed_schemes.join(", ")
        ));
    }

    // Test/local-mock escape hatch (opt-in, OFF by default).
    if loopback_allowed {
        return Ok(());
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| format!("URL '{}' has no host", url))?;

    // Blocked hostnames (case-insensitive).
    let host_lower = host.to_lowercase();
    if BLOCKED_HOSTNAMES.contains(&host_lower.as_str()) {
        return Err(format!(
            "URL host '{}' is blocked (SSRF protection)",
            host
        ));
    }

    // Plain IP literal (the `url` crate normalizes IPv4 forms for http/ws
    // schemes, so decimal/octal/hex literals usually surface here already).
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_private_ip(&ip) {
            return Err(format!(
                "URL points to private IP '{}' (SSRF protection)",
                ip
            ));
        }
        return Ok(());
    }

    // Bracketed IPv6 literal.
    if host.starts_with('[') && host.ends_with(']') {
        if let Ok(ip) = host[1..host.len() - 1].parse::<Ipv6Addr>()
            && is_private_ipv6(&ip)
        {
            return Err(format!(
                "URL points to private IPv6 '{}' (SSRF protection)",
                ip
            ));
        }
        // An IP literal — never DNS-resolved.
        return Ok(());
    }

    // DECIMAL/integer IPv4 literal (e.g. `http://3232235777` == 192.168.1.1).
    // Defense-in-depth for url-crate versions / entry points that do not
    // normalize this form; an all-digits host is an address, not a DNS name.
    if !host.is_empty()
        && host.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n) = host.parse::<u32>()
    {
        let ip = Ipv4Addr::from(n);
        if is_private_ipv4(&ip) {
            return Err(format!(
                "URL host '{}' is a decimal IPv4 literal for private IP '{}' (SSRF protection)",
                host, ip
            ));
        }
        return Ok(());
    }

    // Resolve-then-validate: when the host is a DNS name (not an IP literal),
    // resolve it and reject if ANY resolved address is private/internal. This
    // closes DNS-rebinding / TOCTOU holes where a public-looking hostname
    // resolves to a private/metadata address.
    validate_resolved_host_for_ssrf(host)
}

/// Resolve a DNS hostname and reject if any resolved IP is private/internal.
///
/// Uses a synchronous resolver (`ToSocketAddrs`) so it can run inside the
/// (synchronous) DAG compile path. A host that fails to resolve is allowed
/// through (it may resolve later; at request time the transport applies its
/// own checks); the goal here is to reject hosts that *currently* resolve to a
/// blocked range, which is the DNS-rebind vector for client-supplied targets.
pub fn validate_resolved_host_for_ssrf(host: &str) -> Result<(), String> {
    use std::net::ToSocketAddrs;

    // Port is irrelevant for the IP check; use a dummy port for resolution.
    let resolved = match (host, 0u16).to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(_) => return Ok(()), // unresolvable now; nothing to block
    };

    for addr in resolved {
        let ip = addr.ip();
        if is_private_ip(&ip) {
            return Err(format!(
                "Host '{}' resolves to private/internal IP '{}' (SSRF protection)",
                host, ip
            ));
        }
    }

    Ok(())
}

/// Check if an IP address is private/internal.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

/// Check if an IPv4 address is private/internal.
///
/// Blocked ranges (union of the former copies):
/// - Loopback `127.0.0.0/8`
/// - Link-local `169.254.0.0/16` (includes cloud metadata)
/// - Private RFC1918 (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`)
/// - Current network `0.0.0.0/8` (includes unspecified)
/// - Broadcast `255.255.255.255`
/// - Shared/CGNAT `100.64.0.0/10`
/// - Documentation TEST-NETs (`192.0.2.0/24`, `198.51.100.0/24`,
///   `203.0.113.0/24`)
/// - Benchmarking `198.18.0.0/15`
fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    if ip.is_loopback() || ip.is_link_local() || ip.is_private() || ip.is_broadcast() {
        return true;
    }

    let octets = ip.octets();

    // 0.0.0.0/8 (current network, includes the unspecified address)
    if octets[0] == 0 {
        return true;
    }

    // Shared address space (CGNAT) 100.64.0.0/10
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return true;
    }

    // Documentation (TEST-NET-1/2/3)
    if ip.is_documentation() {
        return true;
    }

    // Reserved for benchmarking 198.18.0.0/15
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) {
        return true;
    }

    false
}

/// Check if an IPv6 address is private/internal.
///
/// Blocked (union of the former copies, plus multicast):
/// - Loopback `::1`, unspecified `::`
/// - IPv4-MAPPED addresses (verdict delegated to the IPv4 rules)
/// - Link-local `fe80::/10`, unique-local `fc00::/7`
/// - Documentation `2001:db8::/32`
/// - Multicast `ff00::/8` (audit gap: includes all-nodes/all-routers groups)
fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }

    // IPv4-mapped addresses (::ffff:0:0/96): check the embedded IPv4.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_ipv4(&v4);
    }

    let segments = ip.segments();

    // Link-local fe80::/10
    if segments[0] & 0xFFC0 == 0xFE80 {
        return true;
    }

    // Unique local fc00::/7
    if segments[0] & 0xFE00 == 0xFC00 {
        return true;
    }

    // Documentation 2001:db8::/32
    if segments[0] == 0x2001 && segments[1] == 0x0DB8 {
        return true;
    }

    // Multicast ff00::/8
    if segments[0] & 0xFF00 == 0xFF00 {
        return true;
    }

    false
}

/// Process-global lock for tests that touch `WAAV_ALLOW_LOOPBACK_ENDPOINTS`.
///
/// Every lib-binary test that reads-after-mutating the env var must hold this
/// lock (core::net, core::tts::websocket, core::conversation tests do).
#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HTTP_SCHEMES: &[&str] = &["http", "https"];
    const WS_SCHEMES: &[&str] = &["http", "https", "ws", "wss"];

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = test_env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };
        guard
    }

    #[test]
    fn scheme_allowlist_enforced() {
        let _guard = env_guard();
        assert!(validate_url_for_ssrf("ftp://example.com/x", HTTP_SCHEMES).is_err());
        assert!(validate_url_for_ssrf("ftp://example.com/x", WS_SCHEMES).is_err());
        assert!(validate_url_for_ssrf("ws://8.8.8.8/x", HTTP_SCHEMES).is_err());
        assert!(validate_url_for_ssrf("ws://8.8.8.8/x", WS_SCHEMES).is_ok());
        assert!(validate_url_for_ssrf("wss://8.8.4.4/v1/speak", WS_SCHEMES).is_ok());
        // Garbage is rejected as unparseable.
        assert!(validate_url_for_ssrf("not a url", HTTP_SCHEMES).is_err());
    }

    #[test]
    fn blocked_hostnames_rejected() {
        let _guard = env_guard();
        for url in [
            "http://localhost/v1",
            "https://LOCALHOST/v1", // case-insensitive
            "http://localhost.localdomain/x",
            "http://169.254.169.254/latest/meta-data",
            "http://metadata.google.internal/computeMetadata/v1/",
            "http://metadata.gcp.internal/x",
            "http://metadata.azure.com/metadata/instance",
            "http://[::1]:9/x",
            "http://0.0.0.0/x",
            "http://internal/x",
            "http://intranet/x",
        ] {
            assert!(
                validate_url_for_ssrf(url, HTTP_SCHEMES).is_err(),
                "{url} must be rejected"
            );
        }
    }

    #[test]
    fn private_ipv4_literals_rejected() {
        let _guard = env_guard();
        for host in [
            "127.0.0.1",       // loopback
            "127.8.9.10",      // loopback /8
            "10.0.0.1",        // RFC1918
            "172.16.0.1",      // RFC1918
            "172.31.255.254",  // RFC1918 upper edge
            "192.168.1.5",     // RFC1918
            "169.254.10.10",   // link-local
            "0.1.2.3",         // 0.0.0.0/8
            "100.64.0.1",      // CGNAT lower edge
            "100.127.255.254", // CGNAT upper edge
            "192.0.2.1",       // TEST-NET-1
            "198.51.100.7",    // TEST-NET-2
            "203.0.113.9",     // TEST-NET-3
            "198.18.0.1",      // benchmarking
            "198.19.255.254",  // benchmarking
            "255.255.255.255", // broadcast
        ] {
            let url = format!("http://{host}/x");
            assert!(
                validate_url_for_ssrf(&url, HTTP_SCHEMES).is_err(),
                "{url} must be rejected"
            );
        }
        // Public literals (incl. just-outside-range neighbors) pass.
        for host in [
            "8.8.8.8",
            "1.1.1.1",
            "100.63.255.255", // below CGNAT
            "100.128.0.1",    // above CGNAT
            "172.15.0.1",     // below 172.16/12
            "172.32.0.1",     // above 172.16/12
            "198.17.255.255", // below benchmarking
            "198.20.0.1",     // above benchmarking
        ] {
            let url = format!("http://{host}/x");
            assert!(
                validate_url_for_ssrf(&url, HTTP_SCHEMES).is_ok(),
                "{url} must be allowed"
            );
        }
    }

    /// Audit gap: integer/decimal IPv4 literals must be treated as addresses.
    #[test]
    fn decimal_ipv4_literals_validated() {
        let _guard = env_guard();
        // 2130706433 == 127.0.0.1, 3232235777 == 192.168.1.1, 167772161 == 10.0.0.1
        for url in [
            "http://2130706433/x",
            "http://3232235777/x",
            "http://167772161/x",
        ] {
            assert!(
                validate_url_for_ssrf(url, HTTP_SCHEMES).is_err(),
                "{url} must be rejected"
            );
        }
        // 134744072 == 8.8.8.8 (public) passes.
        assert!(validate_url_for_ssrf("http://134744072/x", HTTP_SCHEMES).is_ok());
        // The raw-host check catches the same form on the resolve entry point.
        assert!(is_private_ipv4(&Ipv4Addr::from(2130706433u32)));
    }

    #[test]
    fn private_ipv6_literals_rejected() {
        let _guard = env_guard();
        for host in [
            "[::1]",                // loopback (also hostname-blocked)
            "[fe80::1]",            // link-local
            "[febf::1]",            // link-local upper edge
            "[fc00::1]",            // unique-local
            "[fdff::1]",            // unique-local upper edge
            "[2001:db8::1]",        // documentation
            "[::ffff:127.0.0.1]",   // IPv4-mapped loopback
            "[::ffff:10.0.0.1]",    // IPv4-mapped RFC1918
            "[::ffff:a9fe:a9fe]",   // IPv4-mapped 169.254.169.254
            "[ff02::1]",            // multicast all-nodes (audit gap)
            "[ff05::2]",            // multicast site-local scope
        ] {
            let url = format!("http://{host}/x");
            assert!(
                validate_url_for_ssrf(&url, HTTP_SCHEMES).is_err(),
                "{url} must be rejected"
            );
        }
        // Public IPv6 literals pass, including mapped-public.
        for host in ["[2607:f8b0:4004:c07::67]", "[::ffff:8.8.8.8]"] {
            let url = format!("https://{host}/x");
            assert!(
                validate_url_for_ssrf(&url, HTTP_SCHEMES).is_ok(),
                "{url} must be allowed"
            );
        }
    }

    /// Resolve-then-validate: a DNS name currently resolving to a blocked
    /// range is rejected; an unresolvable name is allowed through.
    #[test]
    fn resolve_then_validate_dns_names() {
        let _guard = env_guard();
        // `localhost` resolves via /etc/hosts (no network needed) to 127.0.0.1
        // and/or ::1 — both private.
        assert!(validate_resolved_host_for_ssrf("localhost").is_err());
        // Unresolvable (.invalid is reserved): nothing to block now.
        assert!(validate_resolved_host_for_ssrf("unresolvable-host.invalid").is_ok());
        assert!(
            validate_url_for_ssrf("https://unresolvable-host.invalid/path", HTTP_SCHEMES).is_ok()
        );
    }

    /// Loopback gate OFF (default): private targets rejected via the public,
    /// env-reading entry point (env var removed under the shared lock).
    #[test]
    fn loopback_gate_off_by_default() {
        let _guard = env_guard();
        assert!(validate_url_for_ssrf("http://127.0.0.1:8080/v1", HTTP_SCHEMES).is_err());
        assert!(validate_url_for_ssrf("ws://127.0.0.1:9000/v1/speak", WS_SCHEMES).is_err());
    }

    /// Loopback gate ON: host checks are skipped, the scheme allowlist is not.
    ///
    /// Exercised through the injected flag rather than by SETTING the env var:
    /// other modules' tests (dag compiler/webhook SSRF-rejection tests) read
    /// the var unsynchronized in this same test binary, so a setter here would
    /// race them (this is also why `core::tts::websocket` never set it). The
    /// real env path is covered end-to-end by `deepgram_aura_ws_e2e` /
    /// `conversation_loop` / `dag_dataplane` in their own processes.
    #[test]
    fn loopback_gate_on_permits_private_targets() {
        for url in [
            "http://127.0.0.1:8080/v1",
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/x",
            "http://localhost/x",
        ] {
            assert!(
                validate_url_for_ssrf_inner(url, HTTP_SCHEMES, true).is_ok(),
                "{url} must be allowed with the gate on"
            );
        }
        // Scheme allowlist still applies with the gate on.
        assert!(validate_url_for_ssrf_inner("ftp://example.com/x", HTTP_SCHEMES, true).is_err());
        assert!(validate_url_for_ssrf_inner("ws://127.0.0.1/x", HTTP_SCHEMES, true).is_err());
    }

    /// The env-flag parser itself (pure; no env mutation needed).
    #[test]
    fn loopback_flag_parsing() {
        for on in [Some("1"), Some("true"), Some("TRUE")] {
            assert!(loopback_flag_enabled(on), "{on:?} must enable the gate");
        }
        for off in [None, Some("0"), Some("false"), Some(""), Some("yes")] {
            assert!(!loopback_flag_enabled(off), "{off:?} must keep the gate off");
        }
    }

    #[test]
    fn url_without_host_rejected() {
        let _guard = env_guard();
        assert!(validate_url_for_ssrf("unix:/tmp/x.sock", &["unix"]).is_err());
    }
}
