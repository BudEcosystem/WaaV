//! Which local networks WaaV's LiveKit participant gathers ICE candidates (its media sockets) on.
//!
//! libwebrtc enumerates every interface (the LAN, each Docker bridge, an IPv6 ULA, VPNs) and skips
//! loopback, so a single-machine deployment whose LiveKit server is on `127.0.0.1` still opened its
//! media sockets on the network and none on loopback (Gaia 25 Oct §20). Two rules, applied before
//! each room connection:
//!
//! - when the LiveKit URL is loopback, loopback is gathered too (additive: nothing that connects
//!   today stops connecting);
//! - `WAAV_LIVEKIT_ICE_IPS=<CIDR,…>` keeps only networks whose address is in one of the prefixes
//!   (`127.0.0.1/32` for loopback only). Unset: every network, as before.
//!
//! libwebrtc's network stack is process-wide, so the policy is too: the last connection's URL
//! decides the loopback rule, which only matters for a gateway that talks to two LiveKit servers.

use std::net::IpAddr;

/// Env var: comma-separated CIDR prefixes the participant may gather candidates on.
pub const ICE_IPS_ENV: &str = "WAAV_LIVEKIT_ICE_IPS";

/// One allowed prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IcePrefix {
    pub ip: IpAddr,
    pub len: u8,
}

impl IcePrefix {
    fn contains(&self, ip: IpAddr) -> bool {
        match (self.ip, ip) {
            (IpAddr::V4(p), IpAddr::V4(a)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.len)).unwrap_or(0);
                u32::from(p) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(p), IpAddr::V6(a)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.len))
                    .unwrap_or(0);
                u128::from(p) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

impl std::fmt::Display for IcePrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.ip, self.len)
    }
}

/// Parse `WAAV_LIVEKIT_ICE_IPS`. Empty or unset: no restriction.
pub fn parse_ice_ips(value: Option<&str>) -> Result<Vec<IcePrefix>, String> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (ip, len) = p.split_once('/').unwrap_or((p, ""));
            let ip: IpAddr = ip
                .parse()
                .map_err(|_| format!("{ICE_IPS_ENV}: '{p}' is not an IP address or CIDR"))?;
            let max = if ip.is_ipv4() { 32 } else { 128 };
            let len = if len.is_empty() {
                max
            } else {
                len.parse::<u8>()
                    .ok()
                    .filter(|l| *l <= max)
                    .ok_or_else(|| format!("{ICE_IPS_ENV}: '{p}' has an invalid prefix length"))?
            };
            Ok(IcePrefix { ip, len })
        })
        .collect()
}

/// The policy for a connection to `livekit_url`: the allowed prefixes, and whether loopback is
/// gathered (the URL is loopback, or a prefix covers loopback).
pub fn policy_for(livekit_url: &str, prefixes: &[IcePrefix]) -> (Vec<String>, bool) {
    let url_is_loopback = url::Url::parse(livekit_url)
        .ok()
        .and_then(|u| u.host().map(|h| h.to_owned()))
        .is_some_and(|host| match host {
            url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
            url::Host::Ipv4(ip) => ip.is_loopback(),
            url::Host::Ipv6(ip) => ip.is_loopback(),
        });
    let covers_loopback = prefixes.iter().any(|p| {
        p.contains(IpAddr::from([127, 0, 0, 1]))
            || p.contains(IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 1]))
    });
    (
        prefixes.iter().map(ToString::to_string).collect(),
        url_is_loopback || covers_loopback,
    )
}

/// Apply the policy for a connection to `livekit_url`, before it creates its peer connections.
pub fn apply_for(livekit_url: &str) {
    let prefixes = match parse_ice_ips(std::env::var(ICE_IPS_ENV).ok().as_deref()) {
        Ok(p) => p,
        Err(e) => {
            // Fail closed: an operator who set a restriction must not silently get every
            // interface. No network matches `0.0.0.0/32`, so the call fails, loudly;
            // `waav-gateway check` reports the value.
            tracing::error!(error = %e, "gathering no ICE candidates (fail closed)");
            vec![IcePrefix {
                ip: IpAddr::from([0, 0, 0, 0]),
                len: 32,
            }]
        }
    };
    let (cidrs, include_loopback) = policy_for(livekit_url, &prefixes);
    tracing::debug!(?cidrs, include_loopback, "LiveKit ICE network policy");
    webrtc_sys::peer_connection_factory::ffi::set_ice_network_policy(cidrs, include_loopback);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_parse_and_reject_garbage() {
        let p = parse_ice_ips(Some(" 127.0.0.1/32, 192.168.1.0/24 ,::1 ")).unwrap();
        assert_eq!(
            p.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["127.0.0.1/32", "192.168.1.0/24", "::1/128"]
        );
        assert!(parse_ice_ips(None).unwrap().is_empty());
        assert!(parse_ice_ips(Some("  ")).unwrap().is_empty());
        assert!(parse_ice_ips(Some("10.0.0.0/33")).is_err());
        assert!(parse_ice_ips(Some("lan")).is_err());
    }

    #[test]
    fn loopback_is_gathered_for_a_loopback_server_or_a_loopback_prefix() {
        for url in ["ws://127.0.0.1:7880", "wss://localhost", "ws://[::1]:7880"] {
            assert!(policy_for(url, &[]).1, "{url}");
        }
        assert!(!policy_for("wss://livekit.example.com", &[]).1);
        let lan = parse_ice_ips(Some("192.168.1.0/24")).unwrap();
        assert_eq!(
            policy_for("ws://10.0.0.5:7880", &lan),
            (vec!["192.168.1.0/24".to_string()], false)
        );
        let lo = parse_ice_ips(Some("127.0.0.0/8")).unwrap();
        assert!(policy_for("ws://10.0.0.5:7880", &lo).1);
    }

    #[test]
    fn a_prefix_matches_its_addresses_only() {
        let p = parse_ice_ips(Some("172.16.0.0/12")).unwrap()[0];
        assert!(p.contains("172.17.0.1".parse().unwrap()));
        assert!(!p.contains("172.32.0.1".parse().unwrap()));
        assert!(!p.contains("::1".parse().unwrap()));
        let all = parse_ice_ips(Some("0.0.0.0/0")).unwrap()[0];
        assert!(all.contains("8.8.8.8".parse().unwrap()));
    }
}
