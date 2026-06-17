//! P5 batched-STT REST integration tests (credential-free).
//!
//! These exercise `POST /transcribe/batch` + `GET /transcribe/batch/{job_id}` through the real
//! axum router with `oneshot`, WITHOUT any vendor key. They prove:
//!   * an unrecognized/unsupported provider → 400 (never a silent accept);
//!   * a recognized provider with NO configured key → 400 "API key not configured" (the dispatch
//!     reached the credential-resolution step — the wire contract is correct);
//!   * an unknown job id → 404;
//!   * a valid submission whose provider is pointed at a non-routable `endpoint_override` returns
//!     a uniform `BatchJob` with `status:"error"` (proving the dispatcher built + fired the
//!     provider-native request — a real round-trip needs a vendor key, out of scope).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::util::ServiceExt;
use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    routes,
    state::AppState,
};

fn no_key_config() -> ServerConfig {
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
        aliases: Default::default(),
    }
}

async fn app() -> axum::Router {
    let state: Arc<AppState> = AppState::new(no_key_config()).await;
    routes::api::create_api_router().with_state(state)
}

async fn post_batch(app: axum::Router, body: Value) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/transcribe/batch")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap_or(json!({}));
    (status, v)
}

#[tokio::test]
async fn unsupported_provider_is_rejected_400() {
    let (status, v) = post_batch(
        app().await,
        json!({
            "url": "https://example.com/a.wav",
            "provider": "gladia", "api_key": "", "language": "en-US",
            "sample_rate": 16000, "channels": 1, "punctuation": true,
            "encoding": "linear16", "model": ""
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        v["error"].as_str().unwrap_or("").contains("not supported"),
        "{v}"
    );
}

#[tokio::test]
async fn missing_credential_is_clear_400() {
    // Deepgram IS supported, but no key is configured and none supplied → clean 400.
    let (status, v) = post_batch(
        app().await,
        json!({
            "url": "https://example.com/a.wav",
            "provider": "deepgram", "api_key": "", "language": "en-US",
            "sample_rate": 16000, "channels": 1, "punctuation": true,
            "encoding": "linear16", "model": "nova-2",
            "batch": { "summarize": true, "alternatives": 2, "detect_language": true }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let msg = v["error"].as_str().unwrap_or("");
    assert!(
        msg.contains("API key") || msg.contains("not configured"),
        "expected a credential error, got: {v}"
    );
}

#[tokio::test]
async fn unknown_job_is_404() {
    let req = Request::builder()
        .method("GET")
        .uri("/transcribe/batch/does-not-exist")
        .body(Body::empty())
        .unwrap();
    let resp = app().await.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn valid_submission_dispatches_and_stores_error_job_when_endpoint_unreachable() {
    // A complete OpenAI batch with a (fake) client key + an endpoint_override pointing at a
    // non-routable host. This PROVES the dispatcher resolved the key, built the multipart request,
    // and fired it — the provider call fails (no real endpoint), yielding a uniform error job.
    // A real transcription round-trip needs a vendor key (out of scope here).
    let body = json!({
        "audio_base64": "AAAAAAAAAAA=",
        "content_type": "audio/wav",
        "provider": "openai", "api_key": "sk-fake-for-dispatch-only", "language": "en-US",
        "sample_rate": 16000, "channels": 1, "punctuation": true,
        "encoding": "linear16", "model": "whisper-1",
        "batch": { "detect_language": true },
        "extras": { "endpoint_override": "http://127.0.0.1:1" }
    });
    let the_app = app().await;
    let (status, v) = post_batch(the_app.clone(), body).await;
    // The request was processed (200) and the job carries the failure (uniform pollable shape).
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["status"], "error", "expected an error job: {v}");
    let job_id = v["job_id"].as_str().expect("job_id present").to_string();

    // And it is retrievable by id (poll path), still status=error.
    let req = Request::builder()
        .method("GET")
        .uri(format!("/transcribe/batch/{job_id}"))
        .body(Body::empty())
        .unwrap();
    let resp = the_app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let got: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(got["job_id"], job_id);
    assert_eq!(got["status"], "error");
}
