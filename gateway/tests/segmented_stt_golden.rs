//! Golden recording of a streaming session (plan Release 0): a session on a model that streams
//! takes exactly today's path, whether or not the segmented speech-to-text switch covers it. The
//! only difference the switch may make is the additive `ready.stt` key.
//!
//! The session runs twice against the same mock Deepgram socket, uncovered and covered (Release 2,
//! where only covered sessions get `ready.stt`); both must match the committed recording in
//! `tests/golden/deepgram_streaming_session.json` once `ready.stt` is set aside. Regenerate the
//! recording with `WAAV_UPDATE_GOLDEN=1` after a deliberate change, and review the diff.

mod mock_providers;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::middleware;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use mock_providers::websocket_mock::{WebSocketMockState, start_stt_websocket_mock};
use mock_providers::{ChaosConfig, LatencyProfile};
use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    middleware::auth::auth_middleware,
    routes,
    state::AppState,
};

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/deepgram_streaming_session.json"
);

fn instant() -> LatencyProfile {
    LatencyProfile {
        min_ms: 0,
        max_ms: 0,
        p50_ms: 0,
        p99_ms: 0,
    }
}

fn server_config() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        livekit_api_key: Some("test_key".to_string()),
        livekit_api_secret: Some("test_key".to_string()),
        port: 0, // Let the OS assign a port
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
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
        max_websocket_connections: None,
        max_connections_per_ip: 100,
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        realtime_endpoint_overrides: Default::default(),
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
        aliases: Default::default(),
    }
}

async fn gateway() -> SocketAddr {
    let app_state = AppState::new(server_config()).await;
    let ws_routes = routes::ws::create_ws_router().layer(middleware::from_fn_with_state(
        app_state.clone(),
        auth_middleware,
    ));
    let app = routes::api::create_api_router()
        .merge(ws_routes)
        .with_state(app_state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// One streaming session: config, ten 100 ms chunks (two utterances at the mock), then quiet.
async fn session(gateway: SocketAddr, vendor_port: u16) -> Vec<Value> {
    let (mut ws, _) = connect_async(format!("ws://{gateway}/ws")).await.unwrap();
    let config = json!({
        "type": "config",
        "audio": true,
        "stt_config": {
            "provider": "deepgram",
            "model": "nova-3",
            "api_key": "test-key",
            "language": "en-US",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true,
            "encoding": "linear16",
            "extras": { "endpoint_override": format!("ws://127.0.0.1:{vendor_port}") }
        },
        "tts_config": {
            "provider": "openai",
            "api_key": "test-key",
            "model": "tts-1",
            "voice_id": "alloy",
            "audio_format": "linear16",
            "sample_rate": 24000,
            "connection_timeout": 1,
            "request_timeout": 5
        }
    });
    ws.send(Message::Text(config.to_string().into()))
        .await
        .unwrap();
    let mut seen = Vec::new();
    let mut ready = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !ready && tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(Message::Text(t)))) =
            tokio::time::timeout(Duration::from_secs(1), ws.next()).await
        {
            let v: Value = serde_json::from_str(&t).unwrap();
            ready = v["type"] == "ready";
            seen.push(v);
        }
    }
    assert!(ready, "no ready: {seen:?}");
    for i in 0..10 {
        let chunk: Vec<u8> = (0..1600)
            .flat_map(|s: i32| (((s * 37 + i * 101) % 2000 - 1000) as i16).to_le_bytes())
            .collect();
        ws.send(Message::Binary(chunk.into())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let quiet = tokio::time::Instant::now() + Duration::from_millis(2_500);
    while tokio::time::Instant::now() < quiet {
        match tokio::time::timeout(Duration::from_millis(200), ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => seen.push(serde_json::from_str(&t).unwrap()),
            Ok(Some(Ok(_))) => {}
            Ok(_) => break,
            Err(_) => {}
        }
    }
    let _ = ws.close(None).await;
    seen
}

/// What may differ between runs: the session id.
fn normalized(mut msgs: Vec<Value>) -> Vec<Value> {
    for m in &mut msgs {
        if let Some(o) = m.as_object_mut()
            && o.contains_key("stream_id")
        {
            o.insert("stream_id".into(), json!("<stream_id>"));
        }
    }
    msgs
}

fn without_ready_stt(msgs: &[Value]) -> (Vec<Value>, Option<Value>) {
    let mut stt = None;
    let rest = msgs
        .iter()
        .cloned()
        .map(|mut m| {
            if m["type"] == "ready"
                && let Some(o) = m.as_object_mut()
            {
                stt = o.remove("stt");
            }
            m
        })
        .collect();
    (rest, stt)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_streaming_session_matches_the_golden_recording_covered_or_not() {
    // SAFETY: this binary has one test; every variable is set before the gateway it configures.
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
        std::env::set_var("WAAV_STT_LIVE_RELEASE", "2");
    }
    let vendor = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = vendor.local_addr().unwrap().port();
    drop(vendor);
    let state = Arc::new(WebSocketMockState::new(
        instant(),
        instant(),
        ChaosConfig::default(),
    ));
    tokio::spawn(async move {
        let _ = start_stt_websocket_mock(port, state).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    unsafe { std::env::set_var("WAAV_SEGMENTED_STT", "off") };
    let uncovered = normalized(session(gateway().await, port).await);
    unsafe { std::env::set_var("WAAV_SEGMENTED_STT", "on") };
    let covered = normalized(session(gateway().await, port).await);

    let (uncovered_rest, uncovered_stt) = without_ready_stt(&uncovered);
    let (covered_rest, covered_stt) = without_ready_stt(&covered);
    assert!(
        uncovered_stt.is_none(),
        "an uncovered session gets today's ready (Release 2)"
    );
    let covered_stt = covered_stt.expect("a covered session reports how it is served");
    assert_eq!(covered_stt["transcription_mode"], "streaming");
    assert_eq!(covered_stt["endpointing"], "vendor");
    assert!(
        covered_rest.iter().any(|m| m["type"] == "stt_result"),
        "the mock's transcripts reached the client: {covered_rest:#?}"
    );
    assert_eq!(
        uncovered_rest, covered_rest,
        "covering a streaming session changes nothing else"
    );

    let recorded = serde_json::to_string_pretty(&uncovered_rest).unwrap();
    if std::env::var("WAAV_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(std::path::Path::new(GOLDEN).parent().unwrap()).unwrap();
        std::fs::write(GOLDEN, format!("{recorded}\n")).unwrap();
    }
    let golden = std::fs::read_to_string(GOLDEN).unwrap_or_else(|_| {
        panic!("no golden recording at {GOLDEN}; run with WAAV_UPDATE_GOLDEN=1")
    });
    let golden: Vec<Value> = serde_json::from_str(&golden).unwrap();
    assert_eq!(
        uncovered_rest, golden,
        "the streaming session drifted from its golden recording"
    );
}
