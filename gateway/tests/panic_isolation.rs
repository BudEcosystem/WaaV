//! W-E1 / E6: multi-tenant panic isolation.
//!
//! Under `panic = "abort"` a single panic — e.g. triggered by one malformed WS
//! frame in one session — aborts the entire process, taking down every other
//! tenant. The release profile is now `panic = "unwind"` and each per-session WS
//! task body is wrapped in `catch_unwind` (see `src/handlers/ws/handler.rs`), so a
//! panic is contained to the one offending session.
//!
//! `fuzzed_first_message_does_not_abort` proves this end to end: a first
//! connection sends a frame that panics inside the session body; under abort this
//! would kill the test process. Under unwind + catch the process survives and the
//! server still serves a subsequent healthy connection.

use futures::{SinkExt, StreamExt};
use serde_json::json;
use std::io::ErrorKind;

use axum::middleware;
use serial_test::serial;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    middleware::auth::auth_middleware,
    routes,
    state::AppState,
};

fn test_config() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        livekit_api_key: Some("test_key".to_string()),
        livekit_api_secret: Some("test_secret".to_string()),
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
        rate_limit_burst_size: 1000,
        max_websocket_connections: None,
        max_connections_per_ip: 1000,
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
    }
}

/// A first connection whose frame panics inside the session body must NOT bring
/// down the process; the server must still serve a subsequent healthy connection.
#[tokio::test]
#[serial]
async fn fuzzed_first_message_does_not_abort() {
    // Arm the panic-injection seam: any text frame starting with this token
    // panics inside `process_message` (the per-session task body).
    const PANIC_TOKEN: &str = "__WAAV_PANIC__";
    // SAFETY: single-threaded test setup before the server task is spawned;
    // guarded by #[serial] so no other test reads/writes this env concurrently.
    unsafe {
        std::env::set_var("WAAV_TEST_PANIC_ON_TEXT", PANIC_TOKEN);
    }

    let app_state = AppState::new(test_config()).await;
    let ws_routes = routes::ws::create_ws_router().layer(middleware::from_fn_with_state(
        app_state.clone(),
        auth_middleware,
    ));
    let app = routes::api::create_api_router()
        .merge(ws_routes)
        .with_state(app_state);

    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            eprintln!("Skipping fuzzed_first_message_does_not_abort: {err}");
            unsafe {
                std::env::remove_var("WAAV_TEST_PANIC_ON_TEXT");
            }
            return;
        }
        Err(err) => panic!("bind failed: {err}"),
    };
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let url = format!("ws://127.0.0.1:{}/ws", addr.port());

    // ---- Connection 1: send the panic-triggering ("malformed") first frame ----
    {
        let (ws_stream, _) = connect_async(&url)
            .await
            .expect("first connect should succeed");
        let (mut write, mut read) = ws_stream.split();

        write
            .send(Message::Text(
                format!("{PANIC_TOKEN} fuzzed garbage frame").into(),
            ))
            .await
            .expect("sending the panic frame should not fail on the client side");

        // The session task panics and is contained; the socket is torn down. The
        // client observes the connection ending (Err / Close / stream end). What
        // matters is that the *process* survives — verified by connection 2.
        let _ = read.next().await;
    }

    // The panic seam has done its job; disarm it so the healthy connection is not
    // affected if it happens to send any text.
    unsafe {
        std::env::remove_var("WAAV_TEST_PANIC_ON_TEXT");
    }

    // ---- Connection 2: a fresh, healthy connection must still be served ----
    // If the process had aborted (panic=abort), the test binary would already be
    // dead and we would never reach here. Reaching here AND getting a valid
    // protocol response proves isolation held.
    let (ws_stream, _) = connect_async(&url)
        .await
        .expect("server must still accept connections after the panicked session");
    let (mut write, mut read) = ws_stream.split();

    let config_message = json!({
        "type": "config",
        "stt_config": {
            "provider": "deepgram",
            "api_key": "test_key",
            "language": "en-US",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true
        },
        "tts_config": {
            "provider": "deepgram",
            "api_key": "test_key",
            "voice_id": "aura-luna-en",
            "speaking_rate": 1.0,
            "audio_format": "pcm",
            "sample_rate": 22050,
            "connection_timeout": 30,
            "request_timeout": 60
        }
    });
    write
        .send(Message::Text(config_message.to_string().into()))
        .await
        .expect("healthy connection should accept a config frame");

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), read.next())
        .await
        .expect("server should respond to the healthy connection")
        .expect("stream should yield a message")
        .expect("message should be Ok");

    match response {
        Message::Text(text) => {
            let parsed: serde_json::Value =
                serde_json::from_str(&text).expect("response should be JSON");
            let msg_type = parsed["type"].as_str().unwrap_or("");
            // "ready" (config accepted) or "error" (test keys rejected) both prove
            // the server is alive and processing protocol messages normally.
            assert!(
                msg_type == "ready" || msg_type == "error",
                "expected a normal protocol response after isolation, got: {msg_type}"
            );
        }
        other => panic!("expected a text protocol response, got: {other:?}"),
    }

    server.abort();
}
