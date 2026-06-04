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

/// Spawn a mock for a two-step auth+synth TTS provider: `auth_path` returns the JSON token envelope
/// `token_body` (shape varies per vendor — CereProc `{token}`, Baidu `{access_token,expires_in}`,
/// Sber `{access_token,expires_at}`); every other path returns raw audio bytes. Used by providers
/// that fetch a Bearer/OAuth token before synthesizing.
async fn spawn_auth_then_audio_mock(
    auth_path: &'static str,
    token_body: serde_json::Value,
    audio_bytes: Vec<u8>,
) -> u16 {
    use axum::{Json, Router, http::header, routing::post};
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new()
        .route(
            auth_path,
            post(move || {
                let b = token_body.clone();
                async move { Json(b) }
            }),
        )
        .fallback(move || {
            let bytes = audio_bytes.clone();
            async move { ([(header::CONTENT_TYPE, "audio/mpeg")], bytes) }
        });
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// Spawn a mock that returns a fixed JSON `body` on every path. For providers whose synth response
/// is a JSON envelope with base64 audio nested under vendor-specific keys (e.g. Tencent's
/// `{"Response":{"Audio": ...}}`); build the body with the base64 audio embedded.
async fn spawn_fixed_json_mock(body: serde_json::Value) -> u16 {
    use axum::{Json, Router};
    use tokio::net::TcpListener;
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

/// Spawn a mock for providers whose synth POST returns a JSON envelope containing a *download URL*
/// the provider then GETs for the audio (FPT.AI `{"async": url}`, Zalo `{"data":{"url": url}}`). The
/// mock builds the response body via `build_body(download_url)` pointing the URL at its own
/// `/download` route (which serves the audio bytes), so the second-hop GET lands back on the mock.
async fn spawn_audio_url_then_download_mock(
    build_body: fn(String) -> serde_json::Value,
    audio_bytes: Vec<u8>,
) -> u16 {
    use axum::{Json, Router, http::header, routing::get};
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = build_body(format!("http://127.0.0.1:{port}/download"));
    let app = Router::new()
        .route(
            "/download",
            get(move || {
                let bytes = audio_bytes.clone();
                async move { ([(header::CONTENT_TYPE, "audio/mpeg")], bytes) }
            }),
        )
        .fallback(move || {
            let body = body.clone();
            async move { Json(body) }
        });
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    port
}

/// Spawn a WebSocket mock for streaming TTS providers: accept the connection, wait for the client's
/// synthesis-request frame, then send each JSON `frames` entry as a Text message (the provider's
/// read loop decodes base64 audio out of them). For iFlytek-style providers that deliver audio as
/// base64 inside JSON Text frames rather than binary.
async fn spawn_ws_audio_mock(frames: Vec<serde_json::Value>) -> u16 {
    use futures::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::protocol::Message;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let frames: Vec<String> = frames.iter().map(|v| v.to_string()).collect();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await
            && let Ok(ws) = accept_async(stream).await
        {
            let (mut write, mut read) = ws.split();
            // Wait for the synthesis request, then stream the audio frames back.
            if let Some(Ok(_req)) = read.next().await {
                for f in &frames {
                    let _ = write.send(Message::Text(f.clone().into())).await;
                }
            }
        }
    });
    port
}

/// Base64-encode a blob (for JSON-enveloped audio responses).
fn b64(bytes: &[u8]) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    STANDARD.encode(bytes)
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

#[tokio::test]
async fn deepgram_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "deepgram",
        "/v1/speak",
        TTSConfig {
            provider: "deepgram".into(),
            api_key: "test-key".into(),
            model: "aura-2-thalia-en".to_string(),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn cereproc_tts_full_integration_via_mock_endpoint() {
    // CereProc is two-step: connect() logs in (POST /v2/auth → {"token":...}) then speak() POSTs
    // /v2/speak → audio bytes. The auth mock serves the token; the fallback serves the audio.
    ensure_crypto();
    let port = spawn_auth_then_audio_mock(
        "/v2/auth",
        serde_json::json!({ "token": "mock-token" }),
        fake_audio(),
    )
    .await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "cereproc".into(),
        // email:password packing for HTTP Basic auth.
        api_key: "user@example.com:password".into(),
        voice_id: Some("Heather".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("cereproc", std).expect("build cereproc tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("cereproc TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "cereproc TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn acapela_tts_full_integration_via_mock_endpoint() {
    // Acapela: connect() logs in (POST /api/login/ → {"token":...}) then speak() GETs /api/command/
    // → audio bytes. Auth route serves the token; the fallback serves the audio (GET included).
    ensure_crypto();
    let port = spawn_auth_then_audio_mock(
        "/api/login/",
        serde_json::json!({ "token": "mock-token" }),
        fake_audio(),
    )
    .await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "acapela".into(),
        // email:password packing for the Acapela login.
        api_key: "user@example.com:password".into(),
        voice_id: Some("alice".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("acapela", std).expect("build acapela tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("acapela TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "acapela TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn baidu_tts_full_integration_via_mock_endpoint() {
    // Baidu: OAuth (POST /oauth/2.0/token → {access_token,expires_in}) then synth (POST /text2audio
    // → raw audio bytes). connect() fetches the token; the fallback serves the synth audio.
    ensure_crypto();
    let port = spawn_auth_then_audio_mock(
        "/oauth/2.0/token",
        serde_json::json!({ "access_token": "mock-token", "expires_in": 2_592_000 }),
        fake_audio(),
    )
    .await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "baidu".into(),
        // api_key|secret_key packing (Baidu fetches an OAuth token from these).
        api_key: "test-key|test-secret".into(),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("baidu", std).expect("build baidu tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("baidu TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "baidu TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn sberdevices_tts_full_integration_via_mock_endpoint() {
    // SberDevices: OAuth (POST /api/v2/oauth → {access_token,expires_at_ms}) then synth (POST
    // /rest/v1/text:synthesize → raw audio bytes). `expires_at` is ms-since-epoch and must be well
    // in the future so the token is cached past the refresh threshold.
    ensure_crypto();
    let port = spawn_auth_then_audio_mock(
        "/api/v2/oauth",
        serde_json::json!({ "access_token": "mock-token", "expires_at": 9_999_999_999_000u64 }),
        fake_audio(),
    )
    .await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "sberdevices".into(),
        // client_id:client_secret packing (base64-encoded for HTTP Basic to the OAuth endpoint).
        api_key: "test-client-id:test-client-secret".into(),
        voice_id: Some("Nec_24000".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts =
        create_tts_standard("sberdevices", std).expect("build sberdevices tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("sberdevices TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "sberdevices TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn smallest_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "smallest",
        "/api/v1/lightning/get_speech",
        TTSConfig {
            provider: "smallest".into(),
            api_key: "test-key".into(),
            voice_id: Some("emily".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn resemble_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "resemble",
        "/stream",
        TTSConfig {
            provider: "resemble".into(),
            api_key: "test-key".into(),
            voice_id: Some("55592656".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn lmnt_tts_full_integration_via_mock_endpoint() {
    assert_rest_tts_surfaces_audio(
        "lmnt",
        "/v1/ai/speech/bytes",
        TTSConfig {
            provider: "lmnt".into(),
            api_key: "test-key".into(),
            voice_id: Some("lily".to_string()),
            ..Default::default()
        },
    )
    .await;
}

#[tokio::test]
async fn iflytek_tts_full_integration_via_mock_endpoint() {
    // iFlytek TTS is a signed WebSocket: connect (signed URL), send a JSON request, receive JSON
    // Text frames carrying base64 audio under data.audio (status 2 = final chunk). endpoint_override
    // (a ws:// base) swaps the signed URL's host while keeping the /v2/tts path + auth query.
    ensure_crypto();
    let frame = serde_json::json!({
        "code": 0,
        "message": "success",
        "sid": "mock-sid",
        "data": { "audio": b64(&fake_audio()), "status": 2 }
    });
    let port = spawn_ws_audio_mock(vec![frame]).await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "iflytek".into(),
        // app_id|api_key|api_secret packing for the signed-URL auth.
        api_key: "test-app|test-key-xxxxxxxx|test-secret-xxxxxx".into(),
        voice_id: Some("xiaoyan".to_string()),
        // iFlytek TTS only supports 8000/16000 (default config carries 24000).
        sample_rate: Some(16000),
        ..Default::default()
    })
    .with_endpoint_override(format!("ws://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("iflytek", std).expect("build iflytek tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("iflytek TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "iflytek TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn huawei_cloud_tts_full_integration_via_mock_endpoint() {
    // Huawei: IAM POST /v3/auth/tokens returns the token in the X-Subject-Token RESPONSE HEADER;
    // synth POST /v1/{project}/tts returns {"result":{"data": base64}}. Both hosts are swapped to
    // the mock by endpoint_override (the shared token manager honors it via get_token_with_override).
    use axum::{Json, Router, http::HeaderName, routing::post};
    use tokio::net::TcpListener;
    ensure_crypto();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let synth_body = serde_json::json!({ "result": { "data": b64(&fake_audio()) } });
    let app = Router::new()
        .route(
            "/v3/auth/tokens",
            post(|| async {
                ([(HeaderName::from_static("x-subject-token"), "mock-token")], "{}")
            }),
        )
        .fallback(move || {
            let b = synth_body.clone();
            async move { Json(b) }
        });
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "huawei_cloud".into(),
        // username|password|domain_name|project_id packing.
        api_key: "test-user|test-pass|test-domain|proj-123".into(),
        // Huawei TTS only supports 8000/16000 (default config carries 24000).
        sample_rate: Some(16000),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts =
        create_tts_standard("huawei_cloud", std).expect("build huawei_cloud tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("huawei_cloud TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "huawei_cloud TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn aws_polly_tts_full_integration_via_mock_endpoint() {
    // AWS Polly via the SDK: endpoint_override sets the SDK's endpoint_url (raw base); the SDK
    // appends /v1/speech and SynthesizeSpeech returns the audio in the response body. Credentials
    // flow through the standardized extras passthrough.
    ensure_crypto();
    let port = spawn_audio_mock("/v1/speech", fake_audio()).await;
    let mut std = StandardTTSConfig::from_base(TTSConfig {
        provider: "aws_polly".into(),
        voice_id: Some("Joanna".to_string()),
        // Polly's default pcm output only supports 8000/16000 (default config carries 24000).
        sample_rate: Some(16000),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    std.extras.0.insert(
        "aws_access_key_id".to_string(),
        serde_json::Value::String("AKIAIOSFODNN7EXAMPLE".to_string()),
    );
    std.extras.0.insert(
        "aws_secret_access_key".to_string(),
        serde_json::Value::String("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string()),
    );
    std.extras.0.insert(
        "region".to_string(),
        serde_json::Value::String("us-east-1".to_string()),
    );
    let mut tts = create_tts_standard("aws_polly", std).expect("build aws_polly tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("aws_polly TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "aws_polly TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn bhashini_tts_full_integration_via_mock_endpoint() {
    // Bhashini is two-step: a pipeline-CONFIG POST then a COMPUTE POST. We override the config POST
    // (which returns {} → the provider falls back to custom_callback_url for the compute hop and to
    // the inference key from the api_key) and point custom_callback_url at the mock's /compute route
    // (returns {pipelineResponse:[{taskType:tts, audio:[{audioContent: base64}]}]}).
    use axum::{Json, Router, routing::post};
    use tokio::net::TcpListener;
    ensure_crypto();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let compute_body = serde_json::json!({
        "pipelineResponse": [
            { "taskType": "tts", "audio": [{ "audioContent": b64(&fake_audio()) }] }
        ]
    });
    let app = Router::new()
        .route(
            "/compute",
            post(move || {
                let b = compute_body.clone();
                async move { Json(b) }
            }),
        )
        .fallback(|| async { Json(serde_json::json!({})) });
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let mut std = StandardTTSConfig::from_base(TTSConfig {
        provider: "bhashini".into(),
        // userId|ulcaApiKey|inferenceApiKey packing.
        api_key: "test-user|test-ulca|test-inference".into(),
        // Bhashini parses voice_id as a BCP-47 language code.
        voice_id: Some("hi".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    std.extras.0.insert(
        "custom_callback_url".to_string(),
        serde_json::Value::String(format!("http://127.0.0.1:{port}/compute")),
    );
    std.extras.0.insert(
        "custom_service_id".to_string(),
        serde_json::Value::String("ai4bharat/indic-tts".to_string()),
    );
    let mut tts = create_tts_standard("bhashini", std).expect("build bhashini tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("bhashini TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "bhashini TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn prosa_ai_tts_full_integration_via_mock_endpoint() {
    // Prosa with wait=true (default) returns base64 audio directly in the POST response under
    // {status:"complete", result:{data: ...}} — no poll, no download.
    ensure_crypto();
    let body = serde_json::json!({
        "job_id": "job-1",
        "status": "complete",
        "result": { "data": b64(&fake_audio()), "url": "", "duration": 1.0 }
    });
    let port = spawn_fixed_json_mock(body).await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "prosa_ai".into(),
        api_key: "test-key".into(),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("prosa_ai", std).expect("build prosa_ai tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("prosa_ai TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "prosa_ai TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn ibm_watson_tts_full_integration_via_mock_endpoint() {
    // IBM Watson: IAM token POST /identity/token → {access_token}, then synth POST
    // /instances/{id}/v1/synthesize → raw audio bytes. instance_id is a provider extra.
    ensure_crypto();
    let port = spawn_auth_then_audio_mock(
        "/identity/token",
        serde_json::json!({ "access_token": "mock-token", "token_type": "Bearer", "expires_in": 3600 }),
        fake_audio(),
    )
    .await;
    let mut std = StandardTTSConfig::from_base(TTSConfig {
        provider: "ibm_watson".into(),
        api_key: "test-key".into(),
        voice_id: Some("en-US_AllisonV3Voice".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    std.extras.0.insert(
        "instance_id".to_string(),
        serde_json::Value::String("inst-1".into()),
    );
    let mut tts = create_tts_standard("ibm_watson", std).expect("build ibm_watson tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("ibm_watson TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "ibm_watson TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn fpt_ai_tts_full_integration_via_mock_endpoint() {
    // FPT.AI synth POST returns {"async": <download_url>}; the provider then GETs the URL for audio.
    ensure_crypto();
    let port = spawn_audio_url_then_download_mock(
        |url| serde_json::json!({ "async": url, "error": 0 }),
        fake_audio(),
    )
    .await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "fpt_ai".into(),
        api_key: "test-key".into(),
        voice_id: Some("banmai".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("fpt_ai", std).expect("build fpt_ai tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("fpt_ai TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "fpt_ai TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn nectec_tts_full_integration_via_mock_endpoint() {
    // NECTEC VAJA9 synth POST returns {"wav_url": <download_url>}; the provider then GETs it.
    ensure_crypto();
    let port = spawn_audio_url_then_download_mock(
        |url| serde_json::json!({ "wav_url": url }),
        fake_audio(),
    )
    .await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "nectec".into(),
        api_key: "test-key".into(),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("nectec", std).expect("build nectec tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("nectec TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "nectec TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn zalo_ai_tts_full_integration_via_mock_endpoint() {
    // Zalo synth POST returns {"data":{"url": <download_url>}}; the provider then GETs it for audio.
    ensure_crypto();
    let port = spawn_audio_url_then_download_mock(
        |url| serde_json::json!({ "error_code": 0, "data": { "url": url } }),
        fake_audio(),
    )
    .await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "zalo_ai".into(),
        api_key: "test-key".into(),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("zalo_ai", std).expect("build zalo_ai tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("zalo_ai TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "zalo_ai TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn tencent_tts_full_integration_via_mock_endpoint() {
    // Tencent returns audio base64 nested under {"Response":{"Audio": ...}}.
    ensure_crypto();
    let body = serde_json::json!({ "Response": { "Audio": b64(&fake_audio()) } });
    let port = spawn_fixed_json_mock(body).await;
    let std = StandardTTSConfig::from_base(TTSConfig {
        provider: "tencent".into(),
        // secret_id|secret_key packing.
        api_key: "test-secret-id|test-secret-key".into(),
        voice_id: Some("101001".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    let mut tts = create_tts_standard("tencent", std).expect("build tencent tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("tencent TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "tencent TTS surfaced no audio end-to-end");
}

#[tokio::test]
async fn playht_tts_full_integration_via_mock_endpoint() {
    // PlayHT authenticates with both an api_key (Authorization) and a user_id (X-USER-ID); the
    // user_id is a provider-specific extra, not part of the flat config.
    ensure_crypto();
    let port = spawn_audio_mock("/api/v2/tts/stream", fake_audio()).await;
    let mut std = StandardTTSConfig::from_base(TTSConfig {
        provider: "playht".into(),
        api_key: "test-key".into(),
        voice_id: Some("s3://voice/manifest.json".to_string()),
        ..Default::default()
    })
    .with_endpoint_override(format!("http://127.0.0.1:{port}"));
    std.extras.0.insert(
        "user_id".to_string(),
        serde_json::Value::String("test-user".into()),
    );
    let mut tts = create_tts_standard("playht", std).expect("build playht tts via keystone");
    let bytes = drive_tts(tts.as_mut()).await;
    println!("playht TTS mock e2e surfaced {bytes} audio bytes");
    assert!(bytes > 0, "playht TTS surfaced no audio end-to-end");
}
