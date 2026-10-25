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
//!    that do not resolve pass THIS check; a client whose target is client-chosen
//!    (the LLM legs) is built with [`guarded_client_builder`], whose DNS resolver
//!    re-applies the IP rules to the addresses it actually dials — so a name that
//!    is unresolvable (or public) at validation time and rebinds to a private
//!    address before the dial is still refused.
//!
//! ## Private targets the operator allows
//!
//! * `WAAV_PRIVATE_EGRESS_ALLOWLIST=host:port,host:port` — exact, PORT-SCOPED
//!   exemptions from rules 2-5 (e.g. `127.0.0.1:8000` for a local LLM, or
//!   `llm.lan:8080`). The port is mandatory: allowing a loopback model server
//!   must not open every other loopback service (admin APIs, Redis, the
//!   gateway itself).
//! * A named operator credential (`core::credentials`) with `private: true`
//!   exempts exactly its `host:port` entries, for requests that carry it.
//! * `WAAV_ALLOW_LOOPBACK_ENDPOINTS` (test escape hatch, OFF by default):
//!   `1/true` = loopback on ANY port (legacy, logs a WARN), or a comma list of
//!   ports (`8000,8001`) = loopback on those ports only. It exempts LOOPBACK
//!   ONLY — it used to skip every host check, so `169.254.169.254` (cloud
//!   metadata) and RFC1918 targets were reachable whenever it was on.
//!
//! This module is feature-free: it must be available to non-DAG builds.

use std::net::{IpAddr, Ipv4Addr};

use crate::config::utils::parse_bool;

pub(crate) const HTTP_URL_SCHEMES: &[&str] = &["http", "https"];
pub(crate) const HTTP_WS_URL_SCHEMES: &[&str] = &["http", "https", "ws", "wss"];

/// Env var: exact `host:port` targets exempt from the private-address rules.
pub(crate) const PRIVATE_EGRESS_ALLOWLIST_ENV: &str = "WAAV_PRIVATE_EGRESS_ALLOWLIST";

/// Which loopback targets the `WAAV_ALLOW_LOOPBACK_ENDPOINTS` escape hatch permits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoopbackAllowance {
    /// The default: loopback is blocked like every other private range.
    Off,
    /// `1/true/yes`: loopback on any port (legacy spelling; WARN at startup).
    AnyPort,
    /// `8000,8001`: loopback on these ports only.
    Ports(Vec<u16>),
}

impl LoopbackAllowance {
    pub(crate) fn permits(&self, port: Option<u16>) -> bool {
        match self {
            Self::Off => false,
            Self::AnyPort => true,
            Self::Ports(ports) => port.is_some_and(|p| ports.contains(&p)),
        }
    }
}

/// An exact `host:port` the operator exempted from the private-address rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPort {
    /// Lowercased host as `url::Url::host_str` spells it (IPv6 bracketed).
    pub host: String,
    pub port: u16,
}

impl HostPort {
    /// Parse `host:port` (`[v6]:port` for IPv6). The port is mandatory.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (host, port) = raw
            .rsplit_once(':')
            .filter(|(h, _)| !h.is_empty() && (!h.contains(':') || h.starts_with('[')))
            .ok_or_else(|| format!("'{raw}' is not host:port (the port is required)"))?;
        let port = port
            .parse::<u16>()
            .map_err(|_| format!("'{raw}': '{port}' is not a port"))?;
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    fn matches(&self, host: &str, port: Option<u16>) -> bool {
        port == Some(self.port) && self.host.eq_ignore_ascii_case(host)
    }
}

/// The operator's `WAAV_PRIVATE_EGRESS_ALLOWLIST`, read per call (cheap; lets tests and a
/// config reload see changes). Malformed entries are logged and skipped — never widened.
pub(crate) fn private_egress_allowlist() -> Vec<HostPort> {
    let Ok(raw) = std::env::var(PRIVATE_EGRESS_ALLOWLIST_ENV) else {
        return Vec::new();
    };
    raw.split(',')
        .filter(|e| !e.trim().is_empty())
        .filter_map(|entry| match HostPort::parse(entry) {
            Ok(hp) => Some(hp),
            Err(e) => {
                tracing::error!(error = %e, "{PRIVATE_EGRESS_ALLOWLIST_ENV}: entry ignored");
                None
            }
        })
        .collect()
}

/// The loopback escape hatch, from `WAAV_ALLOW_LOOPBACK_ENDPOINTS`. OFF by default; a
/// malformed value keeps it off (fail closed).
pub(crate) fn loopback_allowance() -> LoopbackAllowance {
    match std::env::var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") {
        Ok(value) => parse_loopback_allowance(&value).unwrap_or_else(|e| {
            tracing::error!(error = %e, "invalid loopback endpoint escape-hatch environment configuration");
            LoopbackAllowance::Off
        }),
        Err(std::env::VarError::NotPresent) => LoopbackAllowance::Off,
        Err(std::env::VarError::NotUnicode(_)) => {
            tracing::error!(
                "WAAV_ALLOW_LOOPBACK_ENDPOINTS must be valid UTF-8; keeping SSRF protection enabled"
            );
            LoopbackAllowance::Off
        }
    }
}

/// Pure parser: a boolean, or a comma list of ports.
fn parse_loopback_allowance(value: &str) -> Result<LoopbackAllowance, String> {
    if let Ok(enabled) = loopback_flag_enabled(Some(value)) {
        return Ok(if enabled {
            LoopbackAllowance::AnyPort
        } else {
            LoopbackAllowance::Off
        });
    }
    let ports = value
        .split(',')
        .map(|p| p.trim().parse::<u16>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| {
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS must be true/false/1/0/yes/no or a comma list of ports"
                .to_string()
        })?;
    Ok(LoopbackAllowance::Ports(ports))
}

/// The same exemptions as data, for the segmented speech-to-text uploader, which sits below the
/// gateway and cannot call it: `WAAV_ALLOW_LOOPBACK_ENDPOINTS` and `WAAV_PRIVATE_EGRESS_ALLOWLIST`
/// read through `get` (the caller's lookup, so tests need no process environment). Malformed
/// values keep loopback off and drop the bad allowlist entry, as [`loopback_allowance`] and
/// [`private_egress_allowlist`] do.
pub(crate) fn private_reach_from(
    get: impl Fn(&str) -> Option<String>,
) -> waav_segmented_stt::net::PrivateReach {
    let loopback_ports =
        match get("WAAV_ALLOW_LOOPBACK_ENDPOINTS").map(|v| parse_loopback_allowance(&v)) {
            Some(Ok(LoopbackAllowance::AnyPort)) => Some(Vec::new()),
            Some(Ok(LoopbackAllowance::Ports(ports))) => Some(ports),
            Some(Ok(LoopbackAllowance::Off)) | None => None,
            Some(Err(e)) => {
                tracing::error!(error = %e, "invalid loopback endpoint escape-hatch configuration");
                None
            }
        };
    let allowed = get(PRIVATE_EGRESS_ALLOWLIST_ENV)
        .unwrap_or_default()
        .split(',')
        .filter(|e| !e.trim().is_empty())
        .filter_map(|entry| match HostPort::parse(entry) {
            Ok(hp) => Some((hp.host, hp.port)),
            Err(e) => {
                tracing::error!(error = %e, "{PRIVATE_EGRESS_ALLOWLIST_ENV}: entry ignored");
                None
            }
        })
        .collect();
    waav_segmented_stt::net::PrivateReach {
        loopback_ports,
        allowed,
    }
}

/// Whether ANY loopback target is permitted (kept for callers that only need a yes/no).
pub(crate) fn loopback_endpoints_allowed() -> bool {
    loopback_allowance() != LoopbackAllowance::Off
}

/// Pure parser for the escape-hatch flag value (unit-testable without touching
/// process-global env state; see the note on [`tests`]).
fn loopback_flag_enabled(value: Option<&str>) -> Result<bool, String> {
    match value {
        Some(value) => parse_bool(value).ok_or_else(|| {
            "WAAV_ALLOW_LOOPBACK_ENDPOINTS must be one of true/false/1/0/yes/no".to_string()
        }),
        None => Ok(false),
    }
}

/// A process-wide pooled HTTP client for vendor calls, one per `key`, built by `build` on first
/// use and reused after.
///
/// A `reqwest::Client` IS its connection pool, so building one per request -- which the audio
/// paths did -- throws the pool away every time: each call paid DNS, TCP and TLS to the vendor.
/// Reusing it keeps keep-alive connections to each vendor host (one pool per host inside the
/// client), which is per-deployment reuse without keying on deployments. Key by whatever changes
/// the client's configuration (schemes, timeout). Build these HTTP/1.1-only: many long requests
/// on one shared HTTP/2 connection queue behind the vendor's concurrent-stream limit.
pub fn shared_http_client(
    key: &str,
    build: impl FnOnce() -> Result<reqwest::Client, reqwest::Error>,
) -> Result<reqwest::Client, reqwest::Error> {
    static CLIENTS: std::sync::LazyLock<dashmap::DashMap<String, reqwest::Client>> =
        std::sync::LazyLock::new(dashmap::DashMap::new);
    if let Some(client) = CLIENTS.get(key) {
        return Ok(client.clone());
    }
    let built = build()?;
    // Two first calls may race to build; both get the one that landed in the map.
    Ok(CLIENTS.entry(key.to_string()).or_insert(built).clone())
}

/// Validate a URL for SSRF (Server-Side Request Forgery) protection.
///
/// `allowed_schemes` must be lowercase (the URL's scheme is lowercased before
/// comparison). Returns `Ok(())` for safe URLs, or a human-readable reason why
/// the URL was rejected. See the module docs for the full rule set.
pub fn validate_url_for_ssrf(url: &str, allowed_schemes: &[&str]) -> Result<(), String> {
    validate_url_for_ssrf_inner(
        url,
        allowed_schemes,
        &loopback_allowance(),
        &private_egress_allowlist(),
    )
}

/// [`validate_url_for_ssrf`] that also exempts `granted` — the `host:port` entries of a named
/// operator credential with `private: true` that this request carries.
pub fn validate_url_for_ssrf_with_grants(
    url: &str,
    allowed_schemes: &[&str],
    granted: &[HostPort],
) -> Result<(), String> {
    let mut allowed = private_egress_allowlist();
    allowed.extend_from_slice(granted);
    validate_url_for_ssrf_inner(url, allowed_schemes, &loopback_allowance(), &allowed)
}

/// [`validate_url_for_ssrf`] without the resolve-then-validate step: the scheme allowlist, the
/// blocked hostnames and every IP-literal spelling, and nothing that does I/O.
///
/// For a synchronous constructor on the request path. `validate_url_for_ssrf` resolves a DNS
/// name with a blocking `getaddrinfo`, and a constructor that runs on a tokio worker holds that
/// worker for the whole lookup (~70 ms for an external name under Kubernetes' `ndots:5`), so
/// every other session on the replica waits with it. Pair this with the full check off the
/// workers (`tokio::task::spawn_blocking`) before the first dial.
pub fn validate_url_for_ssrf_without_dns(
    url: &str,
    allowed_schemes: &[&str],
) -> Result<(), String> {
    ssrf_dns_host(
        url,
        allowed_schemes,
        &loopback_allowance(),
        &private_egress_allowlist(),
    )
    .map(|_| ())
}

/// Build a reqwest redirect policy that validates every redirect target before
/// following it. Use this for requests whose original URL passed
/// [`validate_url_for_ssrf`]; reqwest follows redirects by default, and an
/// attacker-controlled public URL can otherwise redirect to a private target.
pub(crate) fn ssrf_protected_redirect_policy(
    allowed_schemes: &'static [&'static str],
) -> reqwest::redirect::Policy {
    const MAX_REDIRECTS: usize = 10;

    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }

        let next = attempt.url().as_str();
        match validate_url_for_ssrf(next, allowed_schemes) {
            Ok(()) => attempt.follow(),
            Err(msg) => attempt.error(format!("redirect URL rejected (SSRF protection): {msg}")),
        }
    })
}

/// Build a reqwest client builder whose redirects are constrained by
/// [`ssrf_protected_redirect_policy`].
pub(crate) fn ssrf_protected_client_builder(
    allowed_schemes: &'static [&'static str],
) -> reqwest::ClientBuilder {
    reqwest::Client::builder().redirect(ssrf_protected_redirect_policy(allowed_schemes))
}

/// A DNS resolver that applies the IP rules to the addresses a client ACTUALLY dials
/// (connect-time SSRF check), closing the validate-then-resolve-again window (DNS rebinding)
/// and the "unresolvable at validation time" pass-through. Private addresses are allowed only
/// for `allowed_names` (from the operator's port-scoped allowlist / credential grants — the port
/// itself was enforced when the URL was validated) and, under the loopback escape hatch, for
/// loopback addresses.
#[derive(Debug, Clone)]
pub(crate) struct GuardedResolver {
    allowed_names: Vec<String>,
    loopback: LoopbackAllowance,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(guarded_lookup(
            self.clone(),
            name.as_str().to_ascii_lowercase(),
        ))
    }
}

async fn guarded_lookup(
    resolver: GuardedResolver,
    host: String,
) -> Result<reqwest::dns::Addrs, Box<dyn std::error::Error + Send + Sync>> {
    let addrs: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
    let name_allowed = resolver
        .allowed_names
        .iter()
        .any(|n| n.eq_ignore_ascii_case(&host));
    for addr in &addrs {
        let ip = addr.ip();
        let loopback_ok = ip.is_loopback() && resolver.loopback != LoopbackAllowance::Off;
        if is_private_ip(&ip) && !name_allowed && !loopback_ok {
            return Err(format!(
                "'{host}' resolves to private/internal IP '{ip}' (SSRF protection, connect time)"
            )
            .into());
        }
    }
    Ok(Box::new(addrs.into_iter()))
}

/// [`ssrf_protected_client_builder`] plus the connect-time [`GuardedResolver`]. Use it for every
/// client whose target a CLIENT chose (LLM `base_url`s); operator-addressed clients (the auth
/// responder, the Bud gateway) keep [`ssrf_protected_client_builder`]. `granted` are the
/// request's credential grants (see [`validate_url_for_ssrf_with_grants`]).
pub(crate) fn guarded_client_builder(
    allowed_schemes: &'static [&'static str],
    granted: &[HostPort],
) -> reqwest::ClientBuilder {
    let mut allowed_names: Vec<String> = private_egress_allowlist()
        .into_iter()
        .map(|hp| hp.host)
        .collect();
    allowed_names.extend(granted.iter().map(|hp| hp.host.clone()));
    ssrf_protected_client_builder(allowed_schemes).dns_resolver(std::sync::Arc::new(
        GuardedResolver {
            allowed_names,
            loopback: loopback_allowance(),
        },
    ))
}

/// [`guarded_client_builder`] with no credential grants, built: for clients whose target a client
/// chose but which carry no operator credential (DAG HTTP endpoints and webhooks). The URL check
/// before the request and this resolver at connect time apply the same rules, so a name that
/// resolves to a public address when checked and a private one when dialled (DNS rebinding) is
/// refused.
pub(crate) fn guarded_client(
    allowed_schemes: &'static [&'static str],
) -> Result<reqwest::Client, reqwest::Error> {
    guarded_client_builder(allowed_schemes, &[]).build()
}

/// Build a reqwest client whose redirects are constrained by
/// [`ssrf_protected_redirect_policy`].
pub(crate) fn ssrf_protected_client(
    allowed_schemes: &'static [&'static str],
) -> Result<reqwest::Client, reqwest::Error> {
    ssrf_protected_client_builder(allowed_schemes).build()
}

/// Rule engine behind [`validate_url_for_ssrf`], with the loopback escape
/// hatch injected so the gate can be unit-tested deterministically. The env
/// var is read by callers/tests in several modules WITHOUT synchronization, so
/// the unit tests for the gate must not set it (see [`tests`]).
fn validate_url_for_ssrf_inner(
    url: &str,
    allowed_schemes: &[&str],
    loopback: &LoopbackAllowance,
    allowed: &[HostPort],
) -> Result<(), String> {
    match ssrf_dns_host(url, allowed_schemes, loopback, allowed)? {
        // Resolve-then-validate: when the host is a DNS name (not an IP literal),
        // resolve it and reject if ANY resolved address is private/internal. (The
        // dial re-resolves; clients of client-chosen targets close that window with
        // `GuardedResolver`.)
        Some((host, port)) => validate_resolved_host_inner(&host, loopback.permits(port)),
        None => Ok(()),
    }
}

/// Hostnames that name the loopback interface (the escape hatch's scope).
fn is_loopback_name(host_lower: &str) -> bool {
    matches!(
        host_lower,
        "localhost"
            | "localhost.localdomain"
            | "127.0.0.1"
            | "::1"
            | "[::1]"
            | "[::ffff:127.0.0.1]"
    )
}

/// Every check [`validate_url_for_ssrf_inner`] makes that needs no I/O. `Ok(Some(host))` when
/// the host is a DNS name still to be resolved, `Ok(None)` when nothing is left to check.
fn ssrf_dns_host(
    url: &str,
    allowed_schemes: &[&str],
    loopback: &LoopbackAllowance,
    allowed: &[HostPort],
) -> Result<Option<(String, Option<u16>)>, String> {
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

    let host = parsed
        .host_str()
        .ok_or_else(|| format!("URL '{}' has no host", url))?;
    let port = parsed.port_or_known_default();

    // An exact host:port the operator allowed (allowlist or a credential grant): no host rule
    // applies to it. Port-scoped, so allowing one loopback service opens no other.
    if allowed.iter().any(|hp| hp.matches(host, port)) {
        return Ok(None);
    }

    // Blocked hostnames and every IP-literal spelling: the rule segmented sessions apply too.
    // The loopback escape hatch (test/local-mock only, OFF by default) exempts LOOPBACK ONLY —
    // never link-local / cloud metadata / private ranges — and only on its permitted ports.
    match waav_segmented_stt::net::check_host(host) {
        Ok(Some(name)) => Ok(Some((name, port))),
        Ok(None) => Ok(None),
        Err(_) if loopback.permits(port) && is_loopback_host(host) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Whether a URL host (as `url::Url::host_str` writes it) names the loopback interface: a
/// loopback hostname or a loopback IP literal in any spelling `check_host` recognises.
fn is_loopback_host(host: &str) -> bool {
    let host_lower = host.to_lowercase();
    if is_loopback_name(&host_lower) {
        return true;
    }
    let bare = host_lower
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&host_lower);
    if let Ok(ip) = bare.parse::<IpAddr>() {
        return match ip {
            IpAddr::V6(v6) => {
                v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
            }
            IpAddr::V4(v4) => v4.is_loopback(),
        };
    }
    !bare.is_empty()
        && bare.bytes().all(|b| b.is_ascii_digit())
        && bare
            .parse::<u32>()
            .is_ok_and(|n| Ipv4Addr::from(n).is_loopback())
}

/// Resolve a DNS hostname and reject if any resolved IP is private/internal.
///
/// Uses a synchronous resolver (`ToSocketAddrs`) so it can run inside the
/// (synchronous) DAG compile path. A host that fails to resolve is allowed
/// through (it may resolve later; at request time the transport applies its
/// own checks); the goal here is to reject hosts that *currently* resolve to a
/// blocked range, which is the DNS-rebind vector for client-supplied targets.
pub fn validate_resolved_host_for_ssrf(host: &str) -> Result<(), String> {
    validate_resolved_host_inner(host, false)
}

/// [`validate_resolved_host_for_ssrf`], optionally exempting LOOPBACK addresses (the escape
/// hatch, already port-checked by the caller) — never any other private range.
fn validate_resolved_host_inner(host: &str, loopback_ok: bool) -> Result<(), String> {
    use std::net::ToSocketAddrs;

    // Port is irrelevant for the IP check; use a dummy port for resolution.
    let resolved = match (host, 0u16).to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(_) => return Ok(()), // unresolvable now; nothing to block
    };

    for addr in resolved {
        let ip = addr.ip();
        if is_private_ip(&ip) && !(loopback_ok && ip.is_loopback()) {
            return Err(format!(
                "Host '{}' resolves to private/internal IP '{}' (SSRF protection)",
                host, ip
            ));
        }
    }

    Ok(())
}

/// Check if an IP address is private/internal: not on the public internet by the rule segmented
/// sessions' upload pools use (loopback, private, link-local and cloud metadata, CGNAT, current
/// network, documentation, benchmarking, reserved, multicast, broadcast; IPv4-mapped addresses
/// judged as IPv4).
fn is_private_ip(ip: &IpAddr) -> bool {
    !waav_segmented_stt::net::is_public_ip(ip)
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

/// Serializes every test that SETS or DEPENDS ON `WAAV_ALLOW_LOOPBACK_ENDPOINTS`.
/// Env vars are process-global: module-local locks cannot prevent a parallel test in
/// another module from observing a temporarily-set allow-loopback window (the rotating
/// SSRF-test flake). Setters take the lock for their whole set→assert→restore span;
/// rejection-assertion tests take it for their assertion so they never overlap a window.
/// Keys the SAME mutex as [`test_env_lock`] so both entry points exclude each other.
/// `unwrap_or_else(PoisonError::into_inner)` — a poisoned lock must not cascade.
#[cfg(test)]
pub(crate) fn ssrf_env_lock() -> std::sync::MutexGuard<'static, ()> {
    test_env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_shared_client_is_built_once_per_key() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let builds = AtomicUsize::new(0);
        let build = || {
            builds.fetch_add(1, Ordering::SeqCst);
            reqwest::Client::builder().http1_only().build()
        };
        let key = "net-tests-shared-client-once";
        shared_http_client(key, build).unwrap();
        shared_http_client(key, build).unwrap();
        assert_eq!(
            builds.load(Ordering::SeqCst),
            1,
            "the second call reuses the first client"
        );
        shared_http_client("net-tests-shared-client-other", build).unwrap();
        assert_eq!(
            builds.load(Ordering::SeqCst),
            2,
            "another key builds its own"
        );
    }
    use super::*;

    #[test]
    fn segmented_uploads_get_the_same_exemptions_as_data() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        let off = private_reach_from(env(&[]));
        assert_eq!(off, waav_segmented_stt::net::PrivateReach::default());
        let any = private_reach_from(env(&[("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1")]));
        assert_eq!(any.loopback_ports, Some(Vec::new()));
        let ports = private_reach_from(env(&[("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "8080,9000")]));
        assert_eq!(ports.loopback_ports, Some(vec![8080, 9000]));
        let bad = private_reach_from(env(&[("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "maybe")]));
        assert_eq!(bad.loopback_ports, None, "a malformed hatch stays off");
        let listed = private_reach_from(env(&[(
            "WAAV_PRIVATE_EGRESS_ALLOWLIST",
            "10.0.0.5:8000, nonsense ,LLM.lan:8080",
        )]));
        assert_eq!(
            listed.allowed,
            vec![
                ("10.0.0.5".to_string(), 8000),
                ("llm.lan".to_string(), 8080)
            ]
        );
    }

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
        assert!(is_private_ip(&IpAddr::V4(std::net::Ipv4Addr::from(
            2130706433u32
        ))));
    }

    #[test]
    fn private_ipv6_literals_rejected() {
        let _guard = env_guard();
        for host in [
            "[::1]",              // loopback (also hostname-blocked)
            "[fe80::1]",          // link-local
            "[febf::1]",          // link-local upper edge
            "[fc00::1]",          // unique-local
            "[fdff::1]",          // unique-local upper edge
            "[2001:db8::1]",      // documentation
            "[::ffff:127.0.0.1]", // IPv4-mapped loopback
            "[::ffff:10.0.0.1]",  // IPv4-mapped RFC1918
            "[::ffff:a9fe:a9fe]", // IPv4-mapped 169.254.169.254
            "[ff02::1]",          // multicast all-nodes (audit gap)
            "[ff05::2]",          // multicast site-local scope
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

    /// The DNS-free variant makes every check but the resolution: a name that resolves to a
    /// private address passes it (the full check refuses it), everything else is refused alike.
    #[test]
    fn without_dns_skips_only_the_resolution() {
        let _guard = env_guard();
        for bad in [
            "ftp://example.com/x",
            "https://127.0.0.1/x",
            "https://10.0.0.5/x",
            "https://[::1]/x",
            "https://3232235777/x",
            "https://localhost/x",
            "https://169.254.169.254/x",
        ] {
            assert!(
                validate_url_for_ssrf_without_dns(bad, HTTP_SCHEMES).is_err(),
                "{bad}"
            );
            assert!(validate_url_for_ssrf(bad, HTTP_SCHEMES).is_err(), "{bad}");
        }
        assert!(validate_url_for_ssrf_without_dns("https://8.8.8.8/x", HTTP_SCHEMES).is_ok());
        // `localhost.` (trailing dot) is not on the blocked list but resolves to loopback.
        if validate_resolved_host_for_ssrf("localhost.").is_err() {
            assert!(
                validate_url_for_ssrf_without_dns("https://localhost./x", HTTP_SCHEMES).is_ok()
            );
            assert!(validate_url_for_ssrf("https://localhost./x", HTTP_SCHEMES).is_err());
        }
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
    fn loopback_gate_on_permits_loopback_only() {
        let on = LoopbackAllowance::AnyPort;
        for url in [
            "http://127.0.0.1:8080/v1",
            "http://localhost/x",
            "http://[::1]:9000/x",
        ] {
            assert!(
                validate_url_for_ssrf_inner(url, HTTP_SCHEMES, &on, &[]).is_ok(),
                "{url} must be allowed with the gate on"
            );
        }
        // The gate is a LOOPBACK hatch: metadata, link-local and RFC1918 stay blocked (they
        // used to be allowed whenever the gate was on).
        for url in [
            "http://169.254.169.254/latest/meta-data",
            "http://10.0.0.1/x",
            "http://192.168.1.1/x",
            "http://metadata.google.internal/x",
        ] {
            assert!(
                validate_url_for_ssrf_inner(url, HTTP_SCHEMES, &on, &[]).is_err(),
                "{url} must stay blocked with the gate on"
            );
        }
        // Scheme allowlist still applies with the gate on.
        assert!(
            validate_url_for_ssrf_inner("ftp://example.com/x", HTTP_SCHEMES, &on, &[]).is_err()
        );
        assert!(validate_url_for_ssrf_inner("ws://127.0.0.1/x", HTTP_SCHEMES, &on, &[]).is_err());
    }

    #[test]
    fn loopback_gate_can_be_port_scoped() {
        let ports = parse_loopback_allowance("8000, 8001").unwrap();
        assert_eq!(ports, LoopbackAllowance::Ports(vec![8000, 8001]));
        assert!(
            validate_url_for_ssrf_inner("http://127.0.0.1:8000/v1", HTTP_SCHEMES, &ports, &[])
                .is_ok()
        );
        assert!(
            validate_url_for_ssrf_inner("http://127.0.0.1:6379/", HTTP_SCHEMES, &ports, &[])
                .is_err()
        );
        assert!(
            validate_url_for_ssrf_inner("http://127.0.0.1/", HTTP_SCHEMES, &ports, &[]).is_err()
        );
        assert!(parse_loopback_allowance("8000,redis").is_err());
    }

    #[test]
    fn allowlist_entries_are_exact_host_and_port() {
        let allowed = [
            HostPort::parse("127.0.0.1:8000").unwrap(),
            HostPort::parse("LLM.lan:8080").unwrap(),
        ];
        let off = LoopbackAllowance::Off;
        assert!(
            validate_url_for_ssrf_inner("http://127.0.0.1:8000/v1", HTTP_SCHEMES, &off, &allowed)
                .is_ok()
        );
        assert!(
            validate_url_for_ssrf_inner("http://127.0.0.1:8001/v1", HTTP_SCHEMES, &off, &allowed)
                .is_err()
        );
        assert!(
            validate_url_for_ssrf_inner("http://llm.lan:8080/v1", HTTP_SCHEMES, &off, &allowed)
                .is_ok()
        );
        assert!(
            HostPort::parse("127.0.0.1").is_err(),
            "the port is mandatory"
        );
        assert!(HostPort::parse("[::1]:8000").is_ok());
    }

    /// The env-flag parser itself (pure; no env mutation needed).
    #[test]
    fn loopback_flag_parsing() {
        for on in [Some("1"), Some("true"), Some("TRUE"), Some("yes")] {
            assert!(
                loopback_flag_enabled(on).expect("truthy value parses"),
                "{on:?} must enable the gate"
            );
        }
        for off in [None, Some("0"), Some("false"), Some("FALSE"), Some("no")] {
            assert!(
                !loopback_flag_enabled(off).expect("false value parses"),
                "{off:?} must keep the gate off"
            );
        }
        for malformed in [Some(""), Some("maybe"), Some("on")] {
            let err =
                loopback_flag_enabled(malformed).expect_err("malformed bool must fail closed");
            assert!(
                err.contains("WAAV_ALLOW_LOOPBACK_ENDPOINTS"),
                "error should name bad env var: {err}"
            );
        }
    }

    /// DAG HTTP endpoints and webhooks dial through the guarded resolver: a name that resolves to a
    /// private address at connect time is refused even when nothing checked the URL first (the
    /// DNS-rebinding window between the URL check and reqwest's own lookup).
    #[tokio::test(flavor = "current_thread")]
    async fn guarded_client_refuses_a_name_that_resolves_private_at_connect_time() {
        let _guard = env_guard();
        // SAFETY: test-only env mutation, serialized by the guard above.
        unsafe { std::env::remove_var("WAAV_PRIVATE_EGRESS_ALLOWLIST") };
        let client = guarded_client(HTTP_SCHEMES).expect("client");
        let err = client
            .get("http://localhost:9/metadata")
            .send()
            .await
            .expect_err("a loopback address at connect time is refused");
        let chain = format!("{err:?}");
        assert!(chain.contains("SSRF protection, connect time"), "{chain}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn ssrf_redirect_policy_rejects_private_redirect_hop() {
        let _guard = env_guard();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local redirect test server");
        let addr = listener.local_addr().expect("local addr");

        tokio::spawn(async move {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let response = concat!(
                "HTTP/1.1 302 Found\r\n",
                "Location: http://127.0.0.1:9/metadata\r\n",
                "Content-Length: 0\r\n",
                "\r\n"
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
        });

        let client = ssrf_protected_client(HTTP_SCHEMES).expect("client");
        let err = client
            .get(format!("http://{addr}/start"))
            .send()
            .await
            .expect_err("private redirect target must be rejected");
        let mut error_chain = err.to_string();
        let mut source = std::error::Error::source(&err);
        while let Some(error) = source {
            error_chain.push_str(": ");
            error_chain.push_str(&error.to_string());
            source = error.source();
        }
        assert!(
            error_chain.contains("redirect URL rejected"),
            "unexpected redirect error: {error_chain}"
        );
    }

    #[test]
    fn url_without_host_rejected() {
        let _guard = env_guard();
        assert!(validate_url_for_ssrf("unix:/tmp/x.sock", &["unix"]).is_err());
    }
}
