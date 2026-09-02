use axum::{http::StatusCode, response::Json};
use serde::{Deserialize, Serialize};

/// Health check response
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HealthResponse {
    /// Server status
    #[cfg_attr(feature = "openapi", schema(example = "OK"))]
    pub status: String,
}

/// Health check handler
/// Returns a simple JSON response indicating the server is running
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
    Ok(Json(HealthResponse {
        status: "OK".to_string(),
    }))
}

/// Readiness: can this pod actually serve?
///
/// Distinct from liveness on purpose. In Bud mode the auth snapshot is populated by a boot
/// sweep, and until that completes every valid credential resolves to 401. A pod in that state
/// is running but useless, and reporting it ready makes a rollout look healthy while it rejects
/// all traffic.
///
/// In standalone mode there is no snapshot to wait for, so readiness follows liveness.
pub async fn readiness_check(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::state::AppState>>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    match &state.bud_mode {
        Some(bud) if !bud.is_ready() => (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({
                "status": "not_ready",
                "reason": "awaiting first control-plane hydration",
            })),
        )
            .into_response(),
        _ => (
            axum::http::StatusCode::OK,
            axum::Json(serde_json::json!({ "status": "ready" })),
        )
            .into_response(),
    }
}
