use std::sync::Arc;

use axum::{
    extract::State,
    http::{StatusCode, header},
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
pub async fn metrics_handler(State(state): State<Arc<AppState>>) -> Response {
    let body = state.core_state.metrics.render();
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}
