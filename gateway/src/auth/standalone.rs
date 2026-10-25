//! Standalone authentication (no Bud control plane) of one bearer, without its request body.
//!
//! The same authenticators, in the same order, as `middleware::auth` (an API secret, then the
//! external auth responder; see [`super::AuthPolicy`]). The middleware uses it for a request whose
//! body is not JSON; `/v1/realtime`, which authenticates its own upgrade, uses it for the key a
//! client sent there.

use axum::http::HeaderMap;

use super::{Auth, match_api_secret_id};
use crate::errors::auth_error::AuthError;
use crate::state::AppState;

/// The secret the bearer is, when API secrets are active.
pub fn secret_auth(state: &AppState, token: &str) -> Option<Auth> {
    if !state.auth_policy.secrets_active(&state.config) {
        return None;
    }
    match_api_secret_id(token, &state.config.auth_api_secrets).map(Auth::new)
}

/// Authenticate `token` for `method path`. The responder judges it by the request's method, path
/// and headers, with `request_body: {}`.
pub async fn authenticate_bearer(
    state: &AppState,
    token: &str,
    headers: &HeaderMap,
    path: &str,
    method: &str,
) -> Result<Auth, AuthError> {
    if !state.config.auth_required {
        return Ok(Auth::empty());
    }
    let try_responder = state.auth_policy.responder_active(&state.config);
    if !try_responder && !state.auth_policy.secrets_active(&state.config) {
        return Err(AuthError::ConfigError(
            "Authentication required but no auth method configured".to_string(),
        ));
    }
    if let Some(auth) = secret_auth(state, token) {
        tracing::info!(%method, %path, auth_id = ?auth.id, "API secret authentication successful");
        return Ok(auth);
    }
    if !try_responder {
        tracing::warn!(%method, %path, "API secret authentication failed: token mismatch");
        return Err(AuthError::Unauthorized("Invalid API secret".to_string()));
    }
    let client = state
        .auth_client
        .as_ref()
        .ok_or_else(|| AuthError::ConfigError("Auth client not initialized".to_string()))?;
    let auth = client
        .validate_token(
            token,
            &serde_json::json!({}),
            super::jwt::filter_headers(headers),
            path,
            method,
        )
        .await
        .inspect_err(|e| {
            tracing::warn!(%method, %path, error = %e, "JWT authentication failed");
        })?;
    tracing::info!(%method, %path, auth_id = ?auth.id, "JWT authentication successful");
    Ok(auth)
}
