//! Credential-free END-TO-END integration for TTS providers via `endpoint_override` + a local mock.
//!
//! Mirrors `mock_endpoint_e2e.rs` (the STT harness) for the TTS modality: drive the REAL provider
//! through the full loop — `create_tts_standard` → `connect()` → `speak(text)` → the provider's HTTP
//! synth request → response audio bytes → `on_audio` callback — against an in-repo mock, no key. The
//! audio output is the dual of STT's transcript: the test asserts non-empty audio bytes surface.
//!
//! Run with `--test-threads=1` (the OpenAI case sets `OPENAI_BASE_URL`, a process-global env var).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use waav_gateway::core::tts::standard::{StandardTTSConfig, create_tts_standard};
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSError};

fn ensure_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// An `AudioCallback` that accumulates the total number of audio bytes surfaced by the provider.
struct CaptureAudio {
    total: Arc<AtomicUsize>,
}

impl AudioCallback for CaptureAudio {
    fn on_audio(&self, audio: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        let total = self.total.clone();
        Box::pin(async move {
            total.fetch_add(audio.data.len(), Ordering::SeqCst);
        })
    }
    fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

/// Drive a TTS provider: register an audio sink, connect, speak, flush, wait, disconnect; return the
/// total audio bytes the provider surfaced via `on_audio`.
async fn drive_tts(tts: &mut dyn BaseTTS) -> usize {
    let total = Arc::new(AtomicUsize::new(0));
    tts.on_audio(Arc::new(CaptureAudio {
        total: total.clone(),
    }))
    .unwrap();
    let _ = tts.connect().await;
    tts.speak("hello world", true).await.expect("speak");
    let _ = tts.flush().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = tts.disconnect().await;
    total.load(Ordering::SeqCst)
}

/// Spawn an axum HTTP mock that returns `audio_bytes` (Content-Type `audio/mpeg`) on `path` AND on
/// any other path via a fallback. The fallback frees each provider's synth POST from needing an
/// exact route — some embed dynamic segments (Speechmatics `/generate/<voice>`) or punctuation
/// (Yandex `/speech/v1/tts:synthesize`) that are awkward to register literally.
async fn spawn_audio_mock(path: &'static str, audio_bytes: Vec<u8>) -> u16 {
    use axum::{Router, http::header, routing::post};
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let primary = audio_bytes.clone();
    let fallback = audio_bytes.clone();
    let app = Router::new()
        .route(
            path,
            post(move || {
                let bytes = primary.clone();
                async move { ([(header::CONTENT_TYPE, "audio/mpeg")], bytes) }
            }),
        )
        .fallback(move || {
            let bytes = fallback.clone();
            async move { ([(header::CONTENT_TYPE, "audio/mpeg")], bytes) }
        });
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// Spawn a mock that returns a JSON body with base64-encoded audio under `key`, for providers that
/// wrap synthesized audio in a JSON envelope (e.g. Gnani's `audioContent`) rather than streaming
/// raw bytes. Served on every path via a fallback.
async fn spawn_json_audio_mock(key: &'static str, audio_bytes: Vec<u8>) -> u16 {
    use axum::{Json, Router};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use tokio::net::TcpListener;
    let b64 = STANDARD.encode(&audio_bytes);
    let body = serde_json::json!({ key: b64 });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().fallback(move || {
        let body = body.clone();
        async move { Json(body) }
    });
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// A small blob of fake "audio" bytes the mock returns; the provider passes raw response bytes
/// through to `on_audio` for the formats these REST TTS endpoints emit.
fn fake_audio() -> Vec<u8> {
    // ID3 header + filler so it looks vaguely like an MP3 payload; content is irrelevant.
    let mut v = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
    v.extend(std::iter::repeat_n(0xAAu8, 4096));
    v
}

/// Build a REST TTS provider via the keystone with an `endpoint_override`, drive it against a
/// localhost mock serving `path`, and assert audio bytes surfaced end-to-end. This is the dual of
/// the STT mock-e2e `drive_rest_and_capture`: it proves the provider's synth POST reaches the wire
/// (host swapped, path preserved) and the response audio flows back through `on_audio`, with no key.
async fn assert_rest_tts_surfaces_audio(provider: &str, path: &'static str, base: TTSConfig) {
    ensure_crypto();
    let port = spawn_audio_mock(path, fake_audio()).await;
    let std = StandardTTSConfig::from_base(base)
        .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts =
        create_tts_standard(provider, std).unwrap_or_else(|e| panic!("build {provider} tts: {e:?}"));
    let bytes = drive_tts(tts.as_mut()).await;
    println!("{provider} TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "{provider} TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn openai_tts_full_integration_via_mock_endpoint() {
    ensure_crypto();
    let port = spawn_audio_mock("/v1/audio/speech", fake_audio()).await;
    // OpenAI TTS resolves its endpoint via OPENAI_BASE_URL (designed for credential-free e2e).
    unsafe {
        std::env::set_var("OPENAI_BASE_URL", format!("http://127.0.0.1:{port}"));
    }
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "openai".into(),
        api_key: "test-key".into(),
        voice_id: Some("alloy".to_string()),
        model: "tts-1".to_string(),
        ..Default::default()
    });
    let mut tts = create_tts_standard("openai", std).expect("build openai tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    unsafe {
        std::env::remove_var("OPENAI_BASE_URL");
    }
    println!("OpenAI TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "OpenAI TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn cartesia_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "cartesia",
        "/tts/bytes",
        TTSConfig {
            provider: "cartesia".into(),
            api_key: "test-key".into(),
            voice_id: Some("a0e99841-438c-4a64-b679-ae501e7d6091".to_string()),
            model: "sonic-3".to_string(),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn hume_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "hume",
        "/v0/tts/stream/file",
        TTSConfig {
            provider: "hume".into(),
            api_key: "test-key".into(),
            voice_id: Some("Kora".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn speechify_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "speechify",
        "/v1/audio/stream",
        TTSConfig {
            provider: "speechify".into(),
            api_key: "test-key".into(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn murf_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "murf",
        "/v1/speech/stream",
        TTSConfig {
            provider: "murf".into(),
            api_key: "test-key".into(),
            voice_id: Some("en-US-natalie".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn unrealspeech_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "unrealspeech",
        "/stream",
        TTSConfig {
            provider: "unrealspeech".into(),
            api_key: "test-key".into(),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn reverie_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "reverie",
        "/",
        TTSConfig {
            provider: "reverie".into(),
            api_key: "test-key".into(),
            model: "test-app-id".to_string(),
            voice_id: Some("hi_female".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn wellsaid_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "wellsaid",
        "/v1/tts/stream",
        TTSConfig {
            provider: "wellsaid".into(),
            api_key: "test-key".into(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn yandex_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "yandex",
        "/speech/v1/tts_synthesize",
        TTSConfig {
            provider: "yandex".into(),
            // folder_id|api_key packing (no dot → not treated as an IAM token).
            api_key: "folder123:test-key".into(),
            voice_id: Some("alena".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn viettel_ai_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "viettel_ai",
        "/voice/api/tts/v1/rest/syn",
        TTSConfig {
            provider: "viettel_ai".into(),
            api_key: "test-token".into(),
            voice_id: Some("hn-quynhanh".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn gnani_tts_full_integration_via_mock_endpoint() {
    // Gnani wraps audio in a JSON envelope (`audioContent`, base64) rather than streaming raw bytes.
    ensure_crypto();
    let port = spawn_json_audio_mock("audioContent", fake_audio()).await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "gnani".into(),
        // token|access_key packing (Gnani requires both credentials).
        api_key: "test-token|test-access".into(),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("gnani", std).expect("build gnani tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("gnani TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "gnani TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn naver_clova_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "naver_clova",
        "/tts-premium/v1/tts",
        TTSConfig {
            provider: "naver_clova".into(),
            // client_id|client_secret packing for the X-NCP headers.
            api_key: "test-id|test-secret".into(),
            voice_id: Some("nara".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn speechmatics_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "speechmatics",
        "/generate/sarah",
        TTSConfig {
            provider: "speechmatics".into(),
            api_key: "test-key".into(),
            voice_id: Some("sarah".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn azure_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "azure",
        "/cognitiveservices/v1",
        TTSConfig {
            provider: "azure".into(),
            api_key: "test-subscription-key".into(),
            voice_id: Some("en-US-JennyNeural".to_string()),
            ..Default::default()
        },
    )
    .await;
}
