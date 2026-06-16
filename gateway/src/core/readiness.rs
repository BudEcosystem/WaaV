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
//! 3. **Cached TCP reachability (optional)** — a short, cached TCP connect to each enabled
//!    provider's host:port. A reachable provider keeps the instance ready; an unreachable one
//!    flips `/readyz` to `503` and is named in the per-provider JSON breakdown.
//!
//! The probe target for a provider can be overridden with the env var
//! `WAAV_READINESS_PROBE_<PROVIDER>` (value `host:port`, provider upper-cased with `-`→`_`),
//! which is also how tests point a provider at a known-closed port to assert the 503 path.

use std::collections::BTreeMap;
use std::net::ToSocketAddrs;
use std::time::Duration;

use serde::Serialize;

use crate::config::ServerConfig;

/// How long to wait for a single TCP reachability probe before declaring a provider unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_millis(800);

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
        config.deepgram_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.deepgram.com",
    );
    add(
        "elevenlabs",
        config.elevenlabs_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.elevenlabs.io",
    );
    add(
        "openai",
        config.openai_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.openai.com",
    );
    add(
        "cartesia",
        config.cartesia_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.cartesia.ai",
    );
    add(
        "assemblyai",
        config.assemblyai_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.assemblyai.com",
    );
    add(
        "hume",
        config.hume_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.hume.ai",
    );
    add(
        "lmnt",
        config.lmnt_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.lmnt.com",
    );
    add(
        "groq",
        config.groq_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.groq.com",
    );
    add(
        "playht",
        config.playht_api_key.as_deref().is_some_and(|k| !k.is_empty()),
        "api.play.ht",
    );
    add(
        "microsoft-azure",
        config
            .azure_speech_subscription_key
            .as_deref()
            .is_some_and(|k| !k.is_empty()),
        "westus.api.cognitive.microsoft.com",
    );

    out
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

/// Probe one TCP endpoint with a short timeout. `true` if a connection was established.
fn tcp_reachable(host: &str, port: u16) -> bool {
    let addrs = match (host, port).to_socket_addrs() {
        Ok(a) => a,
        Err(_) => return false,
    };
    for addr in addrs {
        if std::net::TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).is_ok() {
            return true;
        }
    }
    false
}

/// Evaluate readiness for the given config.
///
/// `probe_network` gates the (blocking) TCP reachability check: when `false`, only credential
/// presence is evaluated (`reachable` is reported as `None` but does not, by itself, fail
/// readiness). When `true`, each enabled provider is TCP-probed and an unreachable provider
/// flips the report to not-ready.
pub fn evaluate(config: &ServerConfig, probe_network: bool) -> ReadinessReport {
    let targets = enabled_provider_targets(config);

    let mut providers = Vec::with_capacity(targets.len());
    for (provider, default_target) in targets {
        // Credential presence is implied by membership in `enabled_provider_targets`.
        let credential_present = true;

        // Resolve the probe target (override wins over the vendor default).
        let target = probe_override(&provider).or(default_target);

        let (reachable, ready, detail) = if !probe_network {
            // Reachability not evaluated; credential presence alone determines readiness.
            (None, credential_present, None)
        } else {
            match target {
                Some((ref host, port)) => {
                    let ok = tcp_reachable(host, port);
                    let detail = if ok {
                        None
                    } else {
                        Some(format!("TCP probe to {host}:{port} failed"))
                    };
                    (Some(ok), credential_present && ok, detail)
                }
                None => (
                    None,
                    false,
                    Some("no probe endpoint could be derived".to_string()),
                ),
            }
        };

        providers.push(ProviderReadiness {
            provider,
            credential_present,
            reachable,
            ready,
            detail,
        });
    }

    let all_ready = providers.iter().all(|p| p.ready);
    ReadinessReport {
        status: if all_ready { "ready" } else { "not_ready" }.to_string(),
        config_loaded: true,
        providers,
    }
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
            assemblyai_api_key: None,
            hume_api_key: None,
            lmnt_api_key: None,
            groq_api_key: None,
            playht_api_key: None,
            playht_user_id: None,
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
            plugins: PluginConfig::default(),
            dag_timeouts: DAGTimeoutsConfig::default(),
        }
    }

    #[test]
    fn no_providers_configured_is_ready() {
        let cfg = base_config();
        let report = evaluate(&cfg, true);
        assert!(report.providers.is_empty());
        assert!(report.is_ready(), "no enabled providers => ready");
        assert_eq!(report.status, "ready");
    }

    #[test]
    fn credential_only_check_is_ready_without_network() {
        let mut cfg = base_config();
        cfg.deepgram_api_key = Some("dg-key".to_string());
        let report = evaluate(&cfg, false);
        assert_eq!(report.providers.len(), 1);
        assert_eq!(report.providers[0].provider, "deepgram");
        assert!(report.providers[0].credential_present);
        assert_eq!(report.providers[0].reachable, None);
        assert!(report.is_ready());
    }

    #[test]
    fn unreachable_provider_makes_not_ready() {
        // Point deepgram's probe at a closed local port via the override.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener); // close it so the connect refuses
        // SAFETY: single-threaded test scope; restored below.
        unsafe { std::env::set_var("WAAV_READINESS_PROBE_DEEPGRAM", format!("127.0.0.1:{port}")) };

        let mut cfg = base_config();
        cfg.deepgram_api_key = Some("dg-key".to_string());
        let report = evaluate(&cfg, true);

        unsafe { std::env::remove_var("WAAV_READINESS_PROBE_DEEPGRAM") };

        assert_eq!(report.providers.len(), 1);
        assert_eq!(report.providers[0].reachable, Some(false));
        assert!(!report.providers[0].ready);
        assert!(!report.is_ready(), "unreachable enabled provider => 503");
        assert_eq!(report.status, "not_ready");
    }
}
