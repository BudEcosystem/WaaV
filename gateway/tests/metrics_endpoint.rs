//! W-C1 observability: `/metrics` exposes per-provider TTFB (extreme-TDD RED→GREEN).
//!
//! RED (before this PR): `ProviderMetrics` existed but was instantiated nowhere, there was no
//! `metrics`/`metrics-exporter-prometheus` bridge, and no `/metrics` route — so this test could
//! not even compile (`handlers::api::metrics_handler` and `core_state.metrics` did not exist).
//!
//! GREEN: boot a router with the public `/metrics` route + the `/speak` API route, drive ONE
//! real TTS request through the gateway against a local OpenAI-compatible mock (via the standard
//! `OPENAI_BASE_URL` override — no paid keys), then scrape `/metrics` and assert the
//! `waav_provider_ttfb_ms` histogram is present (i.e. the request's time-to-first-byte was
//! recorded and exported).

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use http_body_util::BodyExt;
use tower::util::ServiceExt;

use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    routes,
    state::AppState,
};

/// 8000 samples of s16le PCM "audio" the mock returns as the synthesized speech.
fn fake_pcm() -> Vec<u8> {
    let mut v = Vec::with_capacity(8000 * 2);
    for i in 0..8000u32 {
        let s = (((i as f32) * 0.04).sin() * 9000.0) as i16;
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}

fn config_with_openai() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
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
        openai_api_key: Some("local-test-key".to_string()),
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
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
    }
}

#[tokio::test]
async fn metrics_exposes_provider_ttfb() {
    use axum::routing::post;
    use tokio::net::TcpListener;

    // 1. Stand up a local OpenAI-compatible speech mock returning PCM audio.
    async fn speak(_body: bytes::Bytes) -> Vec<u8> {
        fake_pcm()
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mock = Router::new().route("/v1/audio/speech", post(speak));
    tokio::spawn(async move {
        axum::serve(listener, mock).await.unwrap();
    });

    // SAFETY (edition 2024): set/remove_var are unsafe. This is the only place in this test
    // touching OPENAI_BASE_URL; it is removed after the synthesis call below.
    unsafe { std::env::set_var("OPENAI_BASE_URL", format!("http://{addr}")) };

    // 2. Build the gateway router with the public /metrics route + the /speak API route.
    let app_state = AppState::new(config_with_openai()).await;
    let app = Router::new()
        .route(
            "/metrics",
            get(waav_gateway::handlers::api::metrics_handler),
        )
        .merge(routes::api::create_api_router())
        .with_state(app_state);

    // 3. Drive ONE TTS request through the gateway against the mock.
    let body = serde_json::json!({
        "text": "Hello from the metrics test.",
        "tts_config": {
            "provider": "openai",
            "model": "tts-1",
            "voice_id": "nova",
            "audio_format": "pcm",
            "sample_rate": 24000,
            "pronunciations": []
        }
    });
    let speak_req = Request::builder()
        .method("POST")
        .uri("/speak")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let speak_resp = app.clone().oneshot(speak_req).await.unwrap();

    unsafe { std::env::remove_var("OPENAI_BASE_URL") };

    assert_eq!(
        speak_resp.status(),
        StatusCode::OK,
        "the /speak request must succeed against the OpenAI mock so a TTFB sample is recorded"
    );

    // 4. Scrape /metrics and assert the provider TTFB histogram is present.
    let metrics_req = Request::builder()
        .uri("/metrics")
        .body(Body::empty())
        .unwrap();
    let metrics_resp = app.oneshot(metrics_req).await.unwrap();
    assert_eq!(metrics_resp.status(), StatusCode::OK);

    let bytes = metrics_resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).unwrap();

    assert!(
        text.contains("waav_provider_ttfb_ms"),
        "the /metrics exposition must include the provider TTFB histogram after a request; got:\n{text}"
    );
    assert!(
        text.contains("waav_provider_requests_total"),
        "the /metrics exposition must include the provider request counter; got:\n{text}"
    );
    assert!(
        text.contains("openai"),
        "the TTFB sample must be labelled with the openai provider; got:\n{text}"
    );
}
