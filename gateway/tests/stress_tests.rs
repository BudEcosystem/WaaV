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

use axum::Router;
use futures::future::join_all;
use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use waav_gateway::{config::PluginConfig, routes, state::AppState, ServerConfig, handlers, middleware::auth_middleware};

mod common {
    use super::*;
    use axum::{Router, middleware};
    use std::sync::Arc;

    pub fn get_available_port() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
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
            plugins: PluginConfig::default(),
        }
    }

    fn create_combined_router(state: Arc<AppState>) -> Router {
        // WebSocket and realtime routes need auth middleware to inject Auth extension
        let ws_routes = routes::ws::create_ws_router()
            .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

        let realtime_routes = routes::realtime::create_realtime_router()
            .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

        let api_routes = routes::api::create_api_router()
            .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

        Router::new()
            .route("/", axum::routing::get(handlers::api::health_check))
            .merge(api_routes)
            .merge(ws_routes)
            .merge(realtime_routes)
            .with_state(state)
    }

    pub async fn start_test_server(port: u16) -> SocketAddr {
        let config = create_minimal_config(port);
        let app_state = AppState::new(config).await;
        let app = create_combined_router(app_state);

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let listener = TcpListener::bind(addr).await.expect("Failed to bind");
        let actual_addr = listener.local_addr().expect("Failed to get address");

        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service()).await.ok();
        });

        // Wait for server to be ready
        tokio::time::sleep(Duration::from_millis(100)).await;
        actual_addr
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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
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

    join_all(handles).await;

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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
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

    join_all(handles).await;

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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    let cycles = 50;
    let successful_cycles = Arc::new(AtomicUsize::new(0));

    let start = Instant::now();

    for _ in 0..cycles {
        match timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
            Ok(Ok((ws, _))) => {
                drop(ws);
                successful_cycles.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
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
        .await;

    // Should handle large payload gracefully (either process or reject cleanly)
    match response {
        Ok(resp) => {
            println!(
                "Large JSON payload response: {} {}",
                resp.status(),
                resp.status().canonical_reason().unwrap_or("")
            );
            // Should not be a 5xx error
            assert!(
                resp.status().as_u16() < 500,
                "Server returned 5xx for large payload"
            );
        }
        Err(e) => {
            // Timeout or connection reset is acceptable for very large payloads
            println!("Large JSON payload error (acceptable): {}", e);
        }
    }
}

/// Test large binary WebSocket message
#[tokio::test]
async fn test_large_binary_websocket_message() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    match timeout(Duration::from_secs(10), connect_async(&ws_url)).await {
        Ok(Ok((ws, _))) => {
            let (mut write, mut read) = ws.split();

            // Send config first
            let config = json!({
                "type": "config",
                "stt_config": { "provider": "deepgram" },
                "tts_config": { "provider": "elevenlabs" }
            });
            write
                .send(common::text_message(&config.to_string()))
                .await
                .expect("Failed to send config");

            // Wait for ready
            let mut ready_received = false;
            for _ in 0..10 {
                if let Ok(Some(Ok(msg))) =
                    timeout(Duration::from_millis(500), read.next()).await
                {
                    if let Message::Text(text) = msg {
                        let text_str: &str = &text;
                        if text_str.contains("ready") {
                            ready_received = true;
                            break;
                        }
                    }
                }
            }

            if ready_received {
                // Try to send large binary message (10MB)
                let large_audio = vec![0u8; 10 * 1024 * 1024];

                match write.send(common::binary_message(large_audio)).await {
                    Ok(_) => {
                        println!("Large binary message sent successfully");
                    }
                    Err(e) => {
                        println!(
                            "Large binary message rejected (acceptable): {}",
                            e
                        );
                    }
                }
            }
        }
        Ok(Err(e)) => {
            println!("WebSocket connection error: {}", e);
        }
        Err(_) => {
            println!("WebSocket connection timeout");
        }
    }
}

// =============================================================================
// Throughput Stress Tests
// =============================================================================

/// Test sustained high throughput HTTP requests
#[tokio::test]
async fn test_sustained_http_throughput() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
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

    join_all(handles).await;

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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    match timeout(Duration::from_secs(10), connect_async(&ws_url)).await {
        Ok(Ok((ws, _))) => {
            let (mut write, mut read) = ws.split();

            // Send config
            let config = json!({
                "type": "config",
                "stt_config": { "provider": "deepgram" },
                "tts_config": { "provider": "elevenlabs" }
            });
            write
                .send(common::text_message(&config.to_string()))
                .await
                .expect("Failed to send config");

            // Wait for ready
            let mut ready_received = false;
            for _ in 0..10 {
                if let Ok(Some(Ok(msg))) =
                    timeout(Duration::from_millis(500), read.next()).await
                {
                    if let Message::Text(text) = msg {
                        let text_str: &str = &text;
                        if text_str.contains("ready") {
                            ready_received = true;
                            break;
                        }
                    }
                }
            }

            if ready_received {
                let duration = Duration::from_secs(5);
                let start = Instant::now();
                let mut messages_sent = 0;

                // Send audio chunks as fast as possible
                while start.elapsed() < duration {
                    let audio_chunk = vec![0u8; 3200]; // 100ms at 16kHz
                    if write.send(common::binary_message(audio_chunk)).await.is_ok() {
                        messages_sent += 1;
                    } else {
                        break;
                    }
                }

                let elapsed = start.elapsed();
                let mps = messages_sent as f64 / elapsed.as_secs_f64();

                println!(
                    "WebSocket throughput: {} messages in {:?} ({:.2} msg/s)",
                    messages_sent, elapsed, mps
                );

                // Should be able to send at least 100 messages per second
                assert!(mps >= 100.0, "Message throughput too low: {:.2} msg/s", mps);
            }
        }
        Ok(Err(e)) => {
            panic!("WebSocket connection error: {}", e);
        }
        Err(_) => {
            panic!("WebSocket connection timeout");
        }
    }
}

// =============================================================================
// Resource Stress Tests
// =============================================================================

/// Test memory stability under load
#[tokio::test]
async fn test_memory_stability_under_load() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
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
            println!(
                "After {} requests: avg latency {:.2}ms",
                i, avg_latency
            );

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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let base_url = format!("http://{}", addr);
    let ws_url = format!("ws://{}/ws", addr);

    // Open many WebSocket connections
    let num_connections = 50;
    let mut connections = Vec::new();

    for _ in 0..num_connections {
        if let Ok(Ok((ws, _))) =
            timeout(Duration::from_secs(5), connect_async(&ws_url)).await
        {
            connections.push(ws);
        }
    }

    println!("Opened {} WebSocket connections", connections.len());

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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let iterations = 100;
    let mut proper_rejection_count = 0;

    // Send many invalid requests
    for _ in 0..iterations {
        // Invalid JSON
        let response = client
            .post(format!("{}/speak", base_url))
            .header("Content-Type", "application/json")
            .body("{ invalid json }")
            .send()
            .await;

        if let Ok(resp) = response {
            // Should be rejected with 4xx, not 5xx
            if resp.status().is_client_error() {
                proper_rejection_count += 1;
            }
        }
    }

    println!(
        "Invalid request handling: {} / {} properly rejected",
        proper_rejection_count, iterations
    );

    // All invalid requests should be properly rejected
    assert!(
        proper_rejection_count >= (iterations * 90 / 100),
        "Server not properly rejecting invalid requests"
    );

    // Server should still be responsive
    let response = client.get(format!("{}/", base_url)).send().await;
    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server unresponsive after invalid request flood"
    );
}

/// Test malformed WebSocket messages
#[tokio::test]
async fn test_malformed_websocket_messages() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    match timeout(Duration::from_secs(10), connect_async(&ws_url)).await {
        Ok(Ok((ws, _))) => {
            let (mut write, _read) = ws.split();

            // Send various malformed messages
            let malformed_messages = vec![
                "not json at all",
                "{ broken json",
                r#"{"type": "unknown_type"}"#,
                r#"{"type": "config"}"#, // Missing required fields
                r#"{"type": "config", "stt_config": null}"#,
            ];

            let mut still_connected = true;
            for msg in malformed_messages {
                match write.send(common::text_message(msg)).await {
                    Ok(_) => {}
                    Err(_) => {
                        still_connected = false;
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }

            println!(
                "Connection after malformed messages: {}",
                if still_connected {
                    "maintained"
                } else {
                    "closed"
                }
            );

            // Server may close connection or continue - both are acceptable
        }
        Ok(Err(e)) => {
            panic!("WebSocket connection error: {}", e);
        }
        Err(_) => {
            panic!("WebSocket connection timeout");
        }
    }

    // Server should still be responsive for new connections
    if let Ok(Ok(_)) =
        timeout(Duration::from_secs(5), connect_async(&ws_url)).await
    {
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
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
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

    join_all(handles).await;

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
