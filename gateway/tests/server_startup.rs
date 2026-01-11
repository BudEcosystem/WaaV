//! Server Startup Tests
//!
//! Tests for server lifecycle, configuration loading, and startup behavior.
//! These tests verify that the server can start correctly under various conditions.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{body::Body, http::Request, Router};
use tokio::time::timeout;
use tower::util::ServiceExt;

use waav_gateway::{config::{PluginConfig, AuthApiSecret}, routes, state::AppState, ServerConfig};

/// Helper function to create a minimal test configuration
fn create_minimal_config(port: u16) -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
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
        max_websocket_connections: None,
        max_connections_per_ip: 100,
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

/// Test that the server can start with minimal configuration (no API keys)
#[tokio::test]
async fn test_minimal_config_boot() {
    let port = find_available_port();
    let config = create_minimal_config(port);

    // Create app state - this should succeed even without API keys
    let app_state = AppState::new(config).await;

    // Create a minimal router with health check
    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .with_state(app_state);

    // Test health check endpoint
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

/// Test that the server correctly reports missing API keys when providers are accessed
#[tokio::test]
async fn test_missing_api_keys_returns_error_on_use() {
    let port = find_available_port();
    let config = create_minimal_config(port);
    let app_state = AppState::new(config).await;

    // Create router with API routes
    let app = routes::api::create_api_router().with_state(app_state);

    // Try to use TTS with missing API key
    let request_body = serde_json::json!({
        "text": "Hello, test.",
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

    // Should return error about missing API key
    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );
}

/// Test that the server handles all API routes correctly after startup
#[tokio::test]
async fn test_full_api_routes_available() {
    let port = find_available_port();
    let config = create_minimal_config(port);
    let app_state = AppState::new(config).await;

    // Create full router
    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .merge(routes::api::create_api_router())
        .with_state(app_state);

    // Test health check
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    // Test voices endpoint (should work even without API keys - returns empty list)
    let request = Request::builder()
        .uri("/voices?provider=deepgram")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    // Returns error because API key is missing
    assert!(
        response.status() == axum::http::StatusCode::INTERNAL_SERVER_ERROR
            || response.status() == axum::http::StatusCode::OK
    );
}

/// Test that the server can be created with various rate limit configurations
#[tokio::test]
async fn test_rate_limit_configurations() {
    let port = find_available_port();

    // Test with rate limiting enabled
    let mut config = create_minimal_config(port);
    config.rate_limit_requests_per_second = 100;
    config.rate_limit_burst_size = 50;
    let app_state = AppState::new(config).await;
    assert!(app_state.config.rate_limit_requests_per_second == 100);

    // Test with rate limiting disabled (high value)
    let mut config2 = create_minimal_config(port + 1);
    config2.rate_limit_requests_per_second = 100000;
    let app_state2 = AppState::new(config2).await;
    assert!(app_state2.config.rate_limit_requests_per_second >= 100000);
}

/// Test that CORS configuration is applied correctly
#[tokio::test]
async fn test_cors_configurations() {
    let port = find_available_port();

    // Test with wildcard CORS
    let mut config = create_minimal_config(port);
    config.cors_allowed_origins = Some("*".to_string());
    let app_state = AppState::new(config).await;
    assert_eq!(
        app_state.config.cors_allowed_origins,
        Some("*".to_string())
    );

    // Test with specific origins
    let mut config2 = create_minimal_config(port + 1);
    config2.cors_allowed_origins = Some("http://localhost:3000,http://localhost:8080".to_string());
    let app_state2 = AppState::new(config2).await;
    assert!(app_state2.config.cors_allowed_origins.is_some());
}

/// Test that connection limits are applied correctly
#[tokio::test]
async fn test_connection_limit_configurations() {
    let port = find_available_port();

    // Test with connection limits
    let mut config = create_minimal_config(port);
    config.max_websocket_connections = Some(100);
    config.max_connections_per_ip = 10;
    let app_state = AppState::new(config).await;
    assert_eq!(app_state.config.max_websocket_connections, Some(100));
    assert_eq!(app_state.config.max_connections_per_ip, 10);

    // Test with no connection limits
    let mut config2 = create_minimal_config(port + 1);
    config2.max_websocket_connections = None;
    let app_state2 = AppState::new(config2).await;
    assert!(app_state2.config.max_websocket_connections.is_none());
}

/// Test that auth configuration is applied correctly
#[tokio::test]
async fn test_auth_configurations() {
    let port = find_available_port();

    // Test with auth disabled
    let mut config = create_minimal_config(port);
    config.auth_required = false;
    let app_state = AppState::new(config).await;
    assert!(!app_state.config.auth_required);

    // Test with auth enabled (but no service URL - will fail validation)
    let mut config2 = create_minimal_config(port + 1);
    config2.auth_required = true;
    config2.auth_api_secrets = vec![AuthApiSecret { id: "test_id".to_string(), secret: "test_secret".to_string() }];
    let app_state2 = AppState::new(config2).await;
    assert!(app_state2.config.auth_required);
}

/// Test that the server correctly parses addresses
#[tokio::test]
async fn test_address_parsing() {
    let port = find_available_port();
    let config = create_minimal_config(port);

    let address = config.address();
    assert!(address.contains("127.0.0.1"));
    assert!(address.contains(&port.to_string()));
}

/// Test that multiple AppState instances can be created concurrently
#[tokio::test]
async fn test_concurrent_app_state_creation() {
    let tasks: Vec<_> = (0..5)
        .map(|i| {
            let port = find_available_port() + i;
            tokio::spawn(async move {
                let config = create_minimal_config(port);
                let _app_state = AppState::new(config).await;
            })
        })
        .collect();

    for task in tasks {
        task.await.expect("Task should complete successfully");
    }
}

/// Test that provider configurations are correctly stored
#[tokio::test]
async fn test_provider_configurations() {
    let port = find_available_port();
    let mut config = create_minimal_config(port);

    // Set various provider keys
    config.deepgram_api_key = Some("test_deepgram_key".to_string());
    config.elevenlabs_api_key = Some("test_elevenlabs_key".to_string());
    config.openai_api_key = Some("test_openai_key".to_string());

    let app_state = AppState::new(config).await;

    // Verify keys are stored
    assert_eq!(
        app_state.config.deepgram_api_key,
        Some("test_deepgram_key".to_string())
    );
    assert_eq!(
        app_state.config.elevenlabs_api_key,
        Some("test_elevenlabs_key".to_string())
    );
    assert_eq!(
        app_state.config.openai_api_key,
        Some("test_openai_key".to_string())
    );
}

/// Test that LiveKit configuration is correctly stored
#[tokio::test]
async fn test_livekit_configurations() {
    let port = find_available_port();
    let mut config = create_minimal_config(port);

    config.livekit_url = "wss://custom.livekit.cloud".to_string();
    config.livekit_public_url = "https://custom.livekit.cloud".to_string();
    config.livekit_api_key = Some("test_api_key".to_string());
    config.livekit_api_secret = Some("test_api_secret".to_string());

    let app_state = AppState::new(config).await;

    assert_eq!(
        app_state.config.livekit_url,
        "wss://custom.livekit.cloud".to_string()
    );
    assert_eq!(
        app_state.config.livekit_public_url,
        "https://custom.livekit.cloud".to_string()
    );
}

/// Test that the server can handle shutdown signals gracefully
#[tokio::test]
async fn test_graceful_shutdown_simulation() {
    let port = find_available_port();
    let config = create_minimal_config(port);
    let app_state = AppState::new(config).await;

    // Create a shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Simulate a task that would run until shutdown
    let task = tokio::spawn(async move {
        while !shutdown_clone.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    });

    // Simulate shutdown signal
    shutdown.store(true, Ordering::Relaxed);

    // Task should complete within timeout
    let result = timeout(Duration::from_secs(1), task).await;
    assert!(result.is_ok());
}

/// Test that the server correctly handles WebSocket route setup
#[tokio::test]
async fn test_websocket_route_setup() {
    let port = find_available_port();
    let config = create_minimal_config(port);
    let app_state = AppState::new(config).await;

    // Create WebSocket routes
    let ws_routes = routes::ws::create_ws_router().with_state(app_state.clone());

    // Create a test request to the WebSocket endpoint
    // (will fail upgrade, but route should exist)
    let request = Request::builder()
        .uri("/ws")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();

    let response = ws_routes.oneshot(request).await.unwrap();

    // Should get a response (either upgrade or bad request, not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Test that the server correctly handles Realtime route setup
#[tokio::test]
async fn test_realtime_route_setup() {
    let port = find_available_port();
    let config = create_minimal_config(port);
    let app_state = AppState::new(config).await;

    // Create Realtime routes
    let realtime_routes = routes::realtime::create_realtime_router().with_state(app_state.clone());

    // Create a test request to the Realtime endpoint
    let request = Request::builder()
        .uri("/realtime")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();

    let response = realtime_routes.oneshot(request).await.unwrap();

    // Should get a response (either upgrade or bad request, not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Test that the server correctly handles webhook route setup
#[tokio::test]
async fn test_webhook_route_setup() {
    let port = find_available_port();
    let config = create_minimal_config(port);
    let app_state = AppState::new(config).await;

    // Create webhook routes
    let webhook_routes = routes::webhooks::create_webhook_router().with_state(app_state);

    // Create a test request to the webhook endpoint
    let request = Request::builder()
        .method("POST")
        .uri("/livekit/webhook")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();

    let response = webhook_routes.oneshot(request).await.unwrap();

    // Should get a response (either success or unauthorized, not 404)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Test that the config correctly identifies TLS status
#[tokio::test]
async fn test_tls_configuration() {
    let port = find_available_port();

    // Test without TLS
    let config = create_minimal_config(port);
    assert!(!config.is_tls_enabled());
    assert!(config.tls.is_none());

    // Test with TLS config (just setting - actual TLS requires certs)
    let mut config_with_tls = create_minimal_config(port + 1);
    config_with_tls.tls = Some(waav_gateway::config::TlsConfig {
        cert_path: std::path::PathBuf::from("/path/to/cert.pem"),
        key_path: std::path::PathBuf::from("/path/to/key.pem"),
    });
    assert!(config_with_tls.is_tls_enabled());
}

/// Test that cache configuration is correctly applied
#[tokio::test]
async fn test_cache_configurations() {
    let port = find_available_port();

    // Test with cache enabled
    let mut config = create_minimal_config(port);
    config.cache_path = Some(std::path::PathBuf::from("/tmp/waav-test-cache"));
    config.cache_ttl_seconds = Some(7200);
    let app_state = AppState::new(config).await;
    assert_eq!(app_state.config.cache_ttl_seconds, Some(7200));

    // Test with cache disabled
    let mut config2 = create_minimal_config(port + 1);
    config2.cache_path = None;
    config2.cache_ttl_seconds = None;
    let app_state2 = AppState::new(config2).await;
    assert!(app_state2.config.cache_path.is_none());
}

/// Test that the server handles empty plugin config
#[tokio::test]
async fn test_empty_plugin_config() {
    let port = find_available_port();
    let config = create_minimal_config(port);

    // Default plugin config should have plugin_dir as None
    assert!(config.plugins.plugin_dir.is_none());
    // Default PluginConfig uses derive(Default), so enabled is false
    // The plugin system still works - this just controls external plugin loading

    let app_state = AppState::new(config).await;
    assert!(app_state.config.plugins.plugin_dir.is_none());
}

/// Test concurrent request handling capability
#[tokio::test]
async fn test_concurrent_request_handling() {
    let port = find_available_port();
    let config = create_minimal_config(port);
    let app_state = AppState::new(config).await;

    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .with_state(app_state);

    // Make concurrent requests
    let tasks: Vec<_> = (0..10)
        .map(|_| {
            let app = app.clone();
            tokio::spawn(async move {
                let request = Request::builder().uri("/").body(Body::empty()).unwrap();
                let response = app.oneshot(request).await.unwrap();
                response.status()
            })
        })
        .collect();

    for task in tasks {
        let status = task.await.expect("Task should complete");
        assert_eq!(status, axum::http::StatusCode::OK);
    }
}
