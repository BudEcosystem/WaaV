//! Connection limit middleware for WebSocket connections
//!
//! This module provides middleware to enforce connection limits:
//! - Global maximum WebSocket connections
//! - Per-IP connection limits
//!
//! # Example
//!
//! ```ignore
//! use axum::Router;
//! use waav_gateway::middleware::connection_limit_middleware;
//!
//! let app = Router::new()
//!     .route("/ws", get(websocket_handler))
//!     .layer(axum::middleware::from_fn_with_state(
//!         state.clone(),
//!         connection_limit_middleware,
//!     ));
//! ```

use axum::{
    body::Body,
    extract::{ConnectInfo, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use crate::state::{AppState, ConnectionLimitError};

/// Extension type to carry the client IP through to the handler
/// so the handler can release the connection when done.
#[derive(Clone, Debug)]
pub struct ClientIp(pub IpAddr);

/// Middleware that enforces connection limits for WebSocket connections.
///
/// This middleware:
/// 1. Checks if the global WebSocket connection limit has been reached
/// 2. Checks if the per-IP connection limit has been reached
/// 3. Returns 503 Service Unavailable if global limit is exceeded
/// 4. Returns 429 Too Many Requests if per-IP limit is exceeded
/// 5. Injects `ClientIp` extension so handlers can release the connection later
///
/// The middleware only applies to WebSocket upgrade requests (detected by the
/// Upgrade header). Non-WebSocket requests pass through without limit checks.
pub async fn connection_limit_middleware(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // Only apply limits to WebSocket upgrade requests
    let is_ws_upgrade = request
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.eq_ignore_ascii_case("websocket"))
        .unwrap_or(false);

    if !is_ws_upgrade {
        // Not a WebSocket upgrade, pass through
        return next.run(request).await;
    }

    let client_ip = addr.ip();

    // Try to acquire a connection slot
    match state.try_acquire_connection(client_ip) {
        Ok(()) => {
            // Connection acquired, inject the client IP so handler can release it
            request.extensions_mut().insert(ClientIp(client_ip));
            // Proceed with the request
            // The connection will be released in the WebSocket handler
            next.run(request).await
        }
        Err(ConnectionLimitError::GlobalLimitReached) => {
            tracing::warn!(
                ip = %client_ip,
                "Rejecting connection: global limit reached"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Server at capacity. Please try again later.",
            )
                .into_response()
        }
        Err(ConnectionLimitError::PerIpLimitReached) => {
            tracing::warn!(
                ip = %client_ip,
                "Rejecting connection: per-IP limit reached"
            );
            (
                StatusCode::TOO_MANY_REQUESTS,
                "Too many connections from your IP address.",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_connection_limit_error_debug() {
        let global = ConnectionLimitError::GlobalLimitReached;
        let per_ip = ConnectionLimitError::PerIpLimitReached;

        assert_eq!(
            format!("{:?}", global),
            "GlobalLimitReached"
        );
        assert_eq!(
            format!("{:?}", per_ip),
            "PerIpLimitReached"
        );
    }

    #[tokio::test]
    async fn test_connection_tracking_basic() {
        use crate::config::ServerConfig;
        use std::net::IpAddr;

        let config = ServerConfig {
            host: "localhost".to_string(),
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
            max_websocket_connections: Some(10),
            max_connections_per_ip: 3,
        };

        let state = AppState::new(config).await;
        let ip: IpAddr = Ipv4Addr::new(192, 168, 1, 100).into();

        // Should start with 0 connections
        assert_eq!(state.ws_connection_count(), 0);
        assert_eq!(state.ip_connection_count(&ip), 0);

        // Acquire first connection
        assert!(state.try_acquire_connection(ip).is_ok());
        assert_eq!(state.ws_connection_count(), 1);
        assert_eq!(state.ip_connection_count(&ip), 1);

        // Acquire second connection
        assert!(state.try_acquire_connection(ip).is_ok());
        assert_eq!(state.ws_connection_count(), 2);
        assert_eq!(state.ip_connection_count(&ip), 2);

        // Acquire third connection (at limit)
        assert!(state.try_acquire_connection(ip).is_ok());
        assert_eq!(state.ws_connection_count(), 3);
        assert_eq!(state.ip_connection_count(&ip), 3);

        // Fourth connection should be rejected (per-IP limit)
        assert_eq!(
            state.try_acquire_connection(ip),
            Err(ConnectionLimitError::PerIpLimitReached)
        );

        // Release one connection
        state.release_connection(ip);
        assert_eq!(state.ws_connection_count(), 2);
        assert_eq!(state.ip_connection_count(&ip), 2);

        // Should be able to acquire again
        assert!(state.try_acquire_connection(ip).is_ok());
        assert_eq!(state.ws_connection_count(), 3);
    }

    #[tokio::test]
    async fn test_global_connection_limit() {
        use crate::config::ServerConfig;
        use std::net::IpAddr;

        let config = ServerConfig {
            host: "localhost".to_string(),
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
            max_websocket_connections: Some(5),  // Global limit of 5
            max_connections_per_ip: 10,           // Per-IP limit higher than global
        };

        let state = AppState::new(config).await;

        // Use different IPs to avoid per-IP limit
        let ips: Vec<IpAddr> = (1..=6)
            .map(|i| Ipv4Addr::new(192, 168, 1, i).into())
            .collect();

        // First 5 should succeed
        for ip in &ips[0..5] {
            assert!(state.try_acquire_connection(*ip).is_ok());
        }
        assert_eq!(state.ws_connection_count(), 5);

        // 6th should fail with global limit
        assert_eq!(
            state.try_acquire_connection(ips[5]),
            Err(ConnectionLimitError::GlobalLimitReached)
        );

        // Release one and try again
        state.release_connection(ips[0]);
        assert!(state.try_acquire_connection(ips[5]).is_ok());
    }
}
