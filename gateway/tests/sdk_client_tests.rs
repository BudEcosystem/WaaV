//! SDK Client Tests
//!
//! Tests that simulate SDK client behavior connecting to the gateway.
//! These tests verify the gateway works correctly from a client's perspective.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{body::Body, http::Request, Router};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tower::util::ServiceExt;

use waav_gateway::{config::PluginConfig, routes, state::AppState, ServerConfig};

// =============================================================================
// SDK Client Types
// =============================================================================

/// Configuration message sent to initialize a WebSocket session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub stt_config: Option<STTConfig>,
    pub tts_config: Option<TTSConfig>,
}

/// STT configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct STTConfig {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

/// TTS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TTSConfig {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
}

/// Speak command message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flush: Option<bool>,
}

/// Clear command message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
}

/// Response message from server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_final: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// =============================================================================
// Test Helpers
// =============================================================================

/// Helper function to create a minimal test configuration
fn create_test_config(port: u16) -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        livekit_api_key: Some("test_key".to_string()),
        livekit_api_secret: Some("test_secret".to_string()),
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
        rate_limit_requests_per_second: 100000,
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

/// Generate test audio data (16kHz PCM silence)
fn generate_test_audio(duration_ms: u32) -> Vec<u8> {
    let sample_rate = 16000u32;
    let samples = (sample_rate * duration_ms / 1000) as usize;
    // Generate silence (zeros) as 16-bit PCM
    vec![0u8; samples * 2]
}

/// Generate test audio with a simple sine wave pattern
fn generate_sine_wave_audio(duration_ms: u32, frequency_hz: f32) -> Vec<u8> {
    let sample_rate = 16000u32;
    let samples = (sample_rate * duration_ms / 1000) as usize;
    let mut audio = Vec::with_capacity(samples * 2);

    for i in 0..samples {
        let t = i as f32 / sample_rate as f32;
        let sample = (2.0 * std::f32::consts::PI * frequency_hz * t).sin();
        let sample_i16 = (sample * 16000.0) as i16;
        audio.extend_from_slice(&sample_i16.to_le_bytes());
    }

    audio
}

// =============================================================================
// Config Message Tests
// =============================================================================

/// Test that config message serializes correctly
#[test]
fn test_config_message_serialization() {
    let config = ConfigMessage {
        msg_type: "config".to_string(),
        stt_config: Some(STTConfig {
            provider: "deepgram".to_string(),
            model: Some("nova-2".to_string()),
            language: Some("en-US".to_string()),
            sample_rate: Some(16000),
        }),
        tts_config: Some(TTSConfig {
            provider: "elevenlabs".to_string(),
            voice_id: Some("test_voice".to_string()),
            model: None,
            sample_rate: Some(22050),
        }),
    };

    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("\"type\":\"config\""));
    assert!(json.contains("\"provider\":\"deepgram\""));
    assert!(json.contains("\"provider\":\"elevenlabs\""));
}

/// Test that config message deserializes correctly
#[test]
fn test_config_message_deserialization() {
    let json = r#"{
        "type": "config",
        "stt_config": {
            "provider": "deepgram",
            "model": "nova-2"
        },
        "tts_config": {
            "provider": "elevenlabs",
            "voice_id": "voice123"
        }
    }"#;

    let config: ConfigMessage = serde_json::from_str(json).unwrap();
    assert_eq!(config.msg_type, "config");
    assert_eq!(config.stt_config.unwrap().provider, "deepgram");
    assert_eq!(config.tts_config.unwrap().provider, "elevenlabs");
}

/// Test speak message serialization
#[test]
fn test_speak_message_serialization() {
    let speak = SpeakMessage {
        msg_type: "speak".to_string(),
        text: "Hello, world!".to_string(),
        flush: Some(true),
    };

    let json = serde_json::to_string(&speak).unwrap();
    assert!(json.contains("\"type\":\"speak\""));
    assert!(json.contains("\"text\":\"Hello, world!\""));
    assert!(json.contains("\"flush\":true"));
}

/// Test clear message serialization
#[test]
fn test_clear_message_serialization() {
    let clear = ClearMessage {
        msg_type: "clear".to_string(),
    };

    let json = serde_json::to_string(&clear).unwrap();
    assert!(json.contains("\"type\":\"clear\""));
}

// =============================================================================
// Audio Data Tests
// =============================================================================

/// Test that silence audio generation works correctly
#[test]
fn test_generate_silence_audio() {
    let audio = generate_test_audio(100); // 100ms
    // 16kHz * 100ms / 1000 = 1600 samples * 2 bytes = 3200 bytes
    assert_eq!(audio.len(), 3200);
    // All bytes should be zero (silence)
    assert!(audio.iter().all(|&b| b == 0));
}

/// Test that sine wave audio generation works correctly
#[test]
fn test_generate_sine_wave_audio() {
    let audio = generate_sine_wave_audio(100, 440.0); // 100ms at 440Hz
    assert_eq!(audio.len(), 3200);
    // Should not be all zeros
    assert!(!audio.iter().all(|&b| b == 0));
}

/// Test audio chunking for streaming
#[test]
fn test_audio_chunking() {
    let audio = generate_test_audio(1000); // 1 second
    let chunk_size = 3200; // 100ms chunks

    let chunks: Vec<&[u8]> = audio.chunks(chunk_size).collect();
    assert_eq!(chunks.len(), 10); // 1000ms / 100ms = 10 chunks
}

// =============================================================================
// REST API Client Tests
// =============================================================================

/// Test REST client health check
#[tokio::test]
async fn test_sdk_rest_health_check() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .with_state(app_state);

    // Simulate SDK client making health check request
    let request = Request::builder()
        .uri("/")
        .header("user-agent", "WaavSDK/1.0")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
}

/// Test REST client TTS request
#[tokio::test]
async fn test_sdk_rest_tts_request() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    // Simulate SDK client making TTS request
    let request_body = json!({
        "text": "Hello from SDK",
        "tts_config": {
            "provider": "deepgram",
            "voice_id": "aura-asteria-en",
            "sample_rate": 24000
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .header("user-agent", "WaavSDK/1.0")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Will fail due to test API key, but should process request
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Test REST client voices list request
#[tokio::test]
async fn test_sdk_rest_voices_list() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = routes::api::create_api_router().with_state(app_state);

    // Simulate SDK client requesting voices list
    let request = Request::builder()
        .uri("/voices?provider=deepgram")
        .header("user-agent", "WaavSDK/1.0")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// WebSocket Client Tests
// =============================================================================

/// Test WebSocket connection establishment
#[tokio::test]
async fn test_sdk_websocket_connection() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let ws_routes = routes::ws::create_ws_router().with_state(app_state);

    // Simulate SDK client WebSocket upgrade request
    let request = Request::builder()
        .uri("/ws")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .header("user-agent", "WaavSDK/1.0")
        .body(Body::empty())
        .unwrap();

    let response = ws_routes.oneshot(request).await.unwrap();
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

/// Test WebSocket with custom headers
#[tokio::test]
async fn test_sdk_websocket_with_auth_header() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let ws_routes = routes::ws::create_ws_router().with_state(app_state);

    // Simulate SDK client with auth header
    let request = Request::builder()
        .uri("/ws")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .header("authorization", "Bearer test_token")
        .body(Body::empty())
        .unwrap();

    let response = ws_routes.oneshot(request).await.unwrap();
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// Realtime Client Tests
// =============================================================================

/// Test Realtime WebSocket connection
#[tokio::test]
async fn test_sdk_realtime_connection() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let realtime_routes = routes::realtime::create_realtime_router().with_state(app_state);

    // Simulate SDK client connecting to realtime endpoint
    let request = Request::builder()
        .uri("/realtime")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .header("sec-websocket-version", "13")
        .header("user-agent", "WaavSDK/1.0")
        .body(Body::empty())
        .unwrap();

    let response = realtime_routes.oneshot(request).await.unwrap();
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// Client Configuration Tests
// =============================================================================

/// Test client with different provider configurations
#[tokio::test]
async fn test_sdk_multiple_providers() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.deepgram_api_key = Some("dg_key".to_string());
    config.elevenlabs_api_key = Some("el_key".to_string());
    config.openai_api_key = Some("oai_key".to_string());

    let app_state = AppState::new(config).await;

    // Verify multiple providers are available
    assert!(app_state.config.deepgram_api_key.is_some());
    assert!(app_state.config.elevenlabs_api_key.is_some());
    assert!(app_state.config.openai_api_key.is_some());
}

/// Test client API key override via header
#[tokio::test]
async fn test_sdk_api_key_override() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.deepgram_api_key = None; // No server-side key

    let app_state = AppState::new(config).await;
    let app = routes::api::create_api_router().with_state(app_state);

    // SDK provides API key via header
    let request_body = json!({
        "text": "Test",
        "tts_config": {
            "provider": "deepgram",
            "voice_id": "aura-asteria-en"
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .header("x-provider-api-key", "client_provided_key")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Request should be processed (may fail at provider, but not rejected by gateway)
    assert_ne!(response.status(), axum::http::StatusCode::NOT_FOUND);
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Test SDK handles server errors gracefully
#[tokio::test]
async fn test_sdk_error_handling() {
    let port = find_available_port();
    let mut config = create_test_config(port);
    config.deepgram_api_key = None;

    let app_state = AppState::new(config).await;
    let app = routes::api::create_api_router().with_state(app_state);

    // Request with invalid provider
    let request_body = json!({
        "text": "Test",
        "tts_config": {
            "provider": "invalid_provider",
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

    // Should return error response
    assert!(response.status().is_client_error() || response.status().is_server_error());
}

/// Test SDK handles validation errors
#[tokio::test]
async fn test_sdk_validation_error_handling() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;
    let app = routes::api::create_api_router().with_state(app_state);

    // Empty text should be rejected
    let request_body = json!({
        "text": "",
        "tts_config": {
            "provider": "deepgram",
            "voice_id": "aura-asteria-en"
        }
    });

    let request = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .body(Body::from(request_body.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    // Validation errors return 400 Bad Request or 422 Unprocessable Entity
    assert!(
        response.status() == axum::http::StatusCode::BAD_REQUEST
            || response.status() == axum::http::StatusCode::UNPROCESSABLE_ENTITY
    );
}

// =============================================================================
// Concurrent Client Tests
// =============================================================================

/// Test multiple concurrent SDK clients
#[tokio::test]
async fn test_sdk_concurrent_clients() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .merge(routes::api::create_api_router())
        .with_state(app_state);

    // Simulate 50 concurrent SDK clients
    let tasks: Vec<_> = (0..50)
        .map(|i| {
            let app = app.clone();
            tokio::spawn(async move {
                let request = Request::builder()
                    .uri("/")
                    .header("user-agent", format!("WaavSDK/1.0 Client-{}", i))
                    .body(Body::empty())
                    .unwrap();
                app.oneshot(request).await.unwrap().status()
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

    assert_eq!(success_count, 50);
}

// =============================================================================
// Session Management Tests
// =============================================================================

/// Test SDK can manage multiple sessions
#[tokio::test]
async fn test_sdk_session_management() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    // Each session should have isolated state
    let app1 = routes::ws::create_ws_router().with_state(app_state.clone());
    let app2 = routes::ws::create_ws_router().with_state(app_state.clone());

    // Both should be able to accept connections independently
    let request1 = Request::builder()
        .uri("/ws")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "key1key1key1key1key1key1")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();

    let request2 = Request::builder()
        .uri("/ws")
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-key", "key2key2key2key2key2key2")
        .header("sec-websocket-version", "13")
        .body(Body::empty())
        .unwrap();

    let (response1, response2) = tokio::join!(app1.oneshot(request1), app2.oneshot(request2));

    assert_ne!(
        response1.unwrap().status(),
        axum::http::StatusCode::NOT_FOUND
    );
    assert_ne!(
        response2.unwrap().status(),
        axum::http::StatusCode::NOT_FOUND
    );
}

// =============================================================================
// Timeout Tests
// =============================================================================

/// Test SDK request timeout handling
#[tokio::test]
async fn test_sdk_request_timeout() {
    let port = find_available_port();
    let config = create_test_config(port);
    let app_state = AppState::new(config).await;

    let app = Router::new()
        .route(
            "/",
            axum::routing::get(waav_gateway::handlers::api::health_check),
        )
        .with_state(app_state);

    // Request should complete within reasonable time
    let request = Request::builder().uri("/").body(Body::empty()).unwrap();

    let result = timeout(Duration::from_secs(5), app.oneshot(request)).await;

    assert!(result.is_ok(), "Request should not timeout");
}

// =============================================================================
// Protocol Version Tests
// =============================================================================

/// Test SDK message format compatibility
#[test]
fn test_sdk_message_format_v1() {
    // V1 message format
    let config_v1 = json!({
        "type": "config",
        "stt_config": {
            "provider": "deepgram"
        },
        "tts_config": {
            "provider": "elevenlabs"
        }
    });

    let parsed: ConfigMessage = serde_json::from_value(config_v1).unwrap();
    assert_eq!(parsed.msg_type, "config");
}

/// Test SDK can parse server responses
#[test]
fn test_sdk_parse_server_response() {
    let ready_response = json!({
        "type": "ready",
        "stream_id": "test-stream-123"
    });

    let parsed: ServerMessage = serde_json::from_value(ready_response).unwrap();
    assert_eq!(parsed.msg_type, "ready");
    assert_eq!(parsed.stream_id, Some("test-stream-123".to_string()));
}

/// Test SDK can parse error responses
#[test]
fn test_sdk_parse_error_response() {
    let error_response = json!({
        "type": "error",
        "error": "Invalid configuration"
    });

    let parsed: ServerMessage = serde_json::from_value(error_response).unwrap();
    assert_eq!(parsed.msg_type, "error");
    assert_eq!(parsed.error, Some("Invalid configuration".to_string()));
}

/// Test SDK can parse STT result
#[test]
fn test_sdk_parse_stt_result() {
    let stt_result = json!({
        "type": "stt_result",
        "transcript": "Hello world",
        "is_final": true
    });

    let parsed: ServerMessage = serde_json::from_value(stt_result).unwrap();
    assert_eq!(parsed.msg_type, "stt_result");
    assert_eq!(parsed.transcript, Some("Hello world".to_string()));
    assert_eq!(parsed.is_final, Some(true));
}
