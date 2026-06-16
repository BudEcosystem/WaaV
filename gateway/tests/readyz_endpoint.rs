//! W-C1 observability: `/readyz` != `/livez`, and `/readyz` returns 503 when an enabled
//! provider is unreachable (extreme-TDD RED→GREEN, E13).
//!
//! RED (before this PR): there was a single `health_check` returning `{"status":"OK"}` with no
//! readiness concept — `handlers::api::readyz` did not exist, so this test could not compile.
//!
//! GREEN: `/livez` is liveness (always 200), `/readyz` evaluates enabled-provider reachability
//! and returns 503 + a per-provider JSON breakdown when an enabled provider can't be reached.

use std::net::TcpListener;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    routing::get,
};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::util::ServiceExt;

use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    state::AppState,
};

fn config_with_deepgram() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        tls: None,
        livekit_url: "ws://localhost:7880".to_string(),
        livekit_public_url: "http://localhost:7880".to_string(),
        livekit_api_key: None,
        livekit_api_secret: None,
        deepgram_api_key: Some("dg-test-key".to_string()),
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
        realtime_endpoint_overrides: Default::default(),
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
    }
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/livez", get(waav_gateway::handlers::api::livez))
        .route("/readyz", get(waav_gateway::handlers::api::readyz))
        .with_state(state)
}

#[tokio::test]
async fn readyz_returns_503_when_provider_unreachable() {
    // Force the deepgram readiness probe at a known-closed local port so the TCP probe refuses.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    // SAFETY (edition 2024): single test owns these env vars; removed below.
    unsafe {
        std::env::set_var("WAAV_READINESS_PROBE_DEEPGRAM", format!("127.0.0.1:{port}"));
        std::env::remove_var("WAAV_READINESS_SKIP_NETWORK");
    }

    let state = AppState::new(config_with_deepgram()).await;
    let app = app(state);

    // /livez stays healthy regardless of upstream provider state.
    let live = app
        .clone()
        .oneshot(Request::builder().uri("/livez").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK, "liveness is independent of providers");

    // /readyz must return 503 because the enabled deepgram provider is unreachable.
    let ready = app
        .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = ready.status();

    let bytes = ready.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();

    unsafe { std::env::remove_var("WAAV_READINESS_PROBE_DEEPGRAM") };

    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "an unreachable enabled provider must make /readyz return 503; body: {json}"
    );
    assert_eq!(json["status"], "not_ready");
    // Per-provider breakdown names the unready deepgram provider.
    let providers = json["providers"].as_array().expect("providers array");
    let dg = providers
        .iter()
        .find(|p| p["provider"] == "deepgram")
        .expect("deepgram in breakdown");
    assert_eq!(dg["ready"], false);
    assert_eq!(dg["reachable"], false);
}

#[tokio::test]
async fn readyz_is_200_when_no_providers_enabled() {
    // No provider credentials configured => nothing to be not-ready about => 200 ready.
    let mut cfg = config_with_deepgram();
    cfg.deepgram_api_key = None;
    let state = AppState::new(cfg).await;
    let app = app(state);

    let ready = app
        .oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    let bytes = ready.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["status"], "ready");
}
