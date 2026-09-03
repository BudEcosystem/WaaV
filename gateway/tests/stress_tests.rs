//! Stress Tests for WaaV Gateway
//!
//! These tests verify the gateway's behavior under extreme conditions:
//! - Maximum concurrent connections
//! - Large payload handling
//! - Rapid connect/disconnect cycles
//! - Memory pressure scenarios
//! - Resource exhaustion handling
//!
//! Run: cargo test --test stress_tests -- --nocapture
//! Run with release: cargo test --test stress_tests --release -- --nocapture

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::future::join_all;
use futures::{FutureExt, SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    handlers,
    middleware::auth_middleware,
    routes,
    state::AppState,
};

mod common {
    use super::*;
    use axum::{Router, middleware};
    use std::sync::Arc;

    pub struct TestServer {
        handle: JoinHandle<()>,
        panicked: Arc<AtomicBool>,
    }

    impl Drop for TestServer {
        fn drop(&mut self) {
            if !self.handle.is_finished() {
                self.handle.abort();
            }
            if self.panicked.load(Ordering::SeqCst) {
                if std::thread::panicking() {
                    eprintln!("stress test server panicked");
                } else {
                    panic!("stress test server panicked");
                }
            }
        }
    }

    fn spawn_test_server<F>(future: F) -> TestServer
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let panicked = Arc::new(AtomicBool::new(false));
        let panicked_in_task = Arc::clone(&panicked);
        let handle = tokio::spawn(async move {
            if AssertUnwindSafe(future).catch_unwind().await.is_err() {
                panicked_in_task.store(true, Ordering::SeqCst);
            }
        });
        TestServer { handle, panicked }
    }

    fn create_minimal_config(port: u16) -> ServerConfig {
        ServerConfig {
            host: "127.0.0.1".to_string(),
            port,
            tls: None,
            livekit_url: "ws://localhost:7880".to_string(),
            livekit_public_url: "http://localhost:7880".to_string(),
            livekit_api_key: None,
            livekit_api_secret: None,
            deepgram_api_key: Some("test_key".to_string()),
            elevenlabs_api_key: Some("test_key".to_string()),
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
            rate_limit_requests_per_second: 1000,
            rate_limit_burst_size: 100,
            max_websocket_connections: Some(1000),
            max_connections_per_ip: 500,
            ws_processing_timeout_secs: 10,
            realtime_processing_timeout_secs: 30,
            sip_max_participants: 3,
            realtime_endpoint_overrides: Default::default(),
            plugins: PluginConfig::default(),
            dag_timeouts: DAGTimeoutsConfig::default(),
            aliases: Default::default(),
        }
    }

    fn create_combined_router(state: Arc<AppState>) -> Router {
        // WebSocket and realtime routes need auth middleware to inject Auth extension
        let ws_routes = routes::ws::create_ws_router().layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

        let realtime_routes = routes::realtime::create_realtime_router().layer(
            middleware::from_fn_with_state(state.clone(), auth_middleware),
        );

        let api_routes = routes::api::create_api_router().layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

        Router::new()
            .route("/", axum::routing::get(handlers::api::health_check))
            .merge(api_routes)
            .merge(ws_routes)
            .merge(realtime_routes)
            .with_state(state)
    }

    pub async fn start_test_server() -> (SocketAddr, TestServer) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind stress test server");
        let actual_addr = listener.local_addr().expect("Failed to get address");

        let config = create_minimal_config(actual_addr.port());
        let app_state = AppState::new(config).await;
        let app = create_combined_router(app_state);

        let server = spawn_test_server(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .expect("stress test server failed");
        });

        // Wait for server to be ready
        tokio::time::sleep(Duration::from_millis(100)).await;
        (actual_addr, server)
    }

    /// Create a text message with proper conversion for tungstenite 0.28
    pub fn text_message(s: &str) -> Message {
        Message::Text(s.to_string().into())
    }

    /// Create a binary message with proper conversion for tungstenite 0.28
    pub fn binary_message(data: Vec<u8>) -> Message {
        Message::Binary(data.into())
    }
}

// =============================================================================
// Connection Stress Tests
// =============================================================================

/// Test maximum concurrent HTTP connections
#[tokio::test]
async fn test_max_concurrent_http_connections() {
    let (addr, _server) = common::start_test_server().await;
    let base_url = format!("http://{}", addr);

    let num_connections = 200;
    let successful = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    let mut handles = Vec::new();

    for _ in 0..num_connections {
        let client = client.clone();
        let url = format!("{}/", base_url);
        let successful = Arc::clone(&successful);
        let failed = Arc::clone(&failed);

        handles.push(tokio::spawn(async move {
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    successful.fetch_add(1, Ordering::Relaxed);
                }
                _ => {
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for result in join_all(handles).await {
        result.expect("HTTP stress worker should not panic");
    }

    let success_count = successful.load(Ordering::Relaxed);
    let fail_count = failed.load(Ordering::Relaxed);

    println!(
        "Concurrent HTTP connections: {} successful, {} failed",
        success_count, fail_count
    );

    // At least 90% should succeed
    assert!(
        success_count >= (num_connections * 90 / 100),
        "Too many failed connections: {} / {}",
        fail_count,
        num_connections
    );
}

/// Test maximum concurrent WebSocket connections
#[tokio::test]
async fn test_max_concurrent_websocket_connections() {
    let (addr, _server) = common::start_test_server().await;
    let ws_url = format!("ws://{}/ws", addr);

    let num_connections = 100;
    let connected = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::new();

    for _ in 0..num_connections {
        let url = ws_url.clone();
        let connected = Arc::clone(&connected);
        let failed = Arc::clone(&failed);

        handles.push(tokio::spawn(async move {
            match timeout(Duration::from_secs(10), connect_async(&url)).await {
                Ok(Ok((ws, _))) => {
                    connected.fetch_add(1, Ordering::Relaxed);
                    // Keep connection open briefly
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    drop(ws);
                }
                _ => {
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for result in join_all(handles).await {
        result.expect("WebSocket stress worker should not panic");
    }

    let connect_count = connected.load(Ordering::Relaxed);
    let fail_count = failed.load(Ordering::Relaxed);

    println!(
        "Concurrent WebSocket connections: {} successful, {} failed",
        connect_count, fail_count
    );

    // At least 80% should succeed (WebSocket is more prone to issues)
    assert!(
        connect_count >= (num_connections * 80 / 100),
        "Too many failed WebSocket connections: {} / {}",
        fail_count,
        num_connections
    );
}

/// Test rapid connect/disconnect cycles
#[tokio::test]
async fn test_rapid_connect_disconnect() {
    let (addr, _server) = common::start_test_server().await;
    let ws_url = format!("ws://{}/ws", addr);

    let cycles = 50;
    let successful_cycles = Arc::new(AtomicUsize::new(0));

    let start = Instant::now();

    for _ in 0..cycles {
        if let Ok(Ok((ws, _))) = timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
            drop(ws);
            successful_cycles.fetch_add(1, Ordering::Relaxed);
        }
        // Small delay between cycles
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let elapsed = start.elapsed();
    let success_count = successful_cycles.load(Ordering::Relaxed);

    println!(
        "Rapid connect/disconnect: {} / {} cycles in {:?}",
        success_count, cycles, elapsed
    );

    // At least 80% should succeed
    assert!(
        success_count >= (cycles * 80 / 100),
        "Too many failed cycles: {} / {}",
        cycles - success_count,
        cycles
    );
}

// =============================================================================
// Payload Stress Tests
// =============================================================================

/// Test large JSON payload handling
#[tokio::test]
async fn test_large_json_payload() {
    let (addr, _server) = common::start_test_server().await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    // Create a large text payload (1MB)
    let large_text = "A".repeat(1024 * 1024);

    let payload = json!({
        "text": large_text,
        "voice_id": "test-voice",
        "provider": "elevenlabs"
    });

    let response = client
        .post(format!("{}/speak", base_url))
        .json(&payload)
        .send()
        .await
        .expect("Large JSON payload should receive a clean HTTP response");

    println!(
        "Large JSON payload response: {} {}",
        response.status(),
        response.status().canonical_reason().unwrap_or("")
    );
    assert!(
        response.status().as_u16() < 500,
        "Server returned 5xx for large payload"
    );

    let health = client
        .get(format!("{}/", base_url))
        .send()
        .await
        .expect("Server should remain reachable after large JSON payload");
    assert!(
        health.status().is_success(),
        "Server health check failed after large JSON payload: {}",
        health.status()
    );
}

/// Test large binary WebSocket message
#[tokio::test]
async fn test_large_binary_websocket_message() {
    let (addr, _server) = common::start_test_server().await;
    let ws_url = format!("ws://{}/ws", addr);

    let (ws, _) = timeout(Duration::from_secs(10), connect_async(&ws_url))
        .await
        .expect("WebSocket connection timed out")
        .expect("WebSocket connection failed");
    let (mut write, mut read) = ws.split();

    let config = json!({
        "type": "config",
        "audio": false
    });
    write
        .send(common::text_message(&config.to_string()))
        .await
        .expect("Failed to send config");

    let ready = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timed out waiting for ready response")
        .expect("WebSocket closed before ready")
        .expect("Failed to read ready response");
    match ready {
        Message::Text(text) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&text).expect("ready response should be JSON");
            assert_eq!(
                parsed["type"].as_str(),
                Some("ready"),
                "Expected ready before large binary stress, got: {parsed}"
            );
        }
        other => panic!("Expected ready text response, got {other:?}"),
    }

    let large_audio =
        vec![0u8; waav_gateway::handlers::ws::audio_handler::MAX_AUDIO_FRAME_SIZE + 1];
    write
        .send(common::binary_message(large_audio))
        .await
        .expect("Failed to send oversized binary message");

    let oversized_response = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timed out waiting for oversized-frame response")
        .expect("WebSocket closed after oversized frame")
        .expect("Failed to read oversized-frame response");
    match oversized_response {
        Message::Text(text) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&text).expect("oversized-frame response should be JSON");
            assert_eq!(
                parsed["type"].as_str(),
                Some("error"),
                "Expected error for oversized audio frame, got: {parsed}"
            );
            let message = parsed["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("Audio frame too large"),
                "Expected oversized-audio error, got: {message}"
            );
        }
        other => panic!("Expected oversized-frame error text response, got {other:?}"),
    }

    write
        .send(common::binary_message(vec![0u8; 1600]))
        .await
        .expect("Connection should still accept a later small binary frame");

    let follow_up = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timed out waiting for follow-up binary response")
        .expect("WebSocket closed after oversized-frame recovery check")
        .expect("Failed to read follow-up binary response");
    match follow_up {
        Message::Text(text) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&text).expect("follow-up response should be JSON");
            assert_eq!(
                parsed["type"].as_str(),
                Some("error"),
                "Expected disabled-audio error after recovery send, got: {parsed}"
            );
            let message = parsed["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("Audio processing is disabled"),
                "Expected disabled-audio error after recovery send, got: {message}"
            );
        }
        other => panic!("Expected follow-up error text response, got {other:?}"),
    }
}

// =============================================================================
// Throughput Stress Tests
// =============================================================================

/// Test sustained high throughput HTTP requests
#[tokio::test]
async fn test_sustained_http_throughput() {
    let (addr, _server) = common::start_test_server().await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let duration = Duration::from_secs(10);
    let concurrency = 10;
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let request_count = Arc::new(AtomicUsize::new(0));
    let error_count = Arc::new(AtomicUsize::new(0));

    let start = Instant::now();

    let mut handles = Vec::new();

    while start.elapsed() < duration {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let url = format!("{}/", base_url);
        let request_count = Arc::clone(&request_count);
        let error_count = Arc::clone(&error_count);

        handles.push(tokio::spawn(async move {
            let _permit = permit;
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    request_count.fetch_add(1, Ordering::Relaxed);
                }
                _ => {
                    error_count.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));

        // Small delay to control rate
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    for result in join_all(handles).await {
        result.expect("sustained HTTP stress worker should not panic");
    }

    let elapsed = start.elapsed();
    let requests = request_count.load(Ordering::Relaxed);
    let errors = error_count.load(Ordering::Relaxed);
    let rps = requests as f64 / elapsed.as_secs_f64();

    println!(
        "Sustained throughput: {} requests in {:?} ({:.2} req/s), {} errors",
        requests, elapsed, rps, errors
    );

    // Should handle at least 50 req/s
    assert!(rps >= 50.0, "Throughput too low: {:.2} req/s", rps);
    // Error rate should be less than 5%
    let total = requests + errors;
    let error_rate = errors as f64 / total as f64;
    assert!(
        error_rate < 0.05,
        "Error rate too high: {:.2}%",
        error_rate * 100.0
    );
}

/// Test WebSocket message throughput
#[tokio::test]
async fn test_websocket_message_throughput() {
    let (addr, _server) = common::start_test_server().await;
    let ws_url = format!("ws://{}/ws", addr);

    let (ws, _) = timeout(Duration::from_secs(10), connect_async(&ws_url))
        .await
        .expect("WebSocket connection timed out")
        .expect("WebSocket connection failed");
    let (mut write, mut read) = ws.split();

    let config = json!({
        "type": "config",
        "audio": false
    });
    write
        .send(common::text_message(&config.to_string()))
        .await
        .expect("Failed to send config");

    let ready = timeout(Duration::from_secs(5), read.next())
        .await
        .expect("Timed out waiting for ready response")
        .expect("WebSocket closed before ready")
        .expect("Failed to read ready response");
    match ready {
        Message::Text(text) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&text).expect("ready response should be JSON");
            assert_eq!(
                parsed["type"].as_str(),
                Some("ready"),
                "Expected ready before throughput stress, got: {parsed}"
            );
        }
        other => panic!("Expected ready text response, got {other:?}"),
    }

    let message_count = 200;
    let start = Instant::now();
    for _ in 0..message_count {
        let audio_chunk = vec![0u8; 3200]; // 100ms at 16kHz
        write
            .send(common::binary_message(audio_chunk))
            .await
            .expect("Failed to send WebSocket binary throughput frame");
    }

    let elapsed = start.elapsed();
    let mps = message_count as f64 / elapsed.as_secs_f64();

    println!(
        "WebSocket throughput: {} messages in {:?} ({:.2} msg/s)",
        message_count, elapsed, mps
    );

    assert!(mps >= 100.0, "Message throughput too low: {:.2} msg/s", mps);

    let mut responses = 0;
    while responses < message_count {
        let msg = timeout(Duration::from_secs(5), read.next())
            .await
            .expect("Timed out waiting for throughput response")
            .expect("WebSocket closed during throughput response drain")
            .expect("Failed to read throughput response");
        match msg {
            Message::Text(text) => {
                let parsed: serde_json::Value =
                    serde_json::from_str(&text).expect("throughput response should be JSON");
                assert_eq!(
                    parsed["type"].as_str(),
                    Some("error"),
                    "Expected disabled-audio error while draining throughput response, got: {parsed}"
                );
                let message = parsed["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains("Audio processing is disabled"),
                    "Expected disabled-audio error while draining throughput response, got: {message}"
                );
                responses += 1;
            }
            other => panic!("Expected throughput response text frame, got {other:?}"),
        }
    }
}

// =============================================================================
// Resource Stress Tests
// =============================================================================

/// Test memory stability under load
#[tokio::test]
async fn test_memory_stability_under_load() {
    let (addr, _server) = common::start_test_server().await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    // Make many requests and ensure server stays responsive
    let iterations = 500;
    let mut success_count = 0;

    let start = Instant::now();

    for i in 0..iterations {
        match client.get(format!("{}/", base_url)).send().await {
            Ok(resp) if resp.status().is_success() => {
                success_count += 1;
            }
            _ => {}
        }

        // Check latency doesn't degrade significantly
        if i > 0 && i % 100 == 0 {
            let elapsed = start.elapsed();
            let avg_latency = elapsed.as_millis() as f64 / i as f64;
            println!("After {} requests: avg latency {:.2}ms", i, avg_latency);

            // Average latency should stay under 100ms
            assert!(
                avg_latency < 100.0,
                "Latency degraded: {:.2}ms after {} requests",
                avg_latency,
                i
            );
        }
    }

    println!(
        "Memory stability test: {} / {} successful",
        success_count, iterations
    );

    // At least 95% should succeed
    assert!(
        success_count >= (iterations * 95 / 100),
        "Too many failures: {} / {}",
        iterations - success_count,
        iterations
    );
}

/// Test handling of connection exhaustion
#[tokio::test]
async fn test_connection_exhaustion_recovery() {
    let (addr, _server) = common::start_test_server().await;
    let base_url = format!("http://{}", addr);
    let ws_url = format!("ws://{}/ws", addr);

    // Open many WebSocket connections
    let num_connections = 50;
    let mut connections = Vec::new();
    let mut failed_connections = 0;

    for _ in 0..num_connections {
        match timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
            Ok(Ok((ws, _))) => {
                connections.push(ws);
            }
            _ => {
                failed_connections += 1;
            }
        }
    }

    println!(
        "Opened {} / {} WebSocket connections ({} failed)",
        connections.len(),
        num_connections,
        failed_connections
    );

    assert!(
        connections.len() >= (num_connections * 80 / 100),
        "Connection exhaustion test did not hold enough WebSocket connections: opened {} / {}, failed {}",
        connections.len(),
        num_connections,
        failed_connections
    );

    // Server should still respond to HTTP requests
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let response = client.get(format!("{}/", base_url)).send().await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server unresponsive while connections are held"
    );

    // Close all connections
    drop(connections);

    // Wait for cleanup
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Server should still be responsive after closing connections
    let response = client.get(format!("{}/", base_url)).send().await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server unresponsive after closing connections"
    );
}

// =============================================================================
// Validation Stress Tests
// =============================================================================

/// Test handling of many invalid requests
#[tokio::test]
async fn test_invalid_request_flood() {
    let (addr, _server) = common::start_test_server().await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let iterations = 100;
    let mut proper_rejection_count = 0;

    // Send many invalid requests
    for i in 0..iterations {
        // Invalid JSON
        let response = client
            .post(format!("{}/speak", base_url))
            .header("Content-Type", "application/json")
            .body("{ invalid json }")
            .send()
            .await
            .unwrap_or_else(|e| {
                panic!("Invalid request {i} should receive a clean HTTP response: {e}")
            });

        // Should be rejected with 4xx, not 5xx or transport failure.
        assert!(
            response.status().is_client_error(),
            "Invalid request {i} should be rejected with 4xx, got {}",
            response.status()
        );
        proper_rejection_count += 1;
    }

    println!(
        "Invalid request handling: {} / {} properly rejected",
        proper_rejection_count, iterations
    );

    assert_eq!(
        proper_rejection_count, iterations,
        "Server did not reject every invalid request cleanly"
    );

    // Server should still be responsive
    let response = client
        .get(format!("{}/", base_url))
        .send()
        .await
        .expect("Server should remain reachable after invalid request flood");
    assert!(
        response.status().is_success(),
        "Server health check failed after invalid request flood: {}",
        response.status()
    );
}

/// Test malformed WebSocket messages
#[tokio::test]
async fn test_malformed_websocket_messages() {
    let (addr, _server) = common::start_test_server().await;
    let ws_url = format!("ws://{}/ws", addr);

    match timeout(Duration::from_secs(10), connect_async(&ws_url)).await {
        Ok(Ok((ws, _))) => {
            let (mut write, mut read) = ws.split();

            // Send various malformed messages
            let malformed_messages = vec![
                ("not json at all", "Invalid message format"),
                ("{ broken json", "Invalid message format"),
                (r#"{"type": "unknown_type"}"#, "Invalid message format"),
                (r#"{"type": "config"}"#, "STT configuration is required"),
                (
                    r#"{"type": "config", "stt_config": null}"#,
                    "STT configuration is required",
                ),
            ];

            for (msg, expected_error) in malformed_messages {
                write
                    .send(common::text_message(msg))
                    .await
                    .expect("Malformed message should be accepted for protocol rejection");

                let response = timeout(Duration::from_secs(2), read.next())
                    .await
                    .expect("Timed out waiting for malformed-message error")
                    .expect("WebSocket closed before malformed-message error")
                    .expect("Failed to read malformed-message error");
                match response {
                    Message::Text(text) => {
                        let parsed: serde_json::Value =
                            serde_json::from_str(&text).expect("error response should be JSON");
                        assert_eq!(
                            parsed["type"].as_str(),
                            Some("error"),
                            "Expected error for malformed message {msg:?}, got: {parsed}"
                        );
                        let message = parsed["message"].as_str().unwrap_or_default();
                        assert!(
                            message.contains(expected_error),
                            "Expected malformed-message error to contain {expected_error:?}, got: {message}"
                        );
                    }
                    other => panic!("Expected malformed-message error text frame, got {other:?}"),
                }
            }

            println!("Malformed WebSocket messages were rejected with protocol errors");

            let config = json!({
                "type": "config",
                "audio": false
            });
            write
                .send(common::text_message(&config.to_string()))
                .await
                .expect("Connection should accept valid config after malformed messages");

            let ready = timeout(Duration::from_secs(5), read.next())
                .await
                .expect("Timed out waiting for ready after malformed messages")
                .expect("WebSocket closed before ready after malformed messages")
                .expect("Failed to read ready after malformed messages");
            match ready {
                Message::Text(text) => {
                    let parsed: serde_json::Value =
                        serde_json::from_str(&text).expect("ready response should be JSON");
                    assert_eq!(
                        parsed["type"].as_str(),
                        Some("ready"),
                        "Expected ready after malformed-message recovery, got: {parsed}"
                    );
                }
                other => {
                    panic!("Expected ready text response after malformed messages, got {other:?}")
                }
            }
        }
        Ok(Err(e)) => {
            panic!("WebSocket connection error: {}", e);
        }
        Err(_) => {
            panic!("WebSocket connection timeout");
        }
    }

    // Server should still be responsive for new connections
    if let Ok(Ok(_)) = timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
        println!("Server still accepts new connections after malformed messages");
    } else {
        panic!("Server not accepting new connections after malformed messages");
    }
}

// =============================================================================
// Concurrent Operation Stress Tests
// =============================================================================

/// Test mixed concurrent operations
#[tokio::test]
async fn test_mixed_concurrent_operations() {
    let (addr, _server) = common::start_test_server().await;
    let base_url = format!("http://{}", addr);
    let ws_url = format!("ws://{}/ws", addr);

    let duration = Duration::from_secs(10);
    let http_requests = Arc::new(AtomicUsize::new(0));
    let ws_connections = Arc::new(AtomicUsize::new(0));
    let errors = Arc::new(AtomicUsize::new(0));

    let start = Instant::now();
    let mut handles = Vec::new();

    // HTTP request workers
    for _ in 0..5 {
        let base_url = base_url.clone();
        let http_requests = Arc::clone(&http_requests);
        let errors = Arc::clone(&errors);

        handles.push(tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();

            while Instant::now().duration_since(start) < duration {
                match client.get(format!("{}/", base_url)).send().await {
                    Ok(resp) if resp.status().is_success() => {
                        http_requests.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }));
    }

    // WebSocket connection workers
    for _ in 0..3 {
        let ws_url = ws_url.clone();
        let ws_connections = Arc::clone(&ws_connections);
        let errors = Arc::clone(&errors);

        handles.push(tokio::spawn(async move {
            while Instant::now().duration_since(start) < duration {
                match timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
                    Ok(Ok((ws, _))) => {
                        ws_connections.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        drop(ws);
                    }
                    _ => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }));
    }

    for result in join_all(handles).await {
        result.expect("mixed operation stress worker should not panic");
    }

    let http_count = http_requests.load(Ordering::Relaxed);
    let ws_count = ws_connections.load(Ordering::Relaxed);
    let error_count = errors.load(Ordering::Relaxed);

    println!(
        "Mixed operations: {} HTTP requests, {} WS connections, {} errors",
        http_count, ws_count, error_count
    );

    // Error rate should be less than 10%
    let total = http_count + ws_count + error_count;
    let error_rate = error_count as f64 / total as f64;
    assert!(
        error_rate < 0.10,
        "Error rate too high: {:.2}%",
        error_rate * 100.0
    );
}
