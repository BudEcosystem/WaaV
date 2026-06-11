//! P1.1 wire-mock e2e: Deepgram Aura streaming TTS over the generic
//! `WebSocketTtsClient`, driven through the standardized dispatch
//! (`create_tts_standard("deepgram", features.streaming=Some(true))`) against the
//! protocol-accurate Aura WS mock (`Speak`/`Flush`/`Clear` in; binary audio +
//! `Flushed`/`Cleared` out).
//!
//! Covers the brief's five scenarios:
//! (a) full synthesis flow — ordered chunks at the `AudioCallback`, `Flushed`
//!     completes, TTFB < 300 ms against the mock (wall clock AND the client's
//!     internal first-binary-frame stamp);
//! (b) `clear()` mid-stream cancels — no further `on_audio` after `Cleared`;
//! (c) `flush=false` accumulates; `flush=true` sends ONE `Speak`;
//! (d) dispatch — `streaming=Some(true)` selects the Aura WS variant,
//!     `streaming=None` the HTTP provider;
//! (e) SSRF — loopback `endpoint_override` without `WAAV_ALLOW_LOOPBACK_ENDPOINTS`
//!     fails at connect.

mod mock_providers;

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use mock_providers::websocket_mock::{
    TTS_MOCK_CHUNK_INTERVAL_MS, WebSocketMockState, spawn_tts_websocket_mock_ephemeral,
    tts_mock_chunk_count,
};
use mock_providers::{ChaosConfig, LatencyProfile};
use waav_gateway::core::tts::standard::{StandardTTSConfig, TtsFeatures, create_tts_standard};
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSError};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// `WAAV_ALLOW_LOOPBACK_ENDPOINTS` is process-global: every test that reads or
/// mutates it holds this lock for its whole body so the SSRF-negative test cannot
/// race the loopback-enabled tests.
fn env_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| StdMutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn allow_loopback() {
    // SAFETY: test-only env mutation, serialized by env_guard.
    unsafe { std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1") };
}

fn deny_loopback() {
    // SAFETY: test-only env mutation, serialized by env_guard.
    unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };
}

/// Collects everything the provider delivers through the `AudioCallback`.
#[derive(Default)]
struct Collector {
    /// (arrival instant, first byte = mock chunk index, byte len) per chunk.
    chunks: StdMutex<Vec<(Instant, u8, usize)>>,
    completes: AtomicUsize,
    errors: StdMutex<Vec<String>>,
}

impl Collector {
    fn chunk_count(&self) -> usize {
        self.chunks.lock().unwrap().len()
    }
    fn first_chunk_at(&self) -> Option<Instant> {
        self.chunks.lock().unwrap().first().map(|(t, _, _)| *t)
    }
    fn chunk_indices(&self) -> Vec<u8> {
        self.chunks.lock().unwrap().iter().map(|(_, i, _)| *i).collect()
    }
    fn completes(&self) -> usize {
        self.completes.load(Ordering::Acquire)
    }
    fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
}

impl AudioCallback for Collector {
    fn on_audio(&self, audio: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.chunks.lock().unwrap().push((
            Instant::now(),
            audio.data.first().copied().unwrap_or(u8::MAX),
            audio.data.len(),
        ));
        Box::pin(async {})
    }

    fn on_error(&self, error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.errors.lock().unwrap().push(error.to_string());
        Box::pin(async {})
    }

    fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.completes.fetch_add(1, Ordering::Release);
        Box::pin(async {})
    }
}

/// Poll until `cond` holds or `timeout` elapses; returns the final evaluation.
async fn wait_for(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cond()
}

/// Fast, chaos-free mock latency so TTFB assertions have headroom under CI load.
fn fast_mock_state() -> Arc<WebSocketMockState> {
    Arc::new(WebSocketMockState::new(
        LatencyProfile::deepgram_stt(),
        LatencyProfile {
            min_ms: 40,
            max_ms: 90,
            p50_ms: 60,
            p99_ms: 85,
        },
        ChaosConfig::default(),
    ))
}

fn aura_standard_config(port: u16) -> StandardTTSConfig {
    StandardTTSConfig {
        base: TTSConfig {
            provider: "deepgram".into(),
            api_key: "mock-key".into(),
            voice_id: Some("aura-2-thalia-en".into()),
            audio_format: Some("linear16".into()),
            sample_rate: Some(24000),
            ..Default::default()
        },
        features: TtsFeatures {
            streaming: Some(true),
            ..Default::default()
        },
        extras: Default::default(),
    }
    .with_endpoint_override(format!("ws://127.0.0.1:{port}"))
}

/// Boot mock + connected Aura provider + registered collector.
async fn connected_aura(
    state: Arc<WebSocketMockState>,
) -> (Box<dyn BaseTTS>, Arc<Collector>, tokio::task::JoinHandle<()>) {
    let (port, mock_handle) = spawn_tts_websocket_mock_ephemeral(state)
        .await
        .expect("mock must bind an ephemeral port");
    let mut tts = create_tts_standard("deepgram", aura_standard_config(port))
        .expect("standard dispatch must construct the Aura WS provider");
    assert_eq!(
        tts.get_provider_info()["api_type"], "WebSocket",
        "streaming=Some(true) must select the WS variant"
    );
    tts.connect().await.expect("connect to the mock must succeed");
    assert!(tts.is_ready());
    let collector = Arc::new(Collector::default());
    tts.on_audio(collector.clone() as Arc<dyn AudioCallback>)
        .expect("callback registration");
    (tts, collector, mock_handle)
}

// ---------------------------------------------------------------------------
// (a) Full synthesis flow: ordered chunks, completion, TTFB < 300 ms
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_synthesis_flow_ordered_chunks_completion_and_ttfb() {
    let _env = env_guard();
    allow_loopback();

    let state = fast_mock_state();
    let (mut tts, collector, _mock) = connected_aura(state.clone()).await;

    let text = "Hello from the WaaV streaming voice gateway, low latency P1.1!";
    let expected_chunks = tts_mock_chunk_count(text.len());
    assert!(expected_chunks >= 3);

    let speak_at = Instant::now();
    tts.speak(text, true).await.expect("speak(flush=true)");

    assert!(
        wait_for(Duration::from_secs(5), || collector.completes() >= 1).await,
        "Flushed must complete the utterance (chunks so far: {})",
        collector.chunk_count()
    );

    // Every chunk arrived, in submission order (mock stamps the index in byte 0).
    assert_eq!(collector.chunk_count(), expected_chunks);
    let indices = collector.chunk_indices();
    let expected: Vec<u8> = (0..expected_chunks as u8).collect();
    assert_eq!(indices, expected, "chunks must arrive in order");
    assert_eq!(collector.completes(), 1, "exactly one on_complete");
    assert!(collector.errors().is_empty(), "no errors: {:?}", collector.errors());

    // TTFB against the mock: wall clock from speak() to first on_audio…
    let ttfb = collector
        .first_chunk_at()
        .expect("first chunk recorded")
        .duration_since(speak_at);
    assert!(
        ttfb < Duration::from_millis(300),
        "TTFB must be < 300ms against the mock, was {ttfb:?}"
    );
    // …and the client's internal first-binary-frame stamp agrees.
    let info = tts.get_provider_info();
    let stamped_ms = info["last_ttfb_ms"]
        .as_u64()
        .expect("client must stamp TTFB on the first binary frame");
    assert!(
        stamped_ms < 300,
        "internally stamped TTFB must be < 300ms, was {stamped_ms}ms"
    );

    // One Speak + one Flush hit the wire for the single utterance.
    assert_eq!(state.tts_speak_frames.load(Ordering::Relaxed), 1);
    assert_eq!(state.tts_flush_frames.load(Ordering::Relaxed), 1);

    tts.disconnect().await.expect("disconnect");
    assert!(!tts.is_ready());
}

// ---------------------------------------------------------------------------
// (b) clear() mid-stream cancels: no further on_audio after Cleared
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_mid_stream_cancels_and_stops_audio() {
    let _env = env_guard();
    allow_loopback();

    let state = fast_mock_state();
    let (mut tts, collector, _mock) = connected_aura(state.clone()).await;

    // 400 chars → 40 chunks × 20ms ≈ 800ms of streaming: plenty of mid-stream window.
    let long_text = "All work and no play makes the gateway a dull boy. ".repeat(8);
    assert!(tts_mock_chunk_count(long_text.len()) >= 40);
    tts.speak(&long_text, true).await.expect("speak long utterance");

    // Wait until we are demonstrably mid-stream…
    assert!(
        wait_for(Duration::from_secs(3), || collector.chunk_count() >= 3).await,
        "stream must be underway before clearing"
    );
    // …then barge in.
    tts.clear().await.expect("clear()");

    // Give the Clear → Cleared round-trip time to settle, then take the watermark.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let watermark = collector.chunk_count();
    assert!(
        watermark < tts_mock_chunk_count(long_text.len()),
        "clear must interrupt the stream before it finishes"
    );

    // No further audio after Cleared: the watermark must not move for several
    // would-be chunk intervals.
    tokio::time::sleep(Duration::from_millis(TTS_MOCK_CHUNK_INTERVAL_MS * 25)).await;
    assert_eq!(
        collector.chunk_count(),
        watermark,
        "no on_audio may arrive after clear()/Cleared"
    );
    // The cancelled utterance must not complete.
    assert_eq!(collector.completes(), 0, "cancelled utterance must not on_complete");
    // The Clear frame actually reached the provider.
    assert_eq!(state.tts_clear_frames.load(Ordering::Relaxed), 1);

    tts.disconnect().await.expect("disconnect");
}

// ---------------------------------------------------------------------------
// (c) flush=false accumulates; flush=true sends ONE Speak
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flush_false_accumulates_then_single_speak_on_flush() {
    let _env = env_guard();
    allow_loopback();

    let state = fast_mock_state();
    let (mut tts, collector, _mock) = connected_aura(state.clone()).await;

    tts.speak("Hello ", false).await.expect("buffered speak 1");
    tts.speak("beautiful ", false).await.expect("buffered speak 2");
    // Nothing on the wire while accumulating.
    assert_eq!(state.tts_speak_frames.load(Ordering::Relaxed), 0);
    assert_eq!(state.tts_flush_frames.load(Ordering::Relaxed), 0);

    tts.speak("world!", true).await.expect("flushing speak");
    assert!(
        wait_for(Duration::from_secs(5), || collector.completes() >= 1).await,
        "flushed utterance must complete"
    );

    // ONE Speak frame, ONE Flush frame, carrying the full accumulated text.
    assert_eq!(
        state.tts_speak_frames.load(Ordering::Relaxed),
        1,
        "accumulation must collapse into a single Speak frame"
    );
    assert_eq!(state.tts_flush_frames.load(Ordering::Relaxed), 1);
    assert_eq!(
        *state.tts_last_flushed_text.lock().unwrap(),
        "Hello beautiful world!",
        "the flushed Speak must carry the accumulated text"
    );
    assert_eq!(
        collector.chunk_count(),
        tts_mock_chunk_count("Hello beautiful world!".len())
    );

    tts.disconnect().await.expect("disconnect");
}

// ---------------------------------------------------------------------------
// (d) Dispatch: streaming=Some(true) → Aura WS; streaming=None → HTTP
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_selects_ws_variant_only_when_streaming_requested() {
    let mk = |streaming: Option<bool>| StandardTTSConfig {
        base: TTSConfig {
            provider: "deepgram".into(),
            api_key: "k".into(),
            voice_id: Some("aura-asteria-en".into()),
            ..Default::default()
        },
        features: TtsFeatures {
            streaming,
            ..Default::default()
        },
        extras: Default::default(),
    };

    let aura = create_tts_standard("deepgram", mk(Some(true))).expect("aura constructs");
    let info = aura.get_provider_info();
    assert_eq!(info["provider"], "deepgram");
    assert_eq!(info["api_type"], "WebSocket");
    assert_eq!(info["transport"], "websocket");

    let http = create_tts_standard("deepgram", mk(None)).expect("http constructs");
    assert_eq!(http.get_provider_info()["api_type"], "HTTP REST");

    let http = create_tts_standard("deepgram", mk(Some(false))).expect("http constructs");
    assert_eq!(http.get_provider_info()["api_type"], "HTTP REST");

    // Streaming requested for a provider without a WS variant: graceful HTTP
    // fallback (warn), not an error.
    let mut cfg = mk(Some(true));
    cfg.base.provider = "elevenlabs".into();
    cfg.base.voice_id = Some("21m00Tcm4TlvDq8ikWAM".into());
    let fallback = create_tts_standard("elevenlabs", cfg)
        .expect("providers without a WS variant must fall back to HTTP");
    assert_ne!(fallback.get_provider_info()["api_type"], "WebSocket");
}

// ---------------------------------------------------------------------------
// (e) SSRF: loopback endpoint_override without the env flag → connect error
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssrf_loopback_override_rejected_without_env_flag() {
    let _env = env_guard();
    deny_loopback();

    // No mock needed: the SSRF gate must reject BEFORE any socket dial.
    let mut tts = create_tts_standard("deepgram", aura_standard_config(9))
        .expect("construction itself succeeds; the gate is at connect");
    let err = tts
        .connect()
        .await
        .expect_err("loopback endpoint_override must be rejected without the env flag");
    assert!(
        matches!(err, TTSError::InvalidConfiguration(_)),
        "expected InvalidConfiguration, got: {err:?}"
    );
    assert!(
        err.to_string().contains("SSRF"),
        "error must name the SSRF protection: {err}"
    );
    assert!(!tts.is_ready());

    // Speak must not silently succeed either (lazy connect re-runs the gate).
    let speak_err = tts.speak("hi", true).await.expect_err("speak must fail too");
    assert!(matches!(speak_err, TTSError::InvalidConfiguration(_)));
}
