//! `POST /v1/realtime/client_secrets` — mint an `ek_bud_…` (FRD-023 §5.8, FR-EK-1, D-7).
//!
//! OpenAI's request and response shapes, so an OpenAI client that mints through its SDK works
//! against the gateway unchanged. WaaV writes nothing anywhere: the secret is sealed
//! (`auth::ephemeral`) and carries its own claims.
//!
//! The lifetime is capped by the PARENT's: `exp = min(iat + seconds, parent expiry)`. A Keycloak
//! access token lives about five minutes, so without the cap a JWT could mint a two-hour bearer
//! credential. A clamp is not an error — `expires_at` reports the capped value.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::auth::ephemeral::{self, Claims, Parent};
use crate::state::AppState;

use super::handshake::{self, HandshakeError, REALTIME_CAPABILITY};
use super::policy::{self, ClientOutcome, ClientRules};
use super::session::{CallerCheck, authenticate};

pub const DEFAULT_TTL_SECS: u64 = 600;
pub const MIN_TTL_SECS: u64 = 10;
pub const MAX_TTL_SECS: u64 = 7200;
/// A JWT with less than this left cannot mint (TC-EK-12).
const MIN_PARENT_REMAINING_SECS: u64 = 10;

fn bad_request(
    code: &'static str,
    message: impl Into<String>,
    param: &'static str,
) -> HandshakeError {
    HandshakeError::new(StatusCode::BAD_REQUEST, code, message).param(param)
}

/// Validate a mint request and seal the secret. Split from the handler for the tests.
pub async fn mint(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
    now: u64,
) -> Result<serde_json::Value, HandshakeError> {
    let Some(keys) = state.realtime.client_secret_keys.as_ref() else {
        return Err(HandshakeError::new(
            StatusCode::NOT_IMPLEMENTED,
            "client_secrets_not_configured",
            "Client secrets are not enabled on this gateway.",
        ));
    };

    let Some(credential) = handshake::extract_credential(headers)? else {
        return Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "missing_api_key",
            "Mint client secrets with a Bud API key or an access token in `Authorization: Bearer`.",
        ));
    };
    // No chaining: a client secret cannot mint another (TC-EK-04).
    if credential.is_client_secret() {
        return Err(HandshakeError::new(
            StatusCode::UNAUTHORIZED,
            "invalid_api_key",
            "A client secret cannot mint another client secret.",
        ));
    }
    let caller = authenticate(state, &credential).await?;

    let request: serde_json::Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_slice(body).map_err(|e| {
            bad_request(
                "invalid_json",
                format!("The body is not valid JSON: {e}"),
                "body",
            )
        })?
    };

    let expires_after = request.get("expires_after");
    if let Some(anchor) = expires_after.and_then(|e| e.get("anchor"))
        && anchor.as_str() != Some("created_at")
    {
        return Err(bad_request(
            "invalid_value",
            "expires_after.anchor must be \"created_at\".",
            "expires_after.anchor",
        ));
    }
    let seconds = match expires_after.and_then(|e| e.get("seconds")) {
        None => DEFAULT_TTL_SECS,
        Some(v) => v
            .as_u64()
            .filter(|s| (MIN_TTL_SECS..=MAX_TTL_SECS).contains(s))
            .ok_or_else(|| {
                bad_request(
                    "invalid_value",
                    format!("expires_after.seconds must be {MIN_TTL_SECS}..{MAX_TTL_SECS}."),
                    "expires_after.seconds",
                )
            })?,
    };

    let session = request
        .get("session")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let model = session
        .get("model")
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or_else(|| {
            bad_request(
                "model_required",
                "session.model is required.",
                "session.model",
            )
        })?
        .to_string();

    let resolved = state
        .resolve_voice_endpoint(&model, REALTIME_CAPABILITY, Some(credential.expose()))
        .ok_or_else(|| {
            HandshakeError::new(
                StatusCode::FORBIDDEN,
                "model_not_allowed",
                format!("Model '{model}' is not a realtime deployment this credential can reach."),
            )
            .param("session.model")
        })?;

    // Validate the rest of `session` against the deployment's policy (§5.5). Echoed, not bound
    // (DEG-3).
    let rules = ClientRules::from_settings(resolved.endpoint.config.realtime.as_ref());
    let probe = serde_json::json!({"type": "session.update", "session": session}).to_string();
    if let ClientOutcome::Refuse(r) = policy::client_event(&probe, &rules) {
        return Err(HandshakeError::new(
            StatusCode::BAD_REQUEST,
            "event_not_allowed",
            r.message,
        ));
    }

    // The parent's lifetime caps the secret's (TC-EK-12).
    let mut exp = now + seconds;
    if let Some(parent_exp) = caller.principal.expires_at {
        if parent_exp < now + MIN_PARENT_REMAINING_SECS {
            return Err(HandshakeError::new(
                StatusCode::UNAUTHORIZED,
                "credential_expiring",
                "The minting credential expires in under 10 seconds; refresh it first.",
            ));
        }
        exp = exp.min(parent_exp);
    }

    let parent = match &caller.check {
        CallerCheck::ApiKey { hashed, client_key } => Parent::ApiKey {
            h: hashed.clone(),
            ck: *client_key,
        },
        CallerCheck::Jwt { sub } => Parent::Jwt { sub: sub.clone() },
    };
    let claims = Claims {
        v: 1,
        iat: now,
        exp,
        ep: resolved.endpoint_id.clone(),
        alias: model.clone(),
        parent,
        pid: caller.principal.project_id.clone(),
        uid: caller.principal.user_id.clone(),
        akid: caller.principal.api_key_id.clone(),
    };
    let value = keys
        .seal(&claims)
        .map_err(|e| HandshakeError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", e))?;
    metrics::counter!("waav_realtime_client_secrets_minted_total").increment(1);
    Ok(serde_json::json!({
        "value": value,
        "expires_at": exp,
        "session": session,
    }))
}

/// The route handler.
pub async fn client_secrets_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    match mint(&state, &headers, &body, ephemeral::now_epoch()).await {
        Ok(v) => (StatusCode::OK, axum::Json(v)).into_response(),
        Err(e) => e.into_response(),
    }
}
