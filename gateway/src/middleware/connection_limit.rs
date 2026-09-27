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

/// Extension type to carry the client IP through to the handler.
#[derive(Clone, Debug)]
pub struct ClientIp(pub IpAddr);

/// A held WebSocket connection slot, released when the LAST clone drops.
///
/// The middleware takes the slot and puts this guard into the request's extensions, so every
/// path that ends the request — an auth rejection, a failed upgrade, a handler that never
/// upgrades — releases it by dropping the request. A handler that runs a session moves a clone
/// into the session, which then holds the slot for exactly its lifetime. Before this, only the
/// `/ws` handler released its slot, so every `/realtime` and `/v1/realtime` session and every
/// rejected `/ws` upgrade leaked one; behind Traefik (one peer IP) the pod refused all WebSockets
/// after `max_connections_per_ip` of them (FRD-022 Phase 0.1).
#[derive(Clone)]
pub struct ConnectionSlot(Arc<SlotRelease>);

struct SlotRelease {
    state: Arc<AppState>,
    ip: IpAddr,
}

impl Drop for SlotRelease {
    fn drop(&mut self) {
        tracing::debug!(ip = %self.ip, "Releasing connection slot");
        self.state.release_connection(self.ip);
    }
}

impl std::fmt::Debug for ConnectionSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ConnectionSlot").field(&self.0.ip).finish()
    }
}

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
            request.extensions_mut().insert(ClientIp(client_ip));
            // The slot rides the request: released when the request is dropped, unless a
            // handler moved a clone into its session.
            request
                .extensions_mut()
                .insert(ConnectionSlot(Arc::new(SlotRelease {
                    state: state.clone(),
                    ip: client_ip,
                })));
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

    fn limit_test_config(max_ws: Option<usize>, per_ip: u32) -> crate::config::ServerConfig {
        use crate::config::ServerConfig;
        ServerConfig {
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
            max_websocket_connections: max_ws,
            max_connections_per_ip: per_ip,
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            realtime_endpoint_overrides: Default::default(),
            aliases: Default::default(),
            plugins: crate::config::PluginConfig::default(),
            dag_timeouts: crate::config::DAGTimeoutsConfig::default(),
        }
    }

    /// Drive `n` WebSocket upgrades from one peer IP through the middleware into `handler`.
    async fn upgrades_through(
        state: Arc<AppState>,
        handler: axum::routing::MethodRouter<()>,
        n: usize,
    ) -> Vec<StatusCode> {
        use axum::extract::connect_info::MockConnectInfo;
        use tower::ServiceExt;
        let app = axum::Router::new()
            .route("/ws", handler)
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                connection_limit_middleware,
            ))
            .layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 7], 4242))));
        let mut out = Vec::new();
        for _ in 0..n {
            let req = Request::builder()
                .uri("/ws")
                .header("upgrade", "websocket")
                .body(Body::empty())
                .unwrap();
            out.push(app.clone().oneshot(req).await.unwrap().status());
        }
        out
    }

    /// TC-RG-06: a `/ws` upgrade rejected by auth releases its slot.
    #[tokio::test]
    async fn rejected_upgrades_release_their_slot() {
        let state = AppState::new(limit_test_config(Some(1000), 100)).await;
        let statuses = upgrades_through(
            state.clone(),
            axum::routing::get(|| async { StatusCode::UNAUTHORIZED }),
            150,
        )
        .await;
        assert!(
            statuses.iter().all(|s| *s == StatusCode::UNAUTHORIZED),
            "the per-IP cap must never trip on rejected requests: {statuses:?}"
        );
        assert_eq!(state.ws_connection_count(), 0);
    }

    /// TC-RG-05: sessions that end give their slot back — the 101st from one IP still gets in.
    #[tokio::test]
    async fn ended_sessions_release_their_slot() {
        let state = AppState::new(limit_test_config(Some(1000), 100)).await;
        let held: Arc<std::sync::Mutex<Vec<ConnectionSlot>>> = Default::default();
        let h = held.clone();
        let statuses = upgrades_through(
            state.clone(),
            axum::routing::get(
                move |axum::Extension(slot): axum::Extension<ConnectionSlot>| {
                    // a "session" that holds the slot, then ends
                    h.lock().unwrap().push(slot);
                    h.lock().unwrap().clear();
                    async { StatusCode::SWITCHING_PROTOCOLS }
                },
            ),
            101,
        )
        .await;
        assert!(
            statuses
                .iter()
                .all(|s| *s == StatusCode::SWITCHING_PROTOCOLS),
            "{statuses:?}"
        );
        assert_eq!(state.ws_connection_count(), 0);
    }

    /// `max_connections_per_ip = 0` turns the per-IP cap off (FRD-022 §10); the global cap stays.
    #[tokio::test]
    async fn zero_per_ip_cap_means_no_per_ip_cap() {
        let state = AppState::new(limit_test_config(Some(3), 0)).await;
        let held: Arc<std::sync::Mutex<Vec<ConnectionSlot>>> = Default::default();
        let h = held.clone();
        let handler = axum::routing::get(
            move |axum::Extension(slot): axum::Extension<ConnectionSlot>| {
                h.lock().unwrap().push(slot);
                async { StatusCode::SWITCHING_PROTOCOLS }
            },
        );
        let statuses = upgrades_through(state.clone(), handler, 4).await;
        assert_eq!(statuses[..3], [StatusCode::SWITCHING_PROTOCOLS; 3]);
        assert_eq!(
            statuses[3],
            StatusCode::SERVICE_UNAVAILABLE,
            "the global cap still applies"
        );
    }

    /// A live session keeps its slot until it ends.
    #[tokio::test]
    async fn a_live_session_holds_its_slot() {
        let state = AppState::new(limit_test_config(Some(1000), 2)).await;
        let held: Arc<std::sync::Mutex<Vec<ConnectionSlot>>> = Default::default();
        let h = held.clone();
        let handler = axum::routing::get(
            move |axum::Extension(slot): axum::Extension<ConnectionSlot>| {
                h.lock().unwrap().push(slot);
                async { StatusCode::SWITCHING_PROTOCOLS }
            },
        );
        let statuses = upgrades_through(state.clone(), handler, 3).await;
        assert_eq!(
            statuses,
            vec![
                StatusCode::SWITCHING_PROTOCOLS,
                StatusCode::SWITCHING_PROTOCOLS,
                StatusCode::TOO_MANY_REQUESTS
            ]
        );
        assert_eq!(state.ws_connection_count(), 2);
        held.lock().unwrap().clear();
        assert_eq!(state.ws_connection_count(), 0);
    }

    #[test]
    fn test_connection_limit_error_debug() {
        let global = ConnectionLimitError::GlobalLimitReached;
        let per_ip = ConnectionLimitError::PerIpLimitReached;

        assert_eq!(format!("{:?}", global), "GlobalLimitReached");
        assert_eq!(format!("{:?}", per_ip), "PerIpLimitReached");
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
            max_websocket_connections: Some(10),
            max_connections_per_ip: 3,
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            realtime_endpoint_overrides: Default::default(),
            aliases: Default::default(),
            plugins: crate::config::PluginConfig::default(),
            dag_timeouts: crate::config::DAGTimeoutsConfig::default(),
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
            max_websocket_connections: Some(5), // Global limit of 5
            max_connections_per_ip: 10,         // Per-IP limit higher than global
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            realtime_endpoint_overrides: Default::default(),
            aliases: Default::default(),
            plugins: crate::config::PluginConfig::default(),
            dag_timeouts: crate::config::DAGTimeoutsConfig::default(),
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
