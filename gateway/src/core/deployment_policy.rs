//! FRD-022 — a Bud voice deployment's **Rate limiting** and **Resilience** settings on the audio
//! plane: rate limits and a concurrency cap (the local-first limiter shared with budgateway, via
//! `resil`), the retry policy, the fallback chain and the two-tier circuit breaker.
//!
//! Admission is a local decision in the common case (no Redis round-trip); limits hold across
//! every WaaV replica because each replica spends credit reserved in Redis. Policies come from
//! `voice_table` through the Bud auth plane and follow its snapshot live: every request compares
//! the plane's generation counter (one atomic load) and resyncs only when it moved.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use resil::RateHeaders;
use resil::breaker::TwoTier;
use resil::classify::{self, Failure, Verdict};
use resil::limit::redis::RedisStore;
use resil::limit::{ConcurrencyDenied, ConcurrencyGuard, Decision, Limiter, LimiterOptions};

/// Replica-set name in Redis (`rl2:{waav…}`) and the `svc` metric label.
pub const SERVICE: &str = "waav";

/// Response headers of a request served by the fallback chain (FRD-022 §6.4).
pub const SERVED_ENDPOINT_HEADER: HeaderName = HeaderName::from_static("x-bud-endpoint-id");
pub const FALLBACK_HEADER: HeaderName = HeaderName::from_static("x-bud-fallback");
pub const VOICE_SUBSTITUTED_HEADER: HeaderName = HeaderName::from_static("x-bud-voice-substituted");

/// The deadline for a whole speech request (retries and fallback hops) when the deployment sets
/// no `request_timeout` (FRD-022 §6.3).
pub const DEFAULT_SPEECH_DEADLINE: Duration = Duration::from_secs(30);
/// The same for transcription, whose uploads can be long.
pub const DEFAULT_TRANSCRIPTION_DEADLINE: Duration = Duration::from_secs(120);

/// Everything WaaV enforces from a deployment's policy.
pub struct DeploymentPolicies {
    limiter: Limiter,
    breakers: TwoTier,
    /// The auth-plane generation the limiter last mirrored; `u64::MAX` = never.
    synced_generation: AtomicU64,
    sync_lock: Mutex<()>,
}

impl std::fmt::Debug for DeploymentPolicies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeploymentPolicies")
            .field("pod", &self.limiter.pod_id())
            .field("policies", &self.limiter.len())
            .finish()
    }
}

impl DeploymentPolicies {
    /// Limits shared across replicas through the Bud Redis (`WAAV_REDIS_URL`, db `WAAV_REDIS_DB`).
    ///
    /// A Redis that cannot be reached at boot is not fatal here — the auth plane has already
    /// hydrated from the same Redis, so this is a transient — the limiter starts and its sync task
    /// keeps retrying, deciding locally (fail-static at `⌈L/N⌉`) until Redis answers.
    pub async fn connect(redis_url: &str, db: u8) -> Result<Arc<Self>, String> {
        use redis::IntoConnectionInfo;
        let mut info = redis_url
            .into_connection_info()
            .map_err(|e| format!("invalid WAAV_REDIS_URL for rate limiting: {e}"))?;
        if db != 0 {
            info.redis.db = i64::from(db);
        }
        let client = redis::Client::open(info)
            .map_err(|e| format!("invalid Redis config for rate limiting: {e}"))?;
        let conn = tokio::time::timeout(
            Duration::from_secs(5),
            redis::aio::ConnectionManager::new(client),
        )
        .await
        .map_err(|_| "timed out connecting to Redis for rate limiting".to_string())?
        .map_err(|e| format!("could not connect to Redis for rate limiting: {e}"))?;
        let limiter = Limiter::new(
            limiter_options_from_env(|k| std::env::var(k).ok()),
            Some(Arc::new(RedisStore::new(conn))),
        );
        limiter.start().await;
        Ok(Arc::new(Self::with_limiter(limiter)))
    }

    /// Per-replica limits only (tests, and a WaaV without Redis).
    pub fn local() -> Arc<Self> {
        Arc::new(Self::with_limiter(Limiter::new(
            LimiterOptions {
                service: SERVICE.into(),
                ..Default::default()
            },
            None,
        )))
    }

    fn with_limiter(limiter: Limiter) -> Self {
        Self {
            limiter,
            breakers: TwoTier::default(),
            synced_generation: AtomicU64::new(u64::MAX),
            sync_lock: Mutex::new(()),
        }
    }

    pub fn limiter(&self) -> &Limiter {
        &self.limiter
    }

    pub fn breakers(&self) -> &TwoTier {
        &self.breakers
    }

    /// Mirror every voice endpoint's rate limits and concurrency cap into the limiter, if the
    /// auth snapshot changed since the last time. Cheap when nothing changed (one atomic load).
    pub fn sync(&self, auth: &bud_auth::BudAuth) {
        let generation = auth.generation();
        if self.synced_generation.load(Ordering::Acquire) == generation {
            return;
        }
        let _guard = self.sync_lock.lock().unwrap_or_else(|e| e.into_inner());
        let generation = auth.generation();
        if self.synced_generation.load(Ordering::Acquire) == generation {
            return;
        }
        let endpoints = auth.voice_endpoints();
        self.limiter.sync_policies(endpoints.iter().map(|(id, ep)| {
            (
                &**id,
                ep.policy.rate_limits.as_ref(),
                ep.policy.max_concurrent,
            )
        }));
        self.synced_generation.store(generation, Ordering::Release);
    }

    /// Admit one request to `endpoint_id`: a hit against its rate limits and, when it has a
    /// `max_concurrent`, a slot held until the returned [`Admission`] drops.
    pub async fn admit(&self, endpoint_id: &str) -> Result<Admission, Rejection> {
        let headers = match self.limiter.check(endpoint_id).await {
            Decision::Unlimited => None,
            Decision::Allow(h) => Some(h),
            Decision::Deny(h) => return Err(Rejection::Rate(h)),
        };
        let slot = self
            .limiter
            .acquire(endpoint_id)
            .await
            .map_err(Rejection::Concurrency)?;
        Ok(Admission {
            headers,
            _slot: slot,
        })
    }

    /// Push outstanding hits and hand this replica's reservations back (graceful shutdown).
    pub async fn shutdown(&self) {
        self.limiter.shutdown().await;
    }
}

/// The limiter's per-service knobs (FRD-022 §5.5, §5.7):
/// `WAAV_RATE_LIMIT_LAST_MILE` = `sync` (default) | `local`, and
/// `WAAV_RATE_LIMIT_ON_STORE_UNAVAILABLE` = `local_share` (default) | `allow`.
pub fn limiter_options_from_env(get: impl Fn(&str) -> Option<String>) -> LimiterOptions {
    use resil::limit::{LastMile, OnStoreUnavailable};
    let last_mile = match get("WAAV_RATE_LIMIT_LAST_MILE").as_deref().map(str::trim) {
        Some("local") => LastMile::Local,
        None | Some("") | Some("sync") => LastMile::Sync,
        Some(other) => {
            tracing::warn!("unknown WAAV_RATE_LIMIT_LAST_MILE {other:?}; using \"sync\"");
            LastMile::Sync
        }
    };
    let on_store_unavailable = match get("WAAV_RATE_LIMIT_ON_STORE_UNAVAILABLE")
        .as_deref()
        .map(str::trim)
    {
        Some("allow") => OnStoreUnavailable::Allow,
        None | Some("") | Some("local_share") => OnStoreUnavailable::LocalShare,
        Some(other) => {
            tracing::warn!(
                "unknown WAAV_RATE_LIMIT_ON_STORE_UNAVAILABLE {other:?}; using \"local_share\""
            );
            OnStoreUnavailable::LocalShare
        }
    };
    LimiterOptions {
        service: SERVICE.into(),
        last_mile,
        on_store_unavailable,
        ..Default::default()
    }
}

/// An admitted request. Holding it holds the deployment's concurrency slot.
#[derive(Debug, Default)]
pub struct Admission {
    pub headers: Option<RateHeaders>,
    _slot: Option<ConcurrencyGuard>,
}

impl Admission {
    /// No policy applies (standalone WaaV, or a deployment without limits).
    pub fn none() -> Self {
        Self::default()
    }

    /// Add the `X-RateLimit-*` headers of the admitting deployment.
    pub fn apply(&self, headers: &mut HeaderMap) {
        if let Some(h) = &self.headers {
            resil::http::write_headers(h, headers);
        }
    }
}

/// A request the deployment's own limits refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    Rate(RateHeaders),
    Concurrency(ConcurrencyDenied),
}

impl Rejection {
    pub fn retry_after(&self) -> Duration {
        Duration::from_secs(match self {
            Rejection::Rate(h) => h.retry_after.unwrap_or(1),
            Rejection::Concurrency(c) => c.retry_after,
        })
    }
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        let (status, headers, body) = match self {
            Rejection::Rate(h) => resil::http::rate_limited(&h),
            Rejection::Concurrency(c) => resil::http::concurrency_limited(c.retry_after),
        };
        (status, headers, body).into_response()
    }
}

/// The vendor-tier breaker key: vendor plus API host (FRD-022 §6.5). Two deployments on the same
/// vendor but different base URLs (a self-hosted model, a regional endpoint) are different
/// failure domains.
pub fn vendor_key(vendor: &str, api_base: Option<&str>) -> String {
    let host = api_base
        .and_then(|b| url::Url::parse(b).ok())
        .and_then(|u| {
            u.host_str().map(|h| match u.port() {
                Some(port) => format!("{h}:{port}"),
                None => h.to_owned(),
            })
        })
        .unwrap_or_default();
    if host.is_empty() {
        vendor.to_ascii_lowercase()
    } else {
        format!("{}@{host}", vendor.to_ascii_lowercase())
    }
}

/// The HTTP status a vendor error message carries.
///
/// WaaV's vendor paths render a refusal as a sentence with the status inside — `"deepgram API
/// error (429 Too Many Requests): …"`, `"elevenlabs rejected the request (400 Bad Request): …"`,
/// `"self-hosted returned 503 Service Unavailable: …"`. Only a three-digit code opening a status
/// line (`(` or `returned ` before it, a space and a capitalised reason after) counts, so numbers
/// elsewhere in a message (a sample rate, a byte count) are never read as a status.
pub fn status_in(message: &str) -> Option<u16> {
    let b = message.as_bytes();
    let mut i = 0;
    while i + 4 < b.len() {
        let opens = (i > 0 && b[i - 1] == b'(')
            || (i >= 9 && &message[i.saturating_sub(9)..i] == "returned ");
        if opens
            && b[i].is_ascii_digit()
            && b[i + 1].is_ascii_digit()
            && b[i + 2].is_ascii_digit()
            && b[i + 3] == b' '
            && b[i + 4].is_ascii_uppercase()
        {
            let code: u16 = message[i..i + 3].parse().ok()?;
            if (100..=599).contains(&code) {
                return Some(code);
            }
        }
        i += 1;
    }
    None
}

/// Classify a vendor failure known only by its message (and an optional delay hint).
pub fn classify_message(message: &str, retry_after: Option<Duration>) -> Verdict {
    let lower = message.to_ascii_lowercase();
    let failure = match status_in(message) {
        Some(code) => Failure::Status {
            code,
            headers: None,
            body: Some(message.as_bytes()),
        },
        None if lower.contains("timed out") || lower.contains("timeout") => Failure::Timeout,
        None => Failure::Connect,
    };
    let mut v = classify::classify(&failure);
    if retry_after.is_some() {
        v.retry_after = retry_after;
        if matches!(v.breaker, classify::BreakerSignal::Deployment) && !v.vendor_concurrency {
            if let Some(d) = retry_after {
                v.breaker = classify::BreakerSignal::OpenFor(d);
            }
        }
    }
    v
}

/// A breaker that refused a hop, as a response when there is nothing to fall back to.
pub fn breaker_open_response(open: resil::breaker::Open) -> Response {
    let secs = open.retry_in.as_secs().max(1);
    let mut headers = HeaderMap::new();
    headers.insert(header::RETRY_AFTER, HeaderValue::from(secs));
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    (
        StatusCode::SERVICE_UNAVAILABLE,
        headers,
        serde_json::json!({
            "error": {
                "message": format!(
                    "the deployment's vendor is failing and its circuit breaker is open; retry in {secs}s"
                ),
                "type": "api_error",
                "code": "circuit_open",
            }
        })
        .to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_status_out_of_vendor_messages() {
        assert_eq!(
            status_in("deepgram API error (429 Too Many Requests): slow down"),
            Some(429)
        );
        assert_eq!(
            status_in("elevenlabs rejected the request (400 Bad Request): voice_not_found"),
            Some(400)
        );
        assert_eq!(
            status_in("self-hosted returned 503 Service Unavailable: overloaded"),
            Some(503)
        );
        assert_eq!(
            status_in("synthesis error: API error (500 Internal Server Error): boom"),
            Some(500)
        );
        // numbers that are not a status line
        assert_eq!(
            status_in("sample rate 24000 is not supported (use 16000)"),
            None
        );
        assert_eq!(status_in("got 400 bytes"), None);
        assert_eq!(status_in(""), None);
    }

    #[test]
    fn classifies_vendor_messages() {
        let v = classify_message("x API error (503 Service Unavailable): down", None);
        assert!(v.retryable && v.failover);
        let v = classify_message("x rejected the request (400 Bad Request): bad", None);
        assert!(!v.retryable && !v.failover && v.caller_error);
        let v = classify_message("x API error (401 Unauthorized): key", None);
        assert!(!v.retryable && v.failover);
        let v = classify_message("connect failed: connection refused", None);
        assert!(v.retryable && v.failover);
        let v = classify_message("request timed out after 30s", None);
        assert!(v.retryable);
        let v = classify_message(
            "x API error (429 Too Many Requests): slow",
            Some(Duration::from_secs(20)),
        );
        assert_eq!(v.retry_after, Some(Duration::from_secs(20)));
        assert_eq!(
            v.breaker,
            classify::BreakerSignal::OpenFor(Duration::from_secs(20))
        );
        let v = classify_message(
            r#"x API error (429 Too Many Requests): {"detail":{"status":"too_many_concurrent_requests"}}"#,
            None,
        );
        assert!(v.vendor_concurrency);
        assert_eq!(v.breaker, classify::BreakerSignal::Ignore);
    }

    #[test]
    fn vendor_key_separates_hosts() {
        assert_eq!(vendor_key("ElevenLabs", None), "elevenlabs");
        assert_eq!(
            vendor_key("self_hosted", Some("http://tts-a.ns.svc:8000/v1")),
            "self_hosted@tts-a.ns.svc:8000"
        );
        assert_ne!(
            vendor_key("self_hosted", Some("http://tts-a:8000")),
            vendor_key("self_hosted", Some("http://tts-b:8000"))
        );
        // two servers on one host are two failure domains
        assert_ne!(
            vendor_key("self_hosted", Some("http://127.0.0.1:3401/v1")),
            vendor_key("self_hosted", Some("http://127.0.0.1:3402/v1"))
        );
    }

    #[test]
    fn limiter_knobs_come_from_env() {
        use resil::limit::{LastMile, OnStoreUnavailable};
        let o = limiter_options_from_env(|k| match k {
            "WAAV_RATE_LIMIT_LAST_MILE" => Some("local".into()),
            "WAAV_RATE_LIMIT_ON_STORE_UNAVAILABLE" => Some("allow".into()),
            _ => None,
        });
        assert_eq!(o.last_mile, LastMile::Local);
        assert_eq!(o.on_store_unavailable, OnStoreUnavailable::Allow);
        assert_eq!(o.service, SERVICE);
        let d = limiter_options_from_env(|_| None);
        assert_eq!(d.last_mile, LastMile::Sync);
        assert_eq!(d.on_store_unavailable, OnStoreUnavailable::LocalShare);
    }

    fn endpoint(policy: serde_json::Value) -> bud_auth::VoiceEndpoint {
        let mut entry = serde_json::json!({
            "vendor": "self_hosted", "api_base": "http://tts:8000", "endpoints": ["text_to_speech"]
        });
        for (k, v) in policy.as_object().unwrap() {
            entry[k] = v.clone();
        }
        let blob = serde_json::json!({ "ep": entry }).to_string();
        bud_auth::credentials::parse_voice_blob(&blob, &bud_auth::CredentialDecryptor::disabled())
            .unwrap()
            .remove("ep")
            .unwrap()
    }

    /// Policies follow the auth snapshot: a republished limit applies without a restart.
    #[tokio::test]
    async fn admission_follows_the_auth_snapshot() {
        let auth = bud_auth::BudAuth::new();
        let p = DeploymentPolicies::local();
        auth.mutate_voice(
            "ep-1",
            Some(Arc::new(endpoint(serde_json::json!({
                "rate_limits": {"algorithm": "fixed_window", "requests_per_minute": 2,
                                "local_allowance": 1.0}
            })))),
        );
        p.sync(&auth);
        assert!(p.admit("ep-1").await.is_ok());
        assert!(p.admit("ep-1").await.is_ok());
        let r = p.admit("ep-1").await.unwrap_err();
        assert!(matches!(r, Rejection::Rate(_)));
        let resp = r.into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().contains_key("retry-after"));

        // republished without limits
        auth.mutate_voice("ep-1", Some(Arc::new(endpoint(serde_json::json!({})))));
        p.sync(&auth);
        assert!(p.admit("ep-1").await.unwrap().headers.is_none());
    }

    /// TC-WR-10: `max_concurrent` rejects the request over the cap with 429 + Retry-After: 1,
    /// and a finished request frees its slot.
    #[tokio::test]
    async fn concurrency_cap_rejects_and_releases() {
        let auth = bud_auth::BudAuth::new();
        let p = DeploymentPolicies::local();
        auth.mutate_voice(
            "ep-1",
            Some(Arc::new(endpoint(serde_json::json!({
                "max_concurrent": 2,
                "rate_limits": {"enabled": true, "local_allowance": 1.0}
            })))),
        );
        p.sync(&auth);
        let a = p.admit("ep-1").await.unwrap();
        let _b = p.admit("ep-1").await.unwrap();
        let c = p.admit("ep-1").await.unwrap_err();
        assert!(matches!(c, Rejection::Concurrency(_)));
        let resp = c.into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()["retry-after"], "1");
        drop(a);
        assert!(p.admit("ep-1").await.is_ok());
    }

    /// TC-CT-01's other half: an endpoint without policy fields is unlimited, no headers.
    #[tokio::test]
    async fn no_policy_is_unlimited() {
        let auth = bud_auth::BudAuth::new();
        let p = DeploymentPolicies::local();
        auth.mutate_voice("ep-1", Some(Arc::new(endpoint(serde_json::json!({})))));
        p.sync(&auth);
        for _ in 0..50 {
            assert!(p.admit("ep-1").await.unwrap().headers.is_none());
        }
    }
}
