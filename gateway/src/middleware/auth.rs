use crate::auth::{Auth, filter_headers, match_api_secret_id};
use crate::errors::auth_error::AuthError;
use crate::state::AppState;
use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use http_body_util::BodyExt;
use std::sync::Arc;

/// Extract authentication token from request
///
/// Supports multiple token sources for browser/WebSocket compatibility:
/// 1. Authorization header: `Authorization: Bearer <token>` (preferred)
/// 2. Query parameter: `?token=<token>` (for WebSocket connections)
///
/// # Arguments
/// * `request` - The incoming HTTP request
///
/// # Returns
/// * `Result<String, AuthError>` - The extracted token or an error
fn extract_token(request: &Request) -> Result<String, AuthError> {
    // Try Authorization header first (preferred method)
    if let Some(auth_header) = request.headers().get("authorization") {
        let auth_str = auth_header
            .to_str()
            .map_err(|_| AuthError::InvalidAuthHeader)?;

        if let Some(token) = auth_str.strip_prefix("Bearer ") {
            tracing::debug!("Token extracted from Authorization header");
            return Ok(token.to_string());
        }
        return Err(AuthError::InvalidAuthHeader);
    }

    // Try query parameter (for WebSocket browser connections)
    if let Some(query) = request.uri().query() {
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            if key == "token" {
                tracing::debug!("Token extracted from query parameter");
                return Ok(value.to_string());
            }
        }
    }

    // No token found
    Err(AuthError::MissingAuthHeader)
}

/// Authentication middleware that validates bearer tokens
///
/// This middleware supports two authentication modes:
/// 1. **API Secret Mode**: Simple bearer token comparison against configured API secrets
/// 2. **JWT Mode**: External validation service with signed JWT requests
///
/// Token extraction priority (for browser/WebSocket compatibility):
/// 1. Authorization header: `Authorization: Bearer <token>`
/// 2. Query parameter: `?token=<token>` (for WebSocket connections where headers can't be set)
///
/// The middleware:
/// 1. Extracts the token from Authorization header or query parameter
/// 2. For API secret mode: compares token directly with configured API secrets
/// 3. For JWT mode: buffers body, filters headers, and validates with auth service
/// 4. Inserts an AuthContext into request extensions on successful validation
/// 5. Returns 401 if validation fails, or passes the request through if successful
///
/// # Arguments
/// * `state` - Application state containing the ServerConfig and optional AuthClient
/// * `request` - The incoming HTTP request
/// * `next` - The next middleware or handler in the chain
///
/// # Returns
/// * `Result<Response, AuthError>` - The response from the next handler or an auth error
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    // Skip authentication if auth is not required or not configured
    // Still insert an empty Auth to allow handlers that need Auth context to work
    if !state.config.auth_required {
        tracing::debug!("Authentication disabled, inserting empty Auth context");
        request.extensions_mut().insert(Auth::empty());
        return Ok(next.run(request).await);
    }

    // Extract request method and path for logging
    let request_method = request.method().to_string();
    let request_path = request.uri().path().to_string();

    tracing::debug!(
        method = %request_method,
        path = %request_path,
        "Starting authentication validation"
    );

    // Extract token from Authorization header or query parameter
    // Priority: Authorization header > query parameter (for WebSocket browser compatibility)
    let token = match extract_token(&request) {
        Ok(t) => t,
        Err(e) => {
            // For WebSocket connections, allow deferred auth (first-message auth)
            // This enables browser clients to authenticate after connection
            if request_path == "/ws" {
                tracing::info!(
                    path = %request_path,
                    "WebSocket connection without token, enabling first-message auth"
                );
                request.extensions_mut().insert(Auth::pending());
                return Ok(next.run(request).await);
            }
            // For non-WebSocket routes, require immediate authentication
            return Err(e);
        }
    };

    // Check authentication mode and validate accordingly
    // Priority: API secret mode first (simpler), then JWT mode
    if state.config.has_api_secret_auth() {
        // API Secret authentication mode - constant-time comparison
        if let Some(secret_id) = match_api_secret_id(&token, &state.config.auth_api_secrets) {
            tracing::info!(
                method = %request_method,
                path = %request_path,
                auth_id = %secret_id,
                "API secret authentication successful"
            );
            request.extensions_mut().insert(Auth::new(secret_id));
            return Ok(next.run(request).await);
        } else {
            tracing::warn!(
                method = %request_method,
                path = %request_path,
                "API secret authentication failed: token mismatch"
            );
            return Err(AuthError::Unauthorized("Invalid API secret".to_string()));
        }
    }

    // JWT authentication mode - validate with external auth service
    if state.config.has_jwt_auth() {
        // Get the auth client from state
        let auth_client = state
            .auth_client
            .as_ref()
            .ok_or_else(|| AuthError::ConfigError("Auth client not initialized".to_string()))?;

        // Filter request headers (exclude sensitive ones)
        let request_headers = filter_headers(request.headers());

        // Buffer the request body for auth validation
        // Note: This buffers the entire request body in memory, which is acceptable for
        // the current use case (small JSON payloads). For routes with large file uploads,
        // consider implementing a headers-only validation variant to avoid buffering overhead.
        // Current protected routes (/voices, /speak, /livekit/token) all have small bodies.
        let (parts, body) = request.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| AuthError::ConfigError(format!("Failed to read request body: {e}")))?
            .to_bytes();

        // Parse the body as JSON (if it fails, use empty object)
        let request_body: serde_json::Value = if body_bytes.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_slice(&body_bytes).unwrap_or_else(|_| serde_json::json!({}))
        };

        // Validate the token with the auth service
        match auth_client
            .validate_token(
                &token,
                &request_body,
                request_headers,
                &request_path,
                &request_method,
            )
            .await
        {
            Ok(auth) => {
                tracing::info!(
                    method = %request_method,
                    path = %request_path,
                    auth_id = ?auth.id,
                    "JWT authentication successful"
                );

                // Token is valid, reconstruct the request with the original body and continue
                let mut request = Request::from_parts(parts, Body::from(body_bytes));
                // Insert Auth from the auth service response.
                // Handlers can access this via Extension<Auth> to get auth.id, etc.
                request.extensions_mut().insert(auth);
                Ok(next.run(request).await)
            }
            Err(e) => {
                tracing::warn!(
                    method = %request_method,
                    path = %request_path,
                    error = %e,
                    "JWT authentication failed"
                );
                Err(e)
            }
        }
    } else {
        // No authentication method configured
        Err(AuthError::ConfigError(
            "Authentication required but no auth method configured".to_string(),
        ))
    }
}

/// Helper function to create a test request with authorization header
#[cfg(test)]
pub fn create_test_request_with_auth(token: &str, body: &str) -> Request {
    use axum::http::Method;

    Request::builder()
        .method(Method::POST)
        .uri("/speak")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_test_request_with_auth() {
        let request = create_test_request_with_auth("test-token", r#"{"text": "Hello"}"#);

        let auth_header = request.headers().get("authorization").unwrap();
        assert_eq!(auth_header, "Bearer test-token");
    }

    // Note: Full middleware tests are in tests/auth_integration_test.rs
    // These tests use actual routers to properly test middleware behavior
}
