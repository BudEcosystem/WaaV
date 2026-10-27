//! Readiness evaluation for the `/readyz` probe (W-C1 / E13).
//!
//! Liveness (`/livez`) answers "is the process up?" — it must stay cheap and never depend on an
//! upstream. Readiness (`/readyz`) answers "should this instance receive traffic *right now*?"
//! and therefore *does* depend on configuration + upstream provider health:
//!
//! 1. **Config loaded** — `AppState` exists, so by construction the config parsed.
//! 2. **Enabled-provider credentials present** — every provider whose credentials are configured
//!    in [`ServerConfig`] is "enabled"; a misconfigured/blank credential makes the instance
//!    not-ready for that provider.
//! 3. **Cached TCP reachability (optional)** — a short TCP connect to each enabled provider's
//!    host:port, all providers at once, the result cached for [`CACHE_TTL`]. A reachable
//!    provider keeps the instance ready; an unreachable one flips `/readyz` to `503` and is
//!    named in the per-provider JSON breakdown.
//!
//! The probe target for a provider is where the gateway actually sends its traffic: the openai
//! providers' `OPENAI_BASE_URL` when it is set, else the vendor's well-known host. It can be
//! overridden with `WAAV_READINESS_PROBE_<PROVIDER>` (value `host:port`, provider upper-cased with
//! `-`→`_`), which is also how tests point a provider at a known-closed port for the 503 path.
//!
//! `/readyz` is public and outside the rate limiter, so it must not let a caller make the gateway
//! open outbound connections at will: concurrent requests share one evaluation and a result is
//! reused for [`CACHE_TTL`], and every lookup and connect is bounded by [`PROBE_TIMEOUT`] on the
//! async runtime (a blocking `getaddrinfo` used to hold a blocking-pool thread per request).

use std::collections::BTreeMap;
use std::time::Duration;

use serde::Serialize;

use crate::config::ServerConfig;

/// How long to wait for a single TCP reachability probe (lookup and connect) before declaring a
/// provider unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

/// How long one evaluation answers `/readyz`: a probe storm costs one probe per provider per
/// window, and a kubelet polling every few seconds still sees a change within one window.
pub const CACHE_TTL: Duration = Duration::from_secs(10);

/// Per-provider readiness detail in the `/readyz` JSON breakdown.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ProviderReadiness {
    /// Provider name (e.g. `deepgram`).
    pub provider: String,
    /// Whether a non-empty credential is configured for this provider.
    pub credential_present: bool,
    /// TCP reachability of the provider endpoint: `Some(true/false)` if probed, `None` if the
    /// host could not be resolved/derived (treated as not-ready).
    pub reachable: Option<bool>,
    /// Whether this provider is fully ready (credential present AND reachable).
    pub ready: bool,
    /// Human-readable reason when `ready == false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// The aggregate `/readyz` report.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReadinessReport {
    /// `"ready"` when every enabled provider is ready, else `"not_ready"`.
    pub status: String,
    /// Whether configuration is loaded (always true once `AppState` exists).
    pub config_loaded: bool,
    /// Per-provider breakdown.
    pub providers: Vec<ProviderReadiness>,
}

impl ReadinessReport {
    /// True when the instance should receive traffic (HTTP 200), false → HTTP 503.
    pub fn is_ready(&self) -> bool {
        self.config_loaded && self.providers.iter().all(|p| p.ready)
    }
}

/// Enabled providers and their default probe host:port, derived from configured credentials.
///
/// Only providers with a configured credential are considered "enabled". The host:port is the
/// vendor's well-known endpoint (used purely as a cheap reachability probe target).
fn enabled_provider_targets(config: &ServerConfig) -> BTreeMap<String, Option<(String, u16)>> {
    let mut out: BTreeMap<String, Option<(String, u16)>> = BTreeMap::new();

    let mut add = |name: &str, present: bool, host: &str| {
        if present {
            out.insert(name.to_string(), Some((host.to_string(), 443)));
        }
    };

    add(
        "deepgram",
        config
            .deepgram_api_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "api.deepgram.com",
    );
    add(
        "elevenlabs",
        config
            .elevenlabs_api_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "api.elevenlabs.io",
    );
    add(
        "cartesia",
        config
            .cartesia_api_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "api.cartesia.ai",
    );
    add(
        "assemblyai",
        config
            .assemblyai_api_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "api.assemblyai.com",
    );
    add(
        "hume",
        config
            .hume_api_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "api.hume.ai",
    );
    add(
        "groq",
        config
            .groq_api_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "api.groq.com",
    );
    add(
        "microsoft-azure",
        config
            .azure_speech_subscription_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "westus.api.cognitive.microsoft.com",
    );

    if config
        .openai_api_key
        .as_deref()
        .is_some_and(|k| !k.is_empty())
    {
        out.insert(
            "openai".to_string(),
            Some(openai_probe_target(
                std::env::var("OPENAI_BASE_URL").ok().as_deref(),
            )),
        );
    }

    out
}

/// Where the openai providers send their traffic: `OPENAI_BASE_URL`'s host and port when it is set
/// (an OpenAI-compatible local server, a proxy), else `api.openai.com:443`. Probing the cloud while
/// every request went to a local server made an offline gateway report itself not ready.
fn openai_probe_target(base: Option<&str>) -> (String, u16) {
    base.map(str::trim)
        .filter(|b| !b.is_empty())
        .and_then(|b| url::Url::parse(b).ok())
        .and_then(|u| Some((u.host_str()?.to_string(), u.port_or_known_default()?)))
        .map(|(host, port)| {
            // `[::1]` → `::1`: the socket-address lookup takes the bare IPv6 literal.
            let host = host
                .strip_prefix('[')
                .and_then(|h| h.strip_suffix(']'))
                .map(str::to_string)
                .unwrap_or(host);
            (host, port)
        })
        .unwrap_or_else(|| ("api.openai.com".to_string(), 443))
}

/// Apply the `WAAV_READINESS_PROBE_<PROVIDER>` override if present (host:port).
fn probe_override(provider: &str) -> Option<(String, u16)> {
    let key = format!(
        "WAAV_READINESS_PROBE_{}",
        provider.to_uppercase().replace('-', "_")
    );
    let val = std::env::var(key).ok()?;
    let (host, port) = val.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    Some((host.to_string(), port))
}

/// Probe one TCP endpoint: the lookup and the connect each bounded by [`PROBE_TIMEOUT`], on the
/// async runtime, so no request waits on an unbounded `getaddrinfo`. `true` if a connection was
/// established.
async fn tcp_reachable(host: &str, port: u16) -> bool {
    let Ok(Ok(addrs)) =
        tokio::time::timeout(PROBE_TIMEOUT, tokio::net::lookup_host((host, port))).await
    else {
        return false;
    };
    for addr in addrs {
        if let Ok(Ok(_)) =
            tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(addr)).await
        {
            return true;
        }
    }
    false
}

/// Evaluate readiness for the given config.
///
/// `probe_network` gates the TCP reachability check: when `false`, only credential presence is
/// evaluated (`reachable` is reported as `None` but does not, by itself, fail readiness). When
/// `true`, every enabled provider is probed at once and an unreachable one flips the report to
/// not-ready.
pub async fn evaluate(config: &ServerConfig, probe_network: bool) -> ReadinessReport {
    run_plan(probe_plan(config), probe_network).await
}

/// Each enabled provider and the `host:port` its probe dials (the override wins).
type ProbePlan = Vec<(String, Option<(String, u16)>)>;

fn probe_plan(config: &ServerConfig) -> ProbePlan {
    enabled_provider_targets(config)
        .into_iter()
        .map(|(provider, default_target)| {
            let target = probe_override(&provider).or(default_target);
            (provider, target)
        })
        .collect()
}

async fn run_plan(targets: ProbePlan, probe_network: bool) -> ReadinessReport {
    let probes = targets.iter().map(|(_, target)| async move {
        match target {
            Some((host, port)) if probe_network => Some(tcp_reachable(host, *port).await),
            _ => None,
        }
    });
    let reached = futures::future::join_all(probes).await;
    let providers: Vec<ProviderReadiness> = targets
        .into_iter()
        .zip(reached)
        .map(|((provider, target), reached)| {
            let (reachable, ready, detail) = match (probe_network, target, reached) {
                (false, _, _) => (None, true, None),
                (true, Some((host, port)), Some(ok)) => (
                    Some(ok),
                    ok,
                    (!ok).then(|| format!("TCP probe to {host}:{port} failed")),
                ),
                (true, _, _) => (
                    None,
                    false,
                    Some("no probe endpoint could be derived".to_string()),
                ),
            };
            ProviderReadiness {
                provider,
                credential_present: true,
                reachable,
                ready,
                detail,
            }
        })
        .collect();
    let all_ready = providers.iter().all(|p| p.ready);
    ReadinessReport {
        status: if all_ready { "ready" } else { "not_ready" }.to_string(),
        config_loaded: true,
        providers,
    }
}

/// The last evaluation: when it was made, for which plan and probe setting, and its report.
type CachedReport = Option<(std::time::Instant, (ProbePlan, bool), ReadinessReport)>;

/// [`evaluate`] behind a [`CACHE_TTL`] cache. Concurrent callers wait on one lock, so a burst of
/// requests costs one evaluation; each later caller in the window reads the result. Keyed by what
/// would be probed, so a different configuration (or a changed override) is never answered from
/// another's result.
pub async fn evaluate_cached(config: &ServerConfig, probe_network: bool) -> ReadinessReport {
    static CACHE: tokio::sync::Mutex<CachedReport> = tokio::sync::Mutex::const_new(None);
    let key = (probe_plan(config), probe_network);
    let mut cached = CACHE.lock().await;
    if let Some((at, cached_key, report)) = cached.as_ref()
        && *cached_key == key
        && at.elapsed() < CACHE_TTL
    {
        return report.clone();
    }
    let report = run_plan(key.0.clone(), probe_network).await;
    *cached = Some((std::time::Instant::now(), key, report.clone()));
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DAGTimeoutsConfig, PluginConfig};

    fn base_config() -> ServerConfig {
        ServerConfig {
            host: "127.0.0.1".to_string(),
            port: 3001,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: None,
            elevenlabs_api_key: None,
            google_credentials: None,
            azure_speech_subscription_key: None,
            azure_speech_region: None,
            cartesia_api_key: None,
            openai_api_key: None,
            azure_openai_api_key: None,
            azure_openai_endpoint: None,
            grok_api_key: None,
            inworld_api_key: None,
            gemini_api_key: None,
            ultravox_api_key: None,
            speechmatics_api_key: None,
            yandex_api_key: None,
            yandex_folder_id: None,
            assemblyai_api_key: None,
            hume_api_key: None,
            groq_api_key: None,
            ibm_watson_api_key: None,
            ibm_watson_instance_id: None,
            ibm_watson_region: None,
            aws_access_key_id: None,
            aws_secret_access_key: None,
            aws_region: None,
            gnani_token: None,
            gnani_access_key: None,
            gnani_certificate_path: None,
            recording_s3_bucket: None,
            recording_s3_region: None,
            recording_s3_endpoint: None,
            recording_s3_access_key: None,
            recording_s3_secret_key: None,
            recording_s3_prefix: None,
            cache_path: None,
            cache_ttl_seconds: Some(3600),
            auth_service_url: None,
            auth_signing_key_path: None,
            auth_api_secrets: Vec::new(),
            auth_timeout_seconds: 5,
            auth_required: false,
            sip: None,
            cors_allowed_origins: None,
            rate_limit_requests_per_second: 60,
            rate_limit_burst_size: 10,
            max_websocket_connections: None,
            max_connections_per_ip: 100,
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            realtime_endpoint_overrides: Default::default(),
            aliases: Default::default(),
            plugins: PluginConfig::default(),
            dag_timeouts: DAGTimeoutsConfig::default(),
        }
    }

    #[tokio::test]
    async fn no_providers_configured_is_ready() {
        let cfg = base_config();
        let report = evaluate(&cfg, true).await;
        assert!(report.providers.is_empty());
        assert!(report.is_ready(), "no enabled providers => ready");
        assert_eq!(report.status, "ready");
    }

    #[tokio::test]
    async fn credential_only_check_is_ready_without_network() {
        let mut cfg = base_config();
        cfg.deepgram_api_key = Some("dg-key".to_string());
        let report = evaluate(&cfg, false).await;
        assert_eq!(report.providers.len(), 1);
        assert_eq!(report.providers[0].provider, "deepgram");
        assert!(report.providers[0].credential_present);
        assert_eq!(report.providers[0].reachable, None);
        assert!(report.is_ready());
    }

    #[tokio::test]
    async fn unreachable_provider_makes_not_ready() {
        // Point deepgram's probe at a closed local port via the override.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // close it so the connect refuses
        // SAFETY: single-threaded test scope; restored below.
        unsafe { std::env::set_var("WAAV_READINESS_PROBE_DEEPGRAM", format!("127.0.0.1:{port}")) };

        let mut cfg = base_config();
        cfg.deepgram_api_key = Some("dg-key".to_string());
        let report = evaluate(&cfg, true).await;

        unsafe { std::env::remove_var("WAAV_READINESS_PROBE_DEEPGRAM") };

        assert_eq!(report.providers.len(), 1);
        assert_eq!(report.providers[0].reachable, Some(false));
        assert!(!report.providers[0].ready);
        assert!(!report.is_ready(), "unreachable enabled provider => 503");
        assert_eq!(report.status, "not_ready");
    }

    #[test]
    fn the_openai_probe_follows_openai_base_url() {
        assert_eq!(
            openai_probe_target(Some("http://127.0.0.1:8080")),
            ("127.0.0.1".to_string(), 8080)
        );
        assert_eq!(
            openai_probe_target(Some("https://llm.lan/v1")),
            ("llm.lan".to_string(), 443)
        );
        assert_eq!(
            openai_probe_target(Some("http://[::1]:9000/v1")),
            ("::1".to_string(), 9000)
        );
        for unset in [None, Some(""), Some("  "), Some("not a url")] {
            assert_eq!(
                openai_probe_target(unset),
                ("api.openai.com".to_string(), 443),
                "{unset:?}"
            );
        }
    }

    /// A reachable local server answers ready, and the probe dialled it (not api.openai.com).
    #[tokio::test]
    async fn a_reachable_override_is_ready() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // SAFETY: test-only env, this provider's override only; restored below.
        unsafe { std::env::set_var("WAAV_READINESS_PROBE_CARTESIA", format!("127.0.0.1:{port}")) };
        let mut cfg = base_config();
        cfg.cartesia_api_key = Some("ct-key".to_string());
        let report = evaluate(&cfg, true).await;
        unsafe { std::env::remove_var("WAAV_READINESS_PROBE_CARTESIA") };
        assert_eq!(report.providers[0].reachable, Some(true));
        assert!(report.is_ready());
    }

    /// Within the window a second request reuses the evaluation instead of probing again.
    #[tokio::test]
    async fn a_burst_of_requests_costs_one_evaluation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // SAFETY: test-only env, this provider's override only; restored below.
        unsafe { std::env::set_var("WAAV_READINESS_PROBE_HUME", format!("127.0.0.1:{port}")) };
        let mut cfg = base_config();
        cfg.hume_api_key = Some("hm-key".to_string());
        let reports = futures::future::join_all((0..20).map(|_| evaluate_cached(&cfg, true))).await;
        drop(listener);
        // The listener is gone; a fresh probe would now fail, the cached result does not.
        let after = evaluate_cached(&cfg, true).await;
        unsafe { std::env::remove_var("WAAV_READINESS_PROBE_HUME") };
        assert!(reports.iter().all(|r| r == &reports[0]));
        assert_eq!(after, reports[0]);
    }
}
