//! The one test of whether an address is on the public internet, used wherever WaaV refuses to
//! reach a private one, and the host rules built on it: segmented sessions' upload pools and the
//! gateway's SSRF checks.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Hostnames refused outright (case-insensitive), before any IP parsing or DNS resolution:
/// localhost forms, cloud-metadata endpoints and common internal names.
pub const BLOCKED_HOSTNAMES: &[&str] = &[
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

/// The private targets an operator lets a CLIENT-named address reach: the gateway's
/// `WAAV_ALLOW_LOOPBACK_ENDPOINTS` (loopback only, on any or on listed ports) and
/// `WAAV_PRIVATE_EGRESS_ALLOWLIST` (exact `host:port` pairs). The default reaches nothing private.
///
/// Data, not a flag: a boolean "may reach private" let a client-named base reach every private
/// range — cloud metadata and RFC 1918 included — whenever the loopback escape hatch was on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivateReach {
    /// Loopback: `None` blocked, `Some(empty)` any port, `Some(ports)` those ports only.
    pub loopback_ports: Option<Vec<u16>>,
    /// Exact `(host, port)` pairs exempt from the private-address rules; hosts lower case, as
    /// `url::Url::host_str` writes them (IPv6 bracketed).
    pub allowed: Vec<(String, u16)>,
}

impl PrivateReach {
    /// Whether the loopback escape hatch admits `port`.
    pub fn loopback_permits(&self, port: Option<u16>) -> bool {
        match &self.loopback_ports {
            None => false,
            Some(ports) if ports.is_empty() => true,
            Some(ports) => port.is_some_and(|p| ports.contains(&p)),
        }
    }

    /// Whether `host:port` is one of the operator's exact exemptions.
    pub fn allows_exactly(&self, host: &str, port: Option<u16>) -> bool {
        port.is_some_and(|port| {
            self.allowed
                .iter()
                .any(|(h, p)| *p == port && h.eq_ignore_ascii_case(host))
        })
    }

    /// Whether a URL host (as `url::Url::host_str` writes it) on `port` may be a private target:
    /// an exact exemption, or a loopback name or literal under the escape hatch.
    pub fn permits_host(&self, host: &str, port: Option<u16>) -> bool {
        self.allows_exactly(host, port) || (self.loopback_permits(port) && is_loopback_host(host))
    }

    /// Whether a resolved address of `name` may be dialled although it is not public: `name` is an
    /// exempt host (its port was enforced when the URL was checked), or the address is loopback
    /// and the escape hatch is on.
    pub fn permits_resolved(&self, name: &str, ip: &IpAddr) -> bool {
        self.allowed
            .iter()
            .any(|(h, _)| h.eq_ignore_ascii_case(name))
            || (self.loopback_ports.is_some() && is_loopback_ip(ip))
    }
}

fn is_loopback_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

/// Whether a URL host names the loopback interface: `localhost` forms or a loopback IP literal
/// (bracketed IPv6 included). Never link-local, metadata or another private range.
pub fn is_loopback_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    if matches!(lower.as_str(), "localhost" | "localhost.localdomain") {
        return true;
    }
    let bare = lower
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&lower);
    bare.parse::<IpAddr>().is_ok_and(|ip| is_loopback_ip(&ip))
}

/// Whether `url` names a target on this host or a private network by its spelling: a loopback or
/// blocked name, or a non-public IP literal ([`check_host`] refuses it). A warm-up connection
/// saves nothing there, and its unauthenticated request lands in the server's error log. A DNS
/// name is not resolved: it counts as remote.
pub fn is_local_origin(url: &str) -> bool {
    url::Url::parse(url.trim())
        .ok()
        .and_then(|u| u.host_str().map(|h| check_host(h).is_err()))
        .unwrap_or(false)
}

/// The half of the SSRF rule that needs no lookup, for a URL's host as `url::Url::host_str`
/// writes it: a blocked name, a `.localhost` name, or an IP literal in any spelling (bracketed
/// IPv6, decimal IPv4) that is not public is refused. `Ok(Some(name))` is a DNS name, to be
/// judged again by what it resolves to; `Ok(None)` a public IP literal.
pub fn check_host(host: &str) -> Result<Option<String>, String> {
    let host_lower = host.to_lowercase();
    if BLOCKED_HOSTNAMES.contains(&host_lower.as_str()) || host_lower.ends_with(".localhost") {
        return Err(format!("URL host '{}' is blocked (SSRF protection)", host));
    }

    // Plain IP literal (the `url` crate normalizes IPv4 forms for http/ws schemes, so
    // decimal/octal/hex literals usually surface here already).
    if let Ok(ip) = host.parse::<IpAddr>() {
        if !is_public_ip(&ip) {
            return Err(format!(
                "URL points to private IP '{}' (SSRF protection)",
                ip
            ));
        }
        return Ok(None);
    }

    // Bracketed IPv6 literal: never DNS-resolved.
    if host.starts_with('[') && host.ends_with(']') {
        if let Ok(ip) = host[1..host.len() - 1].parse::<Ipv6Addr>()
            && !is_public_ip(&IpAddr::V6(ip))
        {
            return Err(format!(
                "URL points to private IPv6 '{}' (SSRF protection)",
                ip
            ));
        }
        return Ok(None);
    }

    // DECIMAL/integer IPv4 literal (e.g. `http://3232235777` == 192.168.1.1), for entry points
    // that do not normalize this form: an all-digits host is an address, not a DNS name.
    if !host.is_empty()
        && host.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n) = host.parse::<u32>()
    {
        let ip = Ipv4Addr::from(n);
        if !is_public_ip(&IpAddr::V4(ip)) {
            return Err(format!(
                "URL host '{}' is a decimal IPv4 literal for private IP '{}' (SSRF protection)",
                host, ip
            ));
        }
        return Ok(None);
    }

    Ok(Some(host.to_string()))
}

/// Whether an address is on the public internet: not loopback, private, link-local (cloud
/// metadata), carrier-grade NAT, unspecified, broadcast, documentation, benchmarking, reserved or
/// multicast; an IPv4-mapped IPv6 address is judged as its IPv4 address.
pub fn is_public_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..128).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(&IpAddr::V4(v4));
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00
                || (seg[0] & 0xffc0) == 0xfe80
                || seg[0] == 0x2001 && seg[1] == 0x0db8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_origins_are_recognised_by_their_spelling() {
        for local in [
            "http://127.0.0.1:8080/v1",
            "http://localhost:9000",
            "http://[::1]:7/",
            "http://10.0.0.5:8000",
            "http://192.168.1.10/",
        ] {
            assert!(is_local_origin(local), "{local}");
        }
        for remote in [
            "https://api.openai.com/v1/audio/speech",
            "https://8.8.8.8/",
            "not a url",
        ] {
            assert!(!is_local_origin(remote), "{remote}");
        }
    }

    #[test]
    fn a_host_is_refused_by_name_or_by_any_spelling_of_a_private_address() {
        for bad in [
            "localhost",
            "LOCALHOST",
            "api.localhost",
            "metadata.google.internal",
            "169.254.169.254",
            "10.0.0.5",
            "[::1]",
            "[fe80::1]",
            "3232235777",
        ] {
            assert!(check_host(bad).is_err(), "{bad}");
        }
        assert_eq!(
            check_host("api.example.com"),
            Ok(Some("api.example.com".into()))
        );
        assert_eq!(check_host("8.8.8.8"), Ok(None));
        assert_eq!(check_host("[2606:4700::1111]"), Ok(None));
    }

    #[test]
    fn private_reserved_and_multicast_ranges_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.1.2.3",
            "100.64.0.1",
            "192.0.2.1",
            "198.18.0.1",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "ff02::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public_ip(&ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:8.8.8.8",
        ] {
            assert!(is_public_ip(&ip.parse().unwrap()), "{ip}");
        }
    }
}
