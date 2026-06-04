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

/// Spawn an axum HTTP mock that returns `audio_bytes` (Content-Type `audio/mpeg`) on `path`.
async fn spawn_audio_mock(path: &'static str, audio_bytes: Vec<u8>) -> u16 {
    use axum::{Router, http::header, routing::post};
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = Router::new().route(
        path,
        post(move || {
            let bytes = audio_bytes.clone();
            async move { ([(header::CONTENT_TYPE, "audio/mpeg")], bytes) }
        }),
    );
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
