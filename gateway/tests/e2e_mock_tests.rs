//! End-to-End Mock Tests
//!
//! Tests for complete request flows using mocked provider backends.
//! These tests verify that the gateway correctly handles client requests,
//! routes them to providers, and returns appropriate responses.

use std::net::TcpListener;
use std::time::Duration;

use axum::{body::Body, http::Request, Router};
use serde_json::{json, Value};
use tokio::time::timeout;
use tower::util::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use waav_gateway::{config::PluginConfig, routes, state::AppState, ServerConfig};

/// Helper function to create a minimal test configuration
fn create_test_config(port: u16) -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        livekit_api_key: None,
        livekit_api_secret: None,
        deepgram_api_key: Some("test_deepgram_key".to_string()),
        elevenlabs_api_key: Some("test_elevenlabs_key".to_string()),
        google_credentials: None,
        azure_speech_subscription_key: None,
        azure_speech_region: None,
        cartesia_api_key: None,
        openai_api_key: Some("test_openai_key".to_string()),
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
        cors_allowed_origins: Some("*".to_string()),
        rate_limit_requests_per_second: 100000, // Disable for tests
        rate_limit_burst_size: 100,
        max_websocket_connections: None,
        max_connections_per_ip: 1000,
        plugins: PluginConfig::default(),
    }
}

/// Find an available port for testing
fn find_available_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind to random port");
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

// =============================================================================
// REST API E2E Tests
// =============================================================================

/// Test the health check endpoint returns correct format
#[tokio::test]
async fn test_e2e_health_check() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .with_state(app_state);

    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();

    // Health check response has status field
    assert_eq!(json["status"], "OK");
}

/// Test the speak endpoint validates input correctly
#[tokio::test]
async fn test_e2e_speak_validation() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    // Test empty text validation
    let request_body = json!({
        "text": "",
        "tts_config": {
            "provider": "deepgram",
            "model": "aura-asteria-en",
            "voice_id": "aura-asteria-en",
            "audio_format": "linear16",
            "sample_rate": 24000,
            "pronunciations": []
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("empty"));
}

/// Test the speak endpoint handles missing provider gracefully
#[tokio::test]
async fn test_e2e_speak_missing_provider_config() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.deepgram_api_key = None; // Remove API key
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    let request_body = json!({
        "text": "Hello world",
        "tts_config": {
            "provider": "deepgram",
            "model": "aura-asteria-en",
            "voice_id": "aura-asteria-en",
            "audio_format": "linear16",
            "sample_rate": 24000
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// Test the voices endpoint returns provider voices list
#[tokio::test]
async fn test_e2e_voices_endpoint() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    // Request voices for a provider
    let request = Request::builder()
        .uri("/voices?provider=deepgram")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should get a response (may be error due to test API key, but not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// Request Format Tests
// =============================================================================

/// Test that invalid JSON is rejected
#[tokio::test]
async fn test_e2e_invalid_json_rejected() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .body(Body::from("{ invalid json }"))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Invalid JSON should return either 400 Bad Request or 422 Unprocessable Entity
    assert!(
        response.status() == axum::http::StatusCode::BAD_REQUEST
            || response.status() == axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
}

/// Test that missing content-type is handled
#[tokio::test]
async fn test_e2e_missing_content_type() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    let request_body = json!({
        "text": "Hello",
        "tts_config": {
            "provider": "deepgram",
            "voice_id": "test"
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Should either work or return an appropriate error, not crash
    assert!(response.status().is_client_error() || response.status().is_success());
}

// =============================================================================
// WebSocket Configuration Tests
// =============================================================================

/// Test WebSocket endpoint accepts upgrade requests
#[tokio::test]
async fn test_e2e_websocket_endpoint_exists() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let ws_routes = routes::ws::create_ws_router().with_state(app_state);

    let request = Request::builder()
        .uri("/ws")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();

    let response = ws_routes.oneshot(request).await.unwrap();

    // Should respond (either upgrade or bad request, not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Test Realtime WebSocket endpoint exists
#[tokio::test]
async fn test_e2e_realtime_endpoint_exists() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let realtime_routes = routes::realtime::create_realtime_router().with_state(app_state);

    let request = Request::builder()
        .uri("/realtime")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();

    let response = realtime_routes.oneshot(request).await.unwrap();

    // Should respond (either upgrade or bad request, not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// Webhook Tests
// =============================================================================

/// Test LiveKit webhook endpoint exists
#[tokio::test]
async fn test_e2e_livekit_webhook_endpoint_exists() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let webhook_routes = routes::webhooks::create_webhook_router().with_state(app_state);

    let request = Request::builder()
        .method("POST")
        .uri("/livekit/webhook")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let response = webhook_routes.oneshot(request).await.unwrap();

    // Should respond (may be unauthorized without valid signature, but not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Test webhook rejects unsigned requests when API secret is configured
#[tokio::test]
async fn test_e2e_webhook_requires_signature() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.livekit_api_key = Some("test_key".to_string());
    config.livekit_api_secret = Some("test_secret".to_string());
    let app_state = AppState::new(config).await;

    let webhook_routes = routes::webhooks::create_webhook_router().with_state(app_state);

    // Send request without signature
    let request = Request::builder()
        .method("POST")
        .uri("/livekit/webhook")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"event":"room_started"}"#))
        .unwrap();

    let response = webhook_routes.oneshot(request).await.unwrap();

    // Should reject unsigned requests
    assert!(response.status().is_client_error());
}

// =============================================================================
// Concurrent Request Tests
// =============================================================================

/// Test server handles concurrent health check requests
#[tokio::test]
async fn test_e2e_concurrent_health_checks() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .with_state(app_state);

    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let app = app.clone();
            tokio::spawn(async move {
                let request = Request::builder().uri("/").body(Body::empty()).unwrap();
                let response = app.oneshot(request).await.unwrap();
                response.status()
            })
        })
        .collect();

    let mut success_count = 0;
    for task in tasks {
        if let Ok(status) = task.await {
            if status == axum::http::StatusCode::OK {
                success_count += 1;
            }
        }
    }

    assert_eq!(success_count, 20);
}

// =============================================================================
// Provider API Key Tests
// =============================================================================

/// Test that provider selection works with multiple configured providers
#[tokio::test]
async fn test_e2e_provider_selection() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.deepgram_api_key = Some("deepgram_key".to_string());
    config.elevenlabs_api_key = Some("elevenlabs_key".to_string());
    let app_state = AppState::new(config).await;

    // Verify both providers are configured
    assert!(app_state.config.deepgram_api_key.is_some());
    assert!(app_state.config.elevenlabs_api_key.is_some());
}

/// Test that X-Provider-Api-Key header is recognized
#[tokio::test]
async fn test_e2e_provider_api_key_header() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.deepgram_api_key = None; // Remove server-side key
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    let request_body = json!({
        "text": "Hello world",
        "tts_config": {
            "provider": "deepgram",
            "voice_id": "aura-asteria-en"
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .header("x-provider-api-key", "user_provided_key")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    // Should process request (may fail at provider due to invalid key, but not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// Error Response Format Tests
// =============================================================================

/// Test that error responses have consistent format
#[tokio::test]
async fn test_e2e_error_response_format() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    let request_body = json!({
        "text": "",
        "tts_config": {
            "provider": "deepgram",
            "voice_id": "test"
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();

    // Should be an error response
    assert!(status.is_client_error() || status.is_server_error());

    // Try to parse as JSON if possible
    if let Ok(json) = serde_json::from_slice::<Value>(&body) {
        // Error responses should have an "error" field
        assert!(json.get("error").is_some() || json.get("message").is_some());
    }
    // Some error responses might be plain text, which is also acceptable
}

// =============================================================================
// Rate Limiting Tests
// =============================================================================

/// Test that rate limiting can be configured
#[tokio::test]
async fn test_e2e_rate_limit_configuration() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.rate_limit_requests_per_second = 10;
    config.rate_limit_burst_size = 5;

    let app_state = AppState::new(config).await;
    assert_eq!(app_state.config.rate_limit_requests_per_second, 10);
    assert_eq!(app_state.config.rate_limit_burst_size, 5);
}

// =============================================================================
// Connection Limit Tests
// =============================================================================

/// Test that connection limits can be configured
#[tokio::test]
async fn test_e2e_connection_limit_configuration() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.max_websocket_connections = Some(100);
    config.max_connections_per_ip = 5;

    let app_state = AppState::new(config).await;
    assert_eq!(app_state.config.max_websocket_connections, Some(100));
    assert_eq!(app_state.config.max_connections_per_ip, 5);
}

// =============================================================================
// Timeout Tests
// =============================================================================

/// Test that requests are handled within reasonable time
#[tokio::test]
async fn test_e2e_health_check_response_time() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .with_state(app_state);

    let start = std::time::Instant::now();

    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let result = timeout(Duration::from_secs(5), app.oneshot(request)).await;

    let elapsed = start.elapsed();

    assert!(result.is_ok(), "Request timed out");
    assert!(elapsed < Duration::from_secs(1), "Response took too long");
}

// =============================================================================
// Full Router Integration Tests
// =============================================================================

/// Test that all routes can be combined into a single application
#[tokio::test]
async fn test_e2e_combined_router() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    // Create full combined router like in main.rs
    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .merge(routes::api::create_api_router())
        .merge(routes::webhooks::create_webhook_router())
        .merge(routes::ws::create_ws_router())
        .merge(routes::realtime::create_realtime_router())
        .with_state(app_state);

    // Test health check works
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // Test API routes are accessible
    let request = Request::builder()
        .uri("/voices?provider=deepgram")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// Configuration Persistence Tests
// =============================================================================

/// Test that configuration is correctly passed through the app
#[tokio::test]
async fn test_e2e_config_persistence() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.cors_allowed_origins = Some("https://example.com".to_string());
    config.auth_required = false;

    let app_state = AppState::new(config).await;

    assert_eq!(
        app_state.config.cors_allowed_origins,
        Some("https://example.com".to_string())
    );
    assert!(!app_state.config.auth_required);
}
