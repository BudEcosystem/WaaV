use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};

use crate::core::readiness::{self, ReadinessReport};
use crate::state::AppState;

/// Health/liveness response.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HealthResponse {
    /// Process status. `"ok"` when the process is up and serving.
    #[cfg_attr(feature = "openapi", schema(example = "ok"))]
    pub status: String,
}

/// Liveness handler (`/` and `/livez`).
///
/// Answers only "is the process up and able to serve HTTP?". It performs NO upstream/provider
/// checks so an orchestrator never restarts a healthy pod just because a provider is degraded —
/// that signal belongs to `/readyz`. Returns `{"status":"ok"}`.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/livez",
        responses(
            (status = 200, description = "Process is live", body = HealthResponse)
        ),
        tag = "health"
    )
)]
pub async fn livez() -> Result<Json<HealthResponse>, StatusCode> {
    Ok(Json(HealthResponse {
        status: "ok".to_string(),
    }))
}

/// Back-compat health handler kept at `/`.
///
/// Identical to [`livez`]; both report `{"status":"ok"}` so existing probes (and the CI smoke
/// check) keep working after the liveness/readiness split.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/",
        responses(
            (status = 200, description = "Server is healthy", body = HealthResponse)
        ),
        tag = "health"
    )
)]
pub async fn health_check() -> Result<Json<HealthResponse>, StatusCode> {
    livez().await
}

/// Readiness handler (`/readyz`).
///
/// Answers "should this instance receive traffic right now?". Evaluates config + each enabled
/// provider's credential presence + a cached TCP reachability probe. Returns `200` with the
/// per-provider breakdown when ready, or `503` with the same breakdown (naming the unready
/// providers) when not. This is the E13 contract: `/livez` != `/readyz`, and `/readyz` returns
/// 503 when an enabled provider is unreachable.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/readyz",
        responses(
            (status = 200, description = "Instance is ready to serve", body = ReadinessReport),
            (status = 503, description = "Instance is not ready", body = ReadinessReport)
        ),
        tag = "health"
    )
)]
pub async fn readyz(State(state): State<Arc<AppState>>) -> Response {
    // Reachability probes are blocking (TCP connect with a short timeout); run them off the
    // async runtime worker. Network probing is enabled by default and can be disabled with
    // WAAV_READINESS_SKIP_NETWORK=1 (credential-only readiness).
    let skip_network = std::env::var("WAAV_READINESS_SKIP_NETWORK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let config = state.config.clone();

    let report = tokio::task::spawn_blocking(move || readiness::evaluate(&config, !skip_network))
        .await
        .unwrap_or_else(|_| ReadinessReport {
            status: "not_ready".to_string(),
            config_loaded: true,
            providers: Vec::new(),
        });

    let code = if report.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (code, Json(report)).into_response()
}

/// Prometheus metrics handler (`/metrics`).
///
/// Public (no auth, like the health endpoints) so a scraper can read it. Renders the
/// `metrics-exporter-prometheus` text exposition: `waav_provider_requests_total`,
/// `waav_provider_ttfb_ms`, `waav_provider_errors_total`, `waav_circuit_breaker_state`.
#[cfg_attr(
    feature = "openapi",
    utoipa::path(
        get,
        path = "/metrics",
        responses(
            (status = 200, description = "Prometheus text exposition", content_type = "text/plain")
        ),
        tag = "observability"
    )
)]
pub async fn metrics_handler(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    // RC8 /metrics guard:
    // 1. Optional bearer auth: when WAAV_METRICS_TOKEN is set (non-empty), the request
    //    must carry `Authorization: Bearer <token>` or it gets a 401. Unset/empty env
    //    keeps the endpoint public (back-compat with existing scrape configs).
    // 2. Scrape-storm cache: the rendered exposition is cached for METRICS_CACHE_TTL
    //    (1 s) so a storm of scrapes re-renders at most once per window.
    let required_token = std::env::var("WAAV_METRICS_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    metrics_response(required_token.as_deref(), authorization, || {
        cached_metrics_body(&METRICS_CACHE, METRICS_CACHE_TTL, || {
            state.core_state.metrics.render()
        })
    })
}

/// How long a rendered Prometheus exposition stays fresh (RC8 scrape-storm guard).
const METRICS_CACHE_TTL: Duration = Duration::from_secs(1);

/// Process-wide exposition cache: `(rendered_at, body)`.
///
/// Uses `Instant` (monotonic) — never `SystemTime` — for the freshness check, so
/// wall-clock skew/steps cannot extend or invalidate the window.
static METRICS_CACHE: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// Return the cached exposition if it is younger than `ttl`, otherwise render a
/// fresh one (under the lock, so a scrape storm renders exactly once per window
/// while concurrent scrapers wait briefly and reuse the fresh body).
fn cached_metrics_body(
    cache: &Mutex<Option<(Instant, String)>>,
    ttl: Duration,
    render: impl FnOnce() -> String,
) -> String {
    let mut guard = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((rendered_at, body)) = guard.as_ref()
        && rendered_at.elapsed() < ttl
    {
        return body.clone();
    }
    let body = render();
    *guard = Some((Instant::now(), body.clone()));
    body
}

/// Constant-time string equality (no early exit on the first differing byte),
/// so the bearer-token check does not leak match-prefix length via timing.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Decide whether a `/metrics` request passes the optional bearer gate.
///
/// `required_token = None` means no token is configured → always allowed.
/// Otherwise the `Authorization` header must be exactly `Bearer <token>`.
fn metrics_auth_ok(required_token: Option<&str>, authorization: Option<&str>) -> bool {
    match required_token {
        None => true,
        Some(token) => authorization
            .and_then(|header| header.strip_prefix("Bearer "))
            .is_some_and(|presented| constant_time_eq(presented, token)),
    }
}

/// Build the `/metrics` response: 401 (without invoking `body`) when the bearer
/// gate fails, otherwise 200 with the text exposition. Split from the axum
/// handler so the status-code contract is unit-testable without env mutation.
fn metrics_response(
    required_token: Option<&str>,
    authorization: Option<&str>,
    body: impl FnOnce() -> String,
) -> Response {
    if !metrics_auth_ok(required_token, authorization) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "unauthorized\n",
        )
            .into_response();
    }
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ============== /metrics exposition cache (RC8) ==============

    #[test]
    fn metrics_cache_returns_same_body_within_window() {
        let cache = Mutex::new(None);
        let renders = AtomicUsize::new(0);

        let first = cached_metrics_body(&cache, Duration::from_secs(60), || {
            renders.fetch_add(1, Ordering::SeqCst);
            "exposition-1".to_string()
        });
        // Second scrape inside the window: must reuse the cached body and must
        // NOT invoke the (different) render closure.
        let second = cached_metrics_body(&cache, Duration::from_secs(60), || {
            renders.fetch_add(1, Ordering::SeqCst);
            "exposition-2".to_string()
        });

        assert_eq!(first, "exposition-1");
        assert_eq!(second, first, "cached body must be served within the TTL");
        assert_eq!(
            renders.load(Ordering::SeqCst),
            1,
            "render must run exactly once within the cache window"
        );
    }

    #[test]
    fn metrics_cache_rerenders_after_window() {
        let cache = Mutex::new(None);

        let first = cached_metrics_body(&cache, Duration::from_millis(10), || "old".to_string());
        // Let the (tiny, test-local) TTL lapse; Instant is monotonic so this
        // cannot be perturbed by wall-clock changes.
        std::thread::sleep(Duration::from_millis(30));
        let second = cached_metrics_body(&cache, Duration::from_millis(10), || "new".to_string());

        assert_eq!(first, "old");
        assert_eq!(second, "new", "an expired cache entry must be re-rendered");
    }

    // ============== /metrics bearer gate (RC8) ==============

    #[test]
    fn metrics_auth_open_when_no_token_configured() {
        assert!(metrics_auth_ok(None, None));
        assert!(metrics_auth_ok(None, Some("Bearer anything")));
    }

    #[test]
    fn metrics_auth_rejects_missing_wrong_or_malformed_credentials() {
        assert!(!metrics_auth_ok(Some("s3cret"), None));
        assert!(!metrics_auth_ok(Some("s3cret"), Some("Bearer wrong")));
        assert!(!metrics_auth_ok(Some("s3cret"), Some("Bearer s3cret-longer")));
        assert!(!metrics_auth_ok(Some("s3cret"), Some("Basic s3cret")));
        assert!(!metrics_auth_ok(Some("s3cret"), Some("s3cret")));
    }

    #[test]
    fn metrics_auth_accepts_correct_bearer_token() {
        assert!(metrics_auth_ok(Some("s3cret"), Some("Bearer s3cret")));
    }

    #[test]
    fn metrics_response_token_gate_401() {
        let response = metrics_response(Some("tok"), Some("Bearer wrong"), || {
            panic!("exposition must not be rendered for unauthorized scrapes")
        });
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get(header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok()),
            Some("Bearer")
        );
    }

    #[test]
    fn metrics_response_token_gate_200_with_valid_token() {
        let response = metrics_response(Some("tok"), Some("Bearer tok"), || "body".to_string());
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
    }

    #[test]
    fn metrics_response_200_when_gate_disabled() {
        let response = metrics_response(None, None, || "body".to_string());
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[test]
    fn constant_time_eq_basic_contract() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "ab"));
        assert!(constant_time_eq("", ""));
    }
}
