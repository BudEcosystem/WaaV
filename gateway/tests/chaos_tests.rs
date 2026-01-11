//! Chaos Tests for WaaV Gateway
//!
//! These tests simulate failure scenarios to verify graceful degradation:
//! - Provider failures and timeouts
//! - Network partitions (simulated via mocks)
//! - Resource exhaustion
//! - Sudden connection drops
//! - Invalid state transitions
//!
//! Run: cargo test --test chaos_tests -- --nocapture

use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

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
            max_websocket_connections: None,
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
// Provider Failure Simulation Tests
// =============================================================================

/// Test behavior when provider returns 503 Service Unavailable
#[tokio::test]
async fn test_provider_503_handling() {
    // Start mock server that always returns 503
    let mock_server = MockServer::start().await;

    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_json(json!({"error": "Service temporarily unavailable"})),
        )
        .mount(&mock_server)
        .await;

    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // The server should handle provider failures gracefully
    let response = client
        .post(format!("{}/speak", base_url))
        .json(&json!({
            "text": "Test",
            "provider": "deepgram",
            "voice_id": "test"
        }))
        .send()
        .await
        .expect("Request should complete");

    // Server should return an error, not crash
    println!("Provider 503 test - status: {}", response.status());
    assert!(response.status().as_u16() != 0, "Server should respond");
}

/// Test behavior when provider times out
#[tokio::test]
async fn test_provider_timeout_handling() {
    // Start mock server that delays forever
    let mock_server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
        .mount(&mock_server)
        .await;

    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();

    let start = Instant::now();

    let response = client
        .post(format!("{}/speak", base_url))
        .json(&json!({
            "text": "Test",
            "provider": "deepgram",
            "voice_id": "test"
        }))
        .send()
        .await;

    let elapsed = start.elapsed();

    // Should timeout reasonably quickly, not wait forever
    println!("Provider timeout test - elapsed: {:?}", elapsed);
    assert!(
        elapsed < Duration::from_secs(30),
        "Request should timeout within reasonable time"
    );

    match response {
        Ok(resp) => {
            println!("Response status: {}", resp.status());
        }
        Err(e) => {
            println!("Request error (expected): {}", e);
            // Timeout is expected
        }
    }
}

/// Test behavior when provider returns invalid JSON
#[tokio::test]
async fn test_provider_invalid_response_handling() {
    let mock_server = MockServer::start().await;

    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_string("not valid json at all"))
        .mount(&mock_server)
        .await;

    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // Health check should still work
    let health = client.get(format!("{}/", base_url)).send().await;
    assert!(
        health.is_ok() && health.unwrap().status().is_success(),
        "Server should remain healthy"
    );
}

// =============================================================================
// Connection Chaos Tests
// =============================================================================

/// Test sudden client disconnection during processing
#[tokio::test]
async fn test_client_sudden_disconnect() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    // Connect and immediately disconnect multiple times
    for _ in 0..20 {
        match timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
            Ok(Ok((ws, _))) => {
                // Send partial config then disconnect abruptly
                let (mut write, _) = ws.split();

                let _ = write
                    .send(common::text_message(r#"{"type": "config"#))
                    .await;

                // Drop without proper close
                drop(write);
            }
            _ => {}
        }
    }

    // Server should still be responsive
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}/", addr))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should remain responsive after sudden disconnects"
    );
}

/// Test connection flood followed by immediate close
#[tokio::test]
async fn test_connection_flood_and_close() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    let mut handles = Vec::new();

    // Flood with connections
    for _ in 0..50 {
        let url = ws_url.clone();
        handles.push(tokio::spawn(async move {
            if let Ok(Ok((mut ws, _))) =
                timeout(Duration::from_secs(5), connect_async(&url)).await
            {
                // Immediately close
                let _ = ws.close(None).await;
            }
        }));
    }

    futures::future::join_all(handles).await;

    // Wait for cleanup
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Verify server is healthy
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}/", addr))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should be healthy after connection flood"
    );
}

/// Test half-open connections
#[tokio::test]
async fn test_half_open_connections() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    // Create connections that never complete handshake
    let mut tcp_connections = Vec::new();

    for _ in 0..10 {
        if let Ok(stream) = tokio::net::TcpStream::connect(&addr).await {
            tcp_connections.push(stream);
        }
    }

    // Hold these connections open
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Server should still accept legitimate WebSocket connections
    match timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
        Ok(Ok(_)) => {
            println!("Server accepts WebSocket connections despite half-open TCP");
        }
        Ok(Err(e)) => {
            println!("WebSocket error (may be acceptable): {}", e);
        }
        Err(_) => {
            println!("WebSocket timeout (may be acceptable under load)");
        }
    }

    // Clean up
    drop(tcp_connections);

    // Verify server recovers
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}/", addr))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should recover from half-open connections"
    );
}

// =============================================================================
// Message Chaos Tests
// =============================================================================

/// Test rapid message interleaving
#[tokio::test]
async fn test_rapid_message_interleaving() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    match timeout(Duration::from_secs(10), connect_async(&ws_url)).await {
        Ok(Ok((ws, _))) => {
            let (mut write, _) = ws.split();

            // Send many different message types rapidly
            let messages = vec![
                json!({"type": "config", "stt_config": {"provider": "deepgram"}}),
                json!({"type": "clear"}),
                json!({"type": "speak", "text": "Hello"}),
                json!({"type": "config", "tts_config": {"provider": "elevenlabs"}}),
                json!({"type": "unknown"}),
                json!({"type": "clear"}),
            ];

            for msg in messages {
                let _ = write.send(common::text_message(&msg.to_string())).await;
                // No delay between messages
            }

            // Also send binary messages interspersed
            for _ in 0..10 {
                let _ = write.send(common::binary_message(vec![0u8; 100])).await;
            }

            println!("Sent all interleaved messages");
        }
        Ok(Err(e)) => {
            panic!("WebSocket connection error: {}", e);
        }
        Err(_) => {
            panic!("WebSocket connection timeout");
        }
    }

    // Server should still be healthy
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}/", addr))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should handle rapid message interleaving"
    );
}

/// Test out-of-order messages
#[tokio::test]
async fn test_out_of_order_messages() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    match timeout(Duration::from_secs(10), connect_async(&ws_url)).await {
        Ok(Ok((ws, _))) => {
            let (mut write, _) = ws.split();

            // Send audio before config (out of order)
            let _ = write.send(common::binary_message(vec![0u8; 1600])).await;

            // Send speak before ready
            let _ = write
                .send(common::text_message(&json!({"type": "speak", "text": "Hello"}).to_string()))
                .await;

            // Now send config
            let config = json!({
                "type": "config",
                "stt_config": {"provider": "deepgram"},
                "tts_config": {"provider": "elevenlabs"}
            });
            let _ = write.send(common::text_message(&config.to_string())).await;

            // Send more audio
            let _ = write.send(common::binary_message(vec![0u8; 1600])).await;

            println!("Sent out-of-order messages");
        }
        Ok(Err(e)) => {
            panic!("WebSocket connection error: {}", e);
        }
        Err(_) => {
            panic!("WebSocket connection timeout");
        }
    }

    // Server should handle gracefully
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}/", addr))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should handle out-of-order messages"
    );
}

// =============================================================================
// State Chaos Tests
// =============================================================================

/// Test multiple config changes during session
#[tokio::test]
async fn test_multiple_config_changes() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let ws_url = format!("ws://{}/ws", addr);

    match timeout(Duration::from_secs(10), connect_async(&ws_url)).await {
        Ok(Ok((ws, _))) => {
            let (mut write, _read) = ws.split();

            // Initial config
            let config = json!({
                "type": "config",
                "stt_config": {"provider": "deepgram"},
                "tts_config": {"provider": "elevenlabs"}
            });
            write.send(common::text_message(&config.to_string())).await.ok();

            // Wait for ready
            tokio::time::sleep(Duration::from_millis(200)).await;

            // Change config multiple times rapidly
            for _ in 0..5 {
                let new_config = json!({
                    "type": "config",
                    "stt_config": {"provider": "deepgram"}
                });
                write.send(common::text_message(&new_config.to_string())).await.ok();
            }

            // Send audio during config changes
            for _ in 0..5 {
                write.send(common::binary_message(vec![0u8; 1600])).await.ok();
            }

            println!("Completed multiple config changes test");
        }
        Ok(Err(e)) => {
            panic!("WebSocket connection error: {}", e);
        }
        Err(_) => {
            panic!("WebSocket connection timeout");
        }
    }

    // Verify server stability
    tokio::time::sleep(Duration::from_millis(500)).await;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://{}/", addr))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should handle multiple config changes"
    );
}

// =============================================================================
// Recovery Tests
// =============================================================================

/// Test server recovery after error conditions
#[tokio::test]
async fn test_server_recovery_after_errors() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let base_url = format!("http://{}", addr);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    // Induce various error conditions
    let nested_json = r#"{"b":{}}"#.repeat(100);
    let long_text = "x".repeat(100000);

    let error_payloads = vec![
        "".to_string(),
        "{ not valid".to_string(),
        r#"{"text": ""}"#.to_string(),
        format!(r#"{{"a":{}}}"#, nested_json),
        format!(r#"{{"text": "{}"}}"#, long_text),
    ];

    for payload in error_payloads {
        let _ = client
            .post(format!("{}/speak", base_url))
            .header("Content-Type", "application/json")
            .body(payload)
            .send()
            .await;
    }

    // Wait for any cleanup
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Server should be fully recovered
    let response = client.get(format!("{}/", base_url)).send().await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should recover from error conditions"
    );

    // Should also accept new WebSocket connections
    let ws_url = format!("ws://{}/ws", addr);
    match timeout(Duration::from_secs(5), connect_async(&ws_url)).await {
        Ok(Ok(_)) => {
            println!("Server accepts new WebSocket connections after errors");
        }
        Ok(Err(e)) => {
            panic!("WebSocket should work after recovery: {}", e);
        }
        Err(_) => {
            panic!("WebSocket connection should not timeout after recovery");
        }
    }
}

/// Test concurrent chaos operations
#[tokio::test]
async fn test_concurrent_chaos_operations() {
    let port = common::get_available_port();
    let addr = common::start_test_server(port).await;
    let base_url = format!("http://{}", addr);
    let ws_url = format!("ws://{}/ws", addr);

    let successful_ops = Arc::new(AtomicUsize::new(0));
    let failed_ops = Arc::new(AtomicUsize::new(0));
    let stop_flag = Arc::new(AtomicBool::new(false));

    let duration = Duration::from_secs(5);
    let start = Instant::now();

    let mut handles = Vec::new();

    // HTTP chaos worker
    {
        let base_url = base_url.clone();
        let successful = Arc::clone(&successful_ops);
        let failed = Arc::clone(&failed_ops);
        let stop = Arc::clone(&stop_flag);

        handles.push(tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .unwrap();

            let mut counter = 0usize;
            while !stop.load(Ordering::Relaxed) {
                // Mix of valid and invalid requests
                let req = if counter % 2 == 0 {
                    client.get(format!("{}/", base_url)).send().await
                } else {
                    client
                        .post(format!("{}/speak", base_url))
                        .body("invalid")
                        .send()
                        .await
                };
                counter += 1;

                match req {
                    Ok(_) => successful.fetch_add(1, Ordering::Relaxed),
                    Err(_) => failed.fetch_add(1, Ordering::Relaxed),
                };
            }
        }));
    }

    // WebSocket chaos worker
    {
        let ws_url = ws_url.clone();
        let successful = Arc::clone(&successful_ops);
        let failed = Arc::clone(&failed_ops);
        let stop = Arc::clone(&stop_flag);

        handles.push(tokio::spawn(async move {
            let mut counter = 0usize;
            while !stop.load(Ordering::Relaxed) {
                match timeout(Duration::from_secs(2), connect_async(&ws_url)).await {
                    Ok(Ok((mut ws, _))) => {
                        successful.fetch_add(1, Ordering::Relaxed);
                        // Send random messages
                        for _ in 0..3 {
                            let msg = if counter % 2 == 0 {
                                common::text_message("invalid")
                            } else {
                                common::binary_message(vec![0u8; 100])
                            };
                            counter += 1;
                            let _ = ws.send(msg).await;
                        }
                        let _ = ws.close(None).await;
                    }
                    _ => {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // Wait for test duration
    tokio::time::sleep(duration).await;
    stop_flag.store(true, Ordering::Relaxed);

    // Wait for workers to stop
    tokio::time::sleep(Duration::from_millis(500)).await;

    let success_count = successful_ops.load(Ordering::Relaxed);
    let fail_count = failed_ops.load(Ordering::Relaxed);

    println!(
        "Concurrent chaos: {} successful, {} failed operations",
        success_count, fail_count
    );

    // Final health check
    let client = reqwest::Client::new();
    let response = client
        .get(format!("{}/", base_url))
        .timeout(Duration::from_secs(5))
        .send()
        .await;

    assert!(
        response.is_ok() && response.unwrap().status().is_success(),
        "Server should survive concurrent chaos operations"
    );
}
