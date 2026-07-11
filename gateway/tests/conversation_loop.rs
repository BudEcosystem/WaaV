//! W-O2 — Built-in automatic conversation loop (extreme TDD RED→GREEN).
//!
//! PROBLEM (verified): WaaV exposes STT and TTS as separate primitives; nothing
//! turns a *final transcript* into an *LLM call* into *TTS*. There is no
//! turn-taking/barge-in orchestration or per-session history at the gateway, and
//! the OpenAI-compatible LLM logic was trapped behind the `dag-routing` feature.
//!
//! These tests exercise the feature-flag-free `ConversationOrchestrator` +
//! `LlmClient` (NO `dag-routing` feature required):
//!
//! 1. `final_transcript_triggers_llm_then_tts` — a final STT result drives an LLM
//!    turn (against an in-process OpenAI-compatible mock via the conversation
//!    `base_url`), and the reply is spoken: `VoiceManager::speak()` runs and the
//!    mock TTS egresses audio.
//! 2. `barge_in_interrupts_tts` — while a (slow, streaming) LLM turn is in
//!    flight and the bot is interruptible, new user speech cancels the in-flight
//!    turn and clears TTS.
//! 3. `multi_turn_history_preserved` — across two turns, the second LLM request
//!    body includes the first turn's user + assistant messages.
//!
//! Providers are mocked in-process: a mock TTS that records `speak`/`clear` and
//! egresses a marker audio frame, and a tiny OpenAI-compatible HTTP mock for the
//! LLM. STT is driven by feeding `STTResult`s straight into the orchestrator
//! (the same path the VoiceManager STT callback would take).

use futures_util::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::{Json, Router, extract::State, routing::post};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use waav_gateway::core::conversation::{
    ConversationConfig, ConversationOrchestrator, LatencyFiller,
};
use waav_gateway::core::llm::{LlmClient, LlmClientConfig};
use waav_gateway::core::stt::STTConfig;
use waav_gateway::core::stt::{BaseSTT, STTResult};
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSResult};
use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
use waav_gateway::global_registry;
use waav_gateway::plugin::metadata::ProviderMetadata;

// ──────────────────────────────────────────────────────────────────────────────
// Mock TTS provider: records speak()/clear() and egresses a marker audio frame.
// ──────────────────────────────────────────────────────────────────────────────
const TTS_MARKER: &[u8] = b"CONV_TTS_AUDIO";

#[derive(Default)]
struct TtsStats {
    spoken: Mutex<Vec<String>>,
    clears: AtomicUsize,
    audio_emitted: AtomicUsize,
}

// Global stats so the test can observe a mock created inside the VoiceManager
// (BaseTTS::new gives us no constructor hook to inject shared state otherwise).
static TTS_STATS: once_cell::sync::Lazy<Arc<TtsStats>> =
    once_cell::sync::Lazy::new(|| Arc::new(TtsStats::default()));

fn reset_tts_stats() {
    TTS_STATS.spoken.lock().clear();
    TTS_STATS.clears.store(0, Ordering::SeqCst);
    TTS_STATS.audio_emitted.store(0, Ordering::SeqCst);
}

struct MockTts {
    ready: bool,
    callback: Option<Arc<dyn AudioCallback>>,
    stats: Arc<TtsStats>,
}

#[async_trait::async_trait]
impl BaseTTS for MockTts {
    fn new(_config: TTSConfig) -> TTSResult<Self> {
        Ok(Self {
            ready: false,
            callback: None,
            stats: TTS_STATS.clone(),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        self.ready = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.ready = false;
        self.callback = None;
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.ready
    }

    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        self.stats.spoken.lock().push(text.to_string());
        if let Some(cb) = &self.callback {
            let audio = AudioData {
                data: TTS_MARKER.to_vec(),
                sample_rate: 24000,
                format: "pcm16".to_string(),
                duration_ms: Some(20),
            };
            cb.on_audio(audio).await;
            self.stats.audio_emitted.fetch_add(1, Ordering::SeqCst);
            cb.on_complete().await;
        }
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        self.stats.clears.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.callback = Some(callback);
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        json!({ "provider": "mock-tts-conv" })
    }
}

fn register_mock_tts() {
    let registry = global_registry();
    registry.register_tts(
        "mock-tts-conv",
        Arc::new(|config: TTSConfig| MockTts::new(config).map(|t| Box::new(t) as Box<dyn BaseTTS>)),
        ProviderMetadata::tts("mock-tts-conv", "Mock TTS (conversation)"),
    );
    // STT provider is unused in these tests (we feed STTResults directly), but the
    // VoiceManager still constructs one. Register a no-op deepgram-style mock.
    registry.register_stt(
        "mock-stt-conv",
        Arc::new(|config: STTConfig| {
            NoopStt::new(config).map(|s| Box::new(s) as Box<dyn waav_gateway::core::stt::BaseSTT>)
        }),
        ProviderMetadata::stt("mock-stt-conv", "No-op STT (conversation)"),
    );
}

struct NoopStt {
    ready: bool,
}

#[async_trait::async_trait]
impl waav_gateway::core::stt::BaseSTT for NoopStt {
    fn new(_config: STTConfig) -> Result<Self, waav_gateway::core::stt::STTError> {
        Ok(Self { ready: false })
    }
    async fn connect(&mut self) -> Result<(), waav_gateway::core::stt::STTError> {
        self.ready = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<(), waav_gateway::core::stt::STTError> {
        self.ready = false;
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.ready
    }
    async fn send_audio(
        &mut self,
        _audio: bytes::Bytes,
    ) -> Result<(), waav_gateway::core::stt::STTError> {
        Ok(())
    }
    async fn on_result(
        &mut self,
        _cb: waav_gateway::core::stt::STTResultCallback,
    ) -> Result<(), waav_gateway::core::stt::STTError> {
        Ok(())
    }
    async fn on_error(
        &mut self,
        _cb: waav_gateway::core::stt::STTErrorCallback,
    ) -> Result<(), waav_gateway::core::stt::STTError> {
        Ok(())
    }
    fn get_config(&self) -> Option<&STTConfig> {
        None
    }
    async fn update_config(
        &mut self,
        _config: STTConfig,
    ) -> Result<(), waav_gateway::core::stt::STTError> {
        Ok(())
    }
    fn get_provider_info(&self) -> &'static str {
        "mock-stt-conv"
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// In-process OpenAI-compatible chat-completions mock.
// Records every request body so tests can assert on history; supports an
// optional per-request delay so a turn can be "in flight" for the barge-in test.
// ──────────────────────────────────────────────────────────────────────────────
#[derive(Clone, Default)]
struct LlmMockState {
    requests: Arc<Mutex<Vec<Value>>>,
    delay_ms: Arc<AtomicUsize>,
    reply: Arc<Mutex<String>>,
    /// P1: when set, the endpoint returns HTTP 500 (a failing LLM tier).
    fail: Arc<AtomicBool>,
}

struct TestServer {
    label: &'static str,
    handle: JoinHandle<()>,
    panicked: Arc<AtomicBool>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if !self.handle.is_finished() {
            self.handle.abort();
        }
        if self.panicked.load(Ordering::SeqCst) {
            let msg = format!("conversation_loop test server '{}' panicked", self.label);
            if std::thread::panicking() {
                eprintln!("{msg}");
            } else {
                panic!("{msg}");
            }
        }
    }
}

fn spawn_test_server<F>(label: &'static str, future: F) -> TestServer
where
    F: Future<Output = ()> + Send + 'static,
{
    let panicked = Arc::new(AtomicBool::new(false));
    let panicked_in_task = Arc::clone(&panicked);
    let handle = tokio::spawn(async move {
        if AssertUnwindSafe(future).catch_unwind().await.is_err() {
            panicked_in_task.store(true, Ordering::SeqCst);
            eprintln!("conversation_loop test server '{label}' panicked");
        }
    });
    TestServer {
        label,
        handle,
        panicked,
    }
}

async fn start_llm_mock(state: LlmMockState) -> (String, TestServer) {
    async fn chat(
        State(state): State<LlmMockState>,
        Json(req): Json<Value>,
    ) -> (axum::http::StatusCode, Json<Value>) {
        state.requests.lock().push(req.clone());
        let delay = state.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay as u64)).await;
        }
        if state.fail.load(Ordering::SeqCst) {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": { "message": "mock tier failure" } })),
            );
        }
        let reply = state.reply.lock().clone();
        (
            axum::http::StatusCode::OK,
            Json(json!({
                "id": "chatcmpl-conv-mock",
                "object": "chat.completion",
                "created": 1_700_000_000u64,
                "model": "mock-llm",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": reply },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
            })),
        )
    }

    let app = Router::new()
        .route("/chat/completions", post(chat))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server("llm_mock", async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), server)
}

fn build_voice_manager() -> Arc<VoiceManager> {
    let stt_config = STTConfig {
        provider: "mock-stt-conv".to_string(),
        api_key: "test".to_string(),
        ..Default::default()
    };
    let tts_config = TTSConfig {
        provider: "mock-tts-conv".to_string(),
        api_key: "test".to_string(),
        ..Default::default()
    };
    let config = VoiceManagerConfig::new(stt_config, tts_config);
    Arc::new(VoiceManager::new(config, None).expect("voice manager"))
}

/// Wire the mock TTS callback into a marker channel so the test can assert
/// audio egress, mirroring the gateway's `register_early_tts_callback`.
async fn wire_audio_egress(vm: &Arc<VoiceManager>) -> Arc<AtomicBool> {
    let saw_audio = Arc::new(AtomicBool::new(false));
    let flag = saw_audio.clone();
    vm.on_tts_audio(move |audio: AudioData| {
        let flag = flag.clone();
        Box::pin(async move {
            if audio
                .data
                .windows(TTS_MARKER.len())
                .any(|w| w == TTS_MARKER)
            {
                flag.store(true, Ordering::SeqCst);
            }
        })
    })
    .await
    .expect("register tts audio callback");
    saw_audio
}

fn conv_config(base_url: String, streaming: bool) -> ConversationConfig {
    ConversationConfig {
        base_url,
        model: "mock-llm".to_string(),
        system_prompt: Some("You are a helpful assistant.".to_string()),
        api_key: Some("test".to_string()),
        streaming,
        allow_interruption: true,
        ..Default::default()
    }
}

fn final_result(text: &str) -> STTResult {
    // is_final = true, is_speech_final = true → a finalized turn.
    STTResult::new(text.to_string(), true, true, 0.99)
}

// ──────────────────────────────────────────────────────────────────────────────
// TEST 1: a final transcript triggers an LLM call, then TTS.
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn final_transcript_triggers_llm_then_tts() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Hi there, how can I help?".to_string();
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let saw_audio = wire_audio_egress(&vm).await;

    // Non-streaming for this test so the full reply is spoken in one call.
    let orchestrator =
        ConversationOrchestrator::new("session-1", conv_config(base_url, false), vm.clone())
            .expect("orchestrator");

    // Feed a finalized STT result.
    orchestrator
        .on_stt_result(&final_result("hello agent"))
        .await;

    // The LLM mock must have been hit, and the reply must have been spoken.
    assert_eq!(
        llm_state.requests.lock().len(),
        1,
        "LLM endpoint should be called once on a finalized turn"
    );

    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("how can I help")),
        "VoiceManager::speak() should be invoked with the LLM reply, got: {:?}",
        spoken
    );

    // And TTS audio must egress.
    assert!(
        saw_audio.load(Ordering::SeqCst),
        "Expected TTS audio egress after the LLM reply was spoken"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// TEST 2: barge-in cancels the in-flight LLM turn and clears TTS.
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn barge_in_interrupts_tts() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "This is a long answer.".to_string();
    // Make the LLM slow so the first turn is still in flight when we barge in.
    llm_state.delay_ms.store(1500, Ordering::SeqCst);
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let orchestrator = Arc::new(
        ConversationOrchestrator::new("session-bargein", conv_config(base_url, false), vm.clone())
            .expect("orchestrator"),
    );

    // Kick off turn 1 (will block ~1.5s in the LLM mock).
    let orch1 = orchestrator.clone();
    let turn1 = tokio::spawn(async move { orch1.run_turn("tell me a long story").await });

    // Give the turn time to start the (slow) LLM request.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // New user speech arrives → barge-in: cancel the in-flight turn + clear TTS.
    // (We invoke the barge-in handler directly to isolate the interruption
    // behavior; the full `on_stt_result` path additionally starts a new turn,
    // which `final_transcript_triggers_llm_then_tts` already covers.)
    orchestrator.handle_barge_in().await;

    let clears_after_barge_in = TTS_STATS.clears.load(Ordering::SeqCst);
    assert!(
        clears_after_barge_in >= 1,
        "barge-in must clear TTS at least once, saw {}",
        clears_after_barge_in
    );

    // The first (slow) turn must have been cancelled: it should NOT have spoken
    // its reply. Wait for the spawned task to settle.
    tokio::time::timeout(Duration::from_secs(3), turn1)
        .await
        .expect("cancelled turn should settle within 3s")
        .expect("cancelled turn task should not panic")
        .expect("cancelled turn should complete successfully");

    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        !spoken.iter().any(|s| s.contains("This is a long answer")),
        "the in-flight LLM turn must be cancelled before its reply is spoken, got: {:?}",
        spoken
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// TEST 2b (REALTIME_REASONING.md §4.4 / critique C3): a barge-in DURING a
// protected (filler/uninterruptible) window must STILL cancel the in-flight LLM —
// the old `handle_barge_in` early-returned and let a slow reasoning turn keep
// burning for the clip's whole duration. RED on the pre-fix code.
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn barge_in_during_protected_window_cancels_llm() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "This is a long answer.".to_string();
    llm_state.delay_ms.store(1500, Ordering::SeqCst); // slow LLM, in flight at barge-in
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let orchestrator = Arc::new(
        ConversationOrchestrator::new(
            "session-protected",
            conv_config(base_url, false),
            vm.clone(),
        )
        .expect("orchestrator"),
    );

    // Kick off a slow turn; let it reach the (1.5s) LLM request.
    let orch1 = orchestrator.clone();
    let turn1 = tokio::spawn(async move { orch1.run_turn("think hard about this").await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Open a protected window (as a filler clip would) and confirm it blocks.
    vm.arm_barge_in_suppression(400);
    assert!(
        vm.is_interruption_blocked().await,
        "test setup: the protected window must block interruption"
    );

    // Barge in DURING the protected window — the slow LLM must still be cancelled.
    orchestrator.handle_barge_in().await;

    tokio::time::timeout(Duration::from_secs(3), turn1)
        .await
        .expect("protected-window turn should settle within 3s")
        .expect("protected-window turn task should not panic")
        .expect("protected-window turn should complete successfully");
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        !spoken.iter().any(|s| s.contains("This is a long answer")),
        "the in-flight LLM must be cancelled even inside a protected window, got: {:?}",
        spoken
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// D3 latency_filler masking: one filler on a slow turn, silence on a fast turn.
// ──────────────────────────────────────────────────────────────────────────────
const FILLER_PHRASES: &[&str] = &[
    "Let me check that.",
    "One moment.",
    "Let me look into that.",
    "Give me a moment.",
    "Just a second.",
];

fn is_filler(s: &str) -> bool {
    FILLER_PHRASES.contains(&s) || s.contains("didn't catch that")
}

async fn run_masking_turn(
    filler: LatencyFiller,
    after_ms: u64,
    llm_delay_ms: usize,
) -> Vec<String> {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Your balance is forty two dollars.".to_string();
    llm_state.delay_ms.store(llm_delay_ms, Ordering::SeqCst);
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    // streaming=false matches the non-streaming mock LLM; the masking timer is
    // armed before the streaming branch, so this exercises masking either way.
    let cfg = ConversationConfig {
        latency_filler: filler,
        latency_filler_after_ms: Some(after_ms),
        ..conv_config(base_url, false)
    };
    let orchestrator =
        Arc::new(ConversationOrchestrator::new("session-mask", cfg, vm.clone()).expect("orch"));
    orchestrator
        .run_turn("what is my balance")
        .await
        .expect("masking turn should complete");
    // Let any (aborted) masking task settle.
    tokio::time::sleep(Duration::from_millis(50)).await;
    TTS_STATS.spoken.lock().clone()
}

#[tokio::test]
#[serial_test::serial]
async fn latency_filler_speaks_one_filler_on_slow_llm() {
    // Slow LLM (600ms) with a 150ms wait → exactly ONE filler, then the answer.
    let spoken = run_masking_turn(LatencyFiller::Auto, 150, 600).await;
    let fillers: Vec<_> = spoken.iter().filter(|s| is_filler(s)).collect();
    assert_eq!(
        fillers.len(),
        1,
        "exactly one masking filler expected, got: {spoken:?}"
    );
    assert!(
        spoken.iter().any(|s| s.contains("forty two")),
        "the real answer must still be spoken: {spoken:?}"
    );
    // Ordering: the filler precedes the answer.
    let fi = spoken.iter().position(|s| is_filler(s)).unwrap();
    let ai = spoken.iter().position(|s| s.contains("forty two")).unwrap();
    assert!(fi < ai, "filler must precede the answer: {spoken:?}");
}

#[tokio::test]
#[serial_test::serial]
async fn latency_filler_silent_on_fast_llm() {
    // Fast LLM (0ms) with a 150ms wait → the turn finishes first → NO filler.
    let spoken = run_masking_turn(LatencyFiller::Auto, 150, 0).await;
    assert!(
        !spoken.iter().any(|s| is_filler(s)),
        "a fast turn must not speak a filler: {spoken:?}"
    );
    assert!(spoken.iter().any(|s| s.contains("forty two")));
}

#[tokio::test]
#[serial_test::serial]
async fn latency_filler_off_speaks_nothing() {
    // Off → no filler even on a slow turn.
    let spoken = run_masking_turn(LatencyFiller::Off, 150, 600).await;
    assert!(
        !spoken.iter().any(|s| is_filler(s)),
        "latency_filler=off must never speak a filler: {spoken:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn latency_filler_does_not_poison_interruptibility() {
    // REGRESSION (brutal review CRITICAL): the masking filler must NOT leave the
    // session non-interruptible — a barge-in on the real answer (and on every
    // later turn) must keep working. The old code armed a session-global
    // suppression window that was never restored, silently disabling barge-in.
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Your balance is forty two dollars.".to_string();
    llm_state.delay_ms.store(400, Ordering::SeqCst);
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = ConversationConfig {
        latency_filler: LatencyFiller::Auto,
        latency_filler_after_ms: Some(100),
        ..conv_config(base_url, false)
    };
    let orchestrator =
        Arc::new(ConversationOrchestrator::new("session-poison", cfg, vm.clone()).expect("orch"));
    orchestrator
        .run_turn("what is my balance")
        .await
        .expect("masking poison test turn should complete");
    tokio::time::sleep(Duration::from_millis(50)).await;

    // A filler fired (slow turn past the 100ms threshold).
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| is_filler(s)),
        "filler should have fired: {spoken:?}"
    );
    // The session must NOT be stuck non-interruptible after the masked turn.
    assert!(
        !vm.is_interruption_blocked().await,
        "barge-in must remain available after a masking filler (no allow_interruption poisoning)"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// S1/S2 two-tier: complex turns route to the reasoning model, simple turns stay
// on the fast model (both sharing history).
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn two_tier_routes_complex_to_reasoning_simple_to_fast() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    // Two endpoints with distinct replies so we can see which tier ran.
    let fast_state = LlmMockState::default();
    *fast_state.reply.lock() = "Fast reply.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast_state.clone()).await;
    let reason_state = LlmMockState::default();
    *reason_state.reply.lock() = "Reasoned reply.".to_string();
    let (reason_url, _reason_server) = start_llm_mock(reason_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Auto,
        latency_filler: LatencyFiller::Off, // isolate routing from masking
        ..conv_config(fast_url, false)
    };
    let orch =
        Arc::new(ConversationOrchestrator::new("session-2tier", cfg, vm.clone()).expect("orch"));

    // Simple turn → fast tier; complex turn → reasoning tier.
    orch.run_turn("hi there")
        .await
        .expect("simple two-tier turn should complete");
    orch.run_turn("can you calculate my refund")
        .await
        .expect("complex two-tier turn should complete");

    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("Fast reply")),
        "the simple turn must run on the fast tier: {spoken:?}"
    );
    assert!(
        spoken.iter().any(|s| s.contains("Reasoned reply")),
        "the complex turn must escalate to the reasoning tier: {spoken:?}"
    );
    assert_eq!(
        reason_state.requests.lock().len(),
        1,
        "exactly the one complex turn hit the reasoning endpoint"
    );
    assert_eq!(
        fast_state.requests.lock().len(),
        1,
        "exactly the one simple turn hit the fast endpoint"
    );
}

/// A5: a short anaphoric follow-up to a reasoning turn STICKS to the reasoning
/// tier for one turn (the dropped-thread fix) — proven through the real
/// select_tier→run_turn→tier-endpoint path.
#[tokio::test]
#[serial_test::serial]
async fn two_tier_sticky_followup_stays_on_reasoning() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast_state = LlmMockState::default();
    *fast_state.reply.lock() = "Fast reply.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast_state.clone()).await;
    let reason_state = LlmMockState::default();
    *reason_state.reply.lock() = "Reasoned reply.".to_string();
    let (reason_url, _reason_server) = start_llm_mock(reason_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Auto,
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, false)
    };
    let orch =
        Arc::new(ConversationOrchestrator::new("session-sticky", cfg, vm.clone()).expect("orch"));

    // Turn 1 escalates (keyword); turn 2 is a bare continuation that would have
    // dropped to fast under the old heuristic — it must STICK to reasoning.
    orch.run_turn("calculate my mortgage payment")
        .await
        .expect("initial sticky reasoning turn should complete");
    orch.run_turn("and the second one?")
        .await
        .expect("sticky follow-up turn should complete");

    assert_eq!(
        reason_state.requests.lock().len(),
        2,
        "the continuation must stick to the reasoning tier (no dropped thread)"
    );
    assert_eq!(
        fast_state.requests.lock().len(),
        0,
        "the fast tier must not be hit on a sticky continuation"
    );
}

/// LIVE before/after MEASUREMENT (ollama, credential-free) of a reasoning LLM on
/// the voice path. `#[ignore]` — run explicitly:
///   cargo test --features <…> --test conversation_loop -- --ignored --nocapture \
///       live_reasoning_before_after_measurement
/// Prints a before/after latency report AND validates the spoken UX (no
/// chain-of-thought leak, masking filler present, routing keeps simple turns fast).
#[tokio::test]
#[ignore]
async fn live_reasoning_before_after_measurement() {
    use std::time::Instant;
    if std::net::TcpStream::connect("127.0.0.1:11434").is_err() {
        eprintln!("[skip] ollama not reachable at 127.0.0.1:11434");
        return;
    }
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    let ollama = "http://127.0.0.1:11434/v1".to_string();
    register_mock_tts();

    // (first-audio seconds, total-turn seconds, full spoken sequence)
    async fn measure(
        orch: &Arc<ConversationOrchestrator>,
        transcript: &str,
    ) -> (f64, f64, Vec<String>) {
        reset_tts_stats();
        let t0 = Instant::now();
        let o = orch.clone();
        let tx = transcript.to_string();
        let h = tokio::spawn(async move { o.run_turn(&tx).await });
        let mut first: Option<f64> = None;
        loop {
            if first.is_none() && !TTS_STATS.spoken.lock().is_empty() {
                first = Some(t0.elapsed().as_secs_f64());
            }
            if h.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        h.await
            .expect("live reasoning measurement turn task panicked")
            .expect("live reasoning measurement turn failed");
        let total = t0.elapsed().as_secs_f64();
        let spoken = TTS_STATS.spoken.lock().clone();
        (first.unwrap_or(total), total, spoken)
    }

    // BEFORE: a reasoning model naively single-tier — no masking, no routing.
    let vm_b = build_voice_manager();
    vm_b.start().await.unwrap();
    let _ = wire_audio_egress(&vm_b).await;
    let before = Arc::new(
        ConversationOrchestrator::new(
            "before",
            ConversationConfig {
                base_url: ollama.clone(),
                model: "deepseek-r1:1.5b".into(),
                api_key: Some("ollama".into()),
                system_prompt: Some("Answer the user concisely.".into()),
                max_tokens: Some(1500), // room to think AND answer
                streaming: true,
                latency_filler: LatencyFiller::Off,
                reasoning_budget_ms: 0,
                ..Default::default()
            },
            vm_b,
        )
        .unwrap(),
    );
    let (b_simple_first, b_simple_total, b_simple_spoken) =
        measure(&before, "hi there, are you open today?").await;
    let (b_complex_first, b_complex_total, b_complex_spoken) =
        measure(&before, "how much is a 15% tip on a 60 dollar bill?").await;

    // AFTER: fast model + reasoning_model, masking auto, routing auto.
    let vm_a = build_voice_manager();
    vm_a.start().await.unwrap();
    let _ = wire_audio_egress(&vm_a).await;
    let after = Arc::new(
        ConversationOrchestrator::new(
            "after",
            ConversationConfig {
                base_url: ollama.clone(),
                model: "llama3.2:1b".into(),
                api_key: Some("ollama".into()),
                system_prompt: Some("Answer the user concisely.".into()),
                streaming: true,
                reasoning_model: Some("deepseek-r1:1.5b".into()),
                reasoning_base_url: Some(ollama.clone()),
                max_reasoning_tokens: Some(1500),
                latency_filler: LatencyFiller::Auto,
                reasoning_budget_ms: 120_000, // generous — measure the real masked answer
                ..Default::default()
            },
            vm_a,
        )
        .unwrap(),
    );
    let (a_simple_first, a_simple_total, a_simple_spoken) =
        measure(&after, "hi there, are you open today?").await;
    let (a_complex_first, a_complex_total, a_complex_spoken) =
        measure(&after, "how much is a 15% tip on a 60 dollar bill?").await;

    let leaked = |s: &[String]| {
        s.iter()
            .any(|u| u.contains("<think>") || u.contains("</think>"))
    };
    let answer_correct = a_complex_spoken
        .iter()
        .chain(b_complex_spoken.iter())
        .any(|s| s.contains('9') || s.to_lowercase().contains("nine"));

    eprintln!("\n===== WaaV reasoning-LLM voice — BEFORE / AFTER (live ollama, GB10) =====");
    eprintln!("fast=llama3.2:1b  reasoning=deepseek-r1:1.5b\n");
    eprintln!("BEFORE (reasoner naive single-tier; no masking, no routing):");
    eprintln!("  simple 'hi':   first audio {b_simple_first:6.2}s | total {b_simple_total:6.2}s");
    eprintln!("                 spoke: {b_simple_spoken:?}");
    eprintln!("  complex 'tip': first audio {b_complex_first:6.2}s | total {b_complex_total:6.2}s");
    eprintln!("                 spoke: {b_complex_spoken:?}");
    eprintln!("\nAFTER (two-tier + masking auto + routing auto):");
    eprintln!(
        "  simple 'hi':   first audio {a_simple_first:6.2}s | total {a_simple_total:6.2}s  (→ FAST tier)"
    );
    eprintln!("                 spoke: {a_simple_spoken:?}");
    eprintln!(
        "  complex 'tip': PERCEIVED first audio {a_complex_first:6.2}s | full answer {a_complex_total:6.2}s  (filler→answer)"
    );
    eprintln!("                 spoke: {a_complex_spoken:?}");
    eprintln!("\nUX / perceived-intelligence validation:");
    eprintln!(
        "  <think> chain-of-thought leaked to TTS?   BEFORE={} AFTER={}  (must be false)",
        leaked(&b_complex_spoken),
        leaked(&a_complex_spoken)
    );
    eprintln!(
        "  AFTER-complex 1st utterance is a masking filler? {}",
        a_complex_spoken
            .first()
            .map(|s| is_filler(s))
            .unwrap_or(false)
    );
    eprintln!("  reasoned answer got the math right (mentions 9/nine)? {answer_correct}");
    eprintln!(
        "  simple-turn speedup (routing):     {:5.1}x faster ({b_simple_first:.2}s → {a_simple_first:.2}s)",
        b_simple_first / a_simple_first.max(0.001)
    );
    eprintln!(
        "  complex-turn perceived speedup (masking): {:5.1}x faster ({b_complex_first:.2}s → {a_complex_first:.2}s)",
        b_complex_first / a_complex_first.max(0.001)
    );
    eprintln!("========================================================================\n");

    // Hard invariants.
    assert!(
        !leaked(&b_complex_spoken),
        "BEFORE: chain-of-thought must NEVER be spoken"
    );
    assert!(
        !leaked(&a_complex_spoken),
        "AFTER: chain-of-thought must NEVER be spoken"
    );
    assert!(
        !a_complex_spoken.is_empty(),
        "the complex turn must speak something"
    );
    assert!(
        a_simple_first < b_simple_first,
        "routing must make the simple turn faster ({a_simple_first:.2}s vs {b_simple_first:.2}s)"
    );
}

/// A5: stickiness is a true ONE-SHOT — a chain of bare continuations must NOT
/// latch the session on the reasoning tier indefinitely (the over-escalation
/// regression the review caught). Turn 1 escalates, turn 2 sticks, turn 3 drops.
#[tokio::test]
#[serial_test::serial]
async fn two_tier_stickiness_is_one_shot() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast_state = LlmMockState::default();
    *fast_state.reply.lock() = "Fast reply.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast_state.clone()).await;
    let reason_state = LlmMockState::default();
    *reason_state.reply.lock() = "Reasoned reply.".to_string();
    let (reason_url, _reason_server) = start_llm_mock(reason_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Auto,
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, false)
    };
    let orch =
        Arc::new(ConversationOrchestrator::new("session-oneshot", cfg, vm.clone()).expect("orch"));

    orch.run_turn("calculate my mortgage payment")
        .await
        .expect("initial sticky-expiration turn should complete"); // intrinsic -> reasoning
    orch.run_turn("and the second one?")
        .await
        .expect("sticky-expiration follow-up turn should complete"); // sticks -> reasoning
    orch.run_turn("and the third?")
        .await
        .expect("post-expiration fast turn should complete"); // one-shot expired -> FAST

    assert_eq!(
        reason_state.requests.lock().len(),
        2,
        "exactly the intrinsic turn + one sticky follow-up hit the reasoning tier"
    );
    assert_eq!(
        fast_state.requests.lock().len(),
        1,
        "the second bare continuation must drop back to the fast tier (no indefinite latch)"
    );
}

/// A5: everyday negated/billing speech must NOT false-escalate onto the 2×-cost
/// reasoning tier — proven end-to-end (the money-leak fix).
#[tokio::test]
#[serial_test::serial]
async fn two_tier_negation_stays_fast() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast_state = LlmMockState::default();
    *fast_state.reply.lock() = "Fast reply.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast_state.clone()).await;
    let reason_state = LlmMockState::default();
    *reason_state.reply.lock() = "Reasoned reply.".to_string();
    let (reason_url, _reason_server) = start_llm_mock(reason_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Auto,
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, false)
    };
    let orch =
        Arc::new(ConversationOrchestrator::new("session-neg", cfg, vm.clone()).expect("orch"));

    orch.run_turn("i said no refund please")
        .await
        .expect("negation routing turn should complete");

    assert_eq!(
        fast_state.requests.lock().len(),
        1,
        "negated billing speech must stay on the fast tier"
    );
    assert_eq!(
        reason_state.requests.lock().len(),
        0,
        "it must NOT false-escalate to the reasoning tier"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// P1 degradation ladder (REALTIME_REASONING.md §8): an LLM tier failure must
// NEVER be dead air or a dropped session.
// ──────────────────────────────────────────────────────────────────────────────

/// Single-tier, recoverable failure (500): no fallback tier exists ⇒ the caller
/// hears the canned apology, and the error is still classified RECOVERABLE so the
/// session survives (no fatal-stop on a transient 500).
#[tokio::test]
#[serial_test::serial]
async fn p1_single_tier_failure_speaks_canned_apology() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let st = LlmMockState::default();
    st.fail.store(true, Ordering::SeqCst); // the (only) tier is down (500)
    let (url, _server) = start_llm_mock(st.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = ConversationConfig {
        latency_filler: LatencyFiller::Off,
        ..conv_config(url, false)
    };
    let orch =
        Arc::new(ConversationOrchestrator::new("p1-apology", cfg, vm.clone()).expect("orch"));
    let fatal_fired = Arc::new(AtomicUsize::new(0));
    let ff = Arc::clone(&fatal_fired);
    orch.set_fatal_handler(Arc::new(move |_e| {
        ff.fetch_add(1, Ordering::SeqCst);
    }));

    // The real entry point classifies the error; P1 speaks the apology first.
    orch.on_stt_result(&final_result("hello")).await;
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("having trouble")),
        "the caller must hear the canned apology, got: {spoken:?}"
    );
    assert_eq!(
        fatal_fired.load(Ordering::SeqCst),
        0,
        "a 500 is RECOVERABLE — the session must not fatal-stop"
    );
}

/// Two-tier: a SIMPLE turn routes to the fast tier; the fast tier is down ⇒ the
/// ladder degrades to the healthy reasoning tier (never a canned apology when a
/// working tier exists).
#[tokio::test]
#[serial_test::serial]
async fn p1_fast_tier_failure_degrades_to_reasoning() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast = LlmMockState::default();
    fast.fail.store(true, Ordering::SeqCst); // fast tier down
    let (fast_url, _fast_server) = start_llm_mock(fast.clone()).await;
    let reason = LlmMockState::default();
    *reason.reply.lock() = "Reasoned fallback.".to_string();
    let (reason_url, _reason_server) = start_llm_mock(reason.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Auto, // "hi there" stays on fast → fast 500 → degrade
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, false)
    };
    let orch =
        Arc::new(ConversationOrchestrator::new("p1-fallback", cfg, vm.clone()).expect("orch"));

    let r = orch.run_turn("hi there").await;
    assert!(r.is_ok(), "{r:?}");
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("Reasoned fallback")),
        "a fast-tier failure must degrade to the reasoning tier, got: {spoken:?}"
    );
    assert_eq!(
        reason.requests.lock().len(),
        1,
        "the reasoning tier served the degraded turn"
    );
    // The degraded fallback CONTINUES from history — the failed fast tier already
    // staged the user message, so it must appear EXACTLY ONCE in the fallback
    // request (no duplicate-user-turn context pollution).
    let body = reason.requests.lock()[0].to_string();
    assert_eq!(
        body.matches("hi there").count(),
        1,
        "the user message must not be duplicated in the fallback request: {body}"
    );
}

/// Two-tier P1.b: a reasoner that streams NO audio within its first-audio budget
/// is cancelled and the turn degrades to the fast draft — the slow answer is
/// never spoken, and the caller is never left on a stuck reasoner.
#[tokio::test]
#[serial_test::serial]
async fn p1_reasoner_over_budget_degrades_to_fast_draft() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast = LlmMockState::default();
    *fast.reply.lock() = "Fast draft answer.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast.clone()).await;
    let reason = LlmMockState::default();
    *reason.reply.lock() = "Slow reasoned answer.".to_string();
    reason.delay_ms.store(3000, Ordering::SeqCst); // 3s ≫ the 200ms budget
    let (reason_url, _reason_server) = start_llm_mock(reason.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Always, // force the reasoner
        reasoning_budget_ms: 200,             // tiny first-audio budget
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, false)
    };
    let orch = Arc::new(ConversationOrchestrator::new("p1-budget", cfg, vm.clone()).expect("orch"));

    let r = orch.run_turn("please reason carefully about this").await;
    assert!(r.is_ok(), "{r:?}");
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("Fast draft answer")),
        "an over-budget reasoner must degrade to the fast draft, got: {spoken:?}"
    );
    assert!(
        !spoken.iter().any(|s| s.contains("Slow reasoned answer")),
        "the over-budget reasoner's answer must NOT be spoken: {spoken:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// P2 cost budget (REALTIME_REASONING.md §8): bound the spend multipliers.
// ──────────────────────────────────────────────────────────────────────────────

/// The reasoning-token ceiling clamps the reasoning tier's `max_tokens` (the
/// priciest calls) without touching the fast tier — the request that reaches the
/// reasoning endpoint carries the clamped budget.
#[tokio::test]
#[serial_test::serial]
async fn p2_reasoning_token_ceiling_clamps_reasoning_tier() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast = LlmMockState::default();
    *fast.reply.lock() = "Fast.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast.clone()).await;
    let reason = LlmMockState::default();
    *reason.reply.lock() = "Reasoned.".to_string();
    let (reason_url, _reason_server) = start_llm_mock(reason.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = ConversationConfig {
        max_tokens: Some(8000), // generous base budget
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Always, // force the reasoner
        max_reasoning_tokens: Some(2048),     // P2 ceiling ≪ base
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, false)
    };
    let orch = Arc::new(ConversationOrchestrator::new("p2-clamp", cfg, vm.clone()).expect("orch"));

    orch.run_turn("reason about this please")
        .await
        .expect("reasoning budget turn should complete");

    let reqs = reason.requests.lock().clone();
    assert_eq!(reqs.len(), 1, "the reasoning tier served the turn");
    assert_eq!(
        reqs[0]["max_tokens"].as_u64(),
        Some(2048),
        "the reasoning tier's max_tokens must be clamped to the P2 ceiling, got: {}",
        reqs[0]["max_tokens"]
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// S3 async tools (REALTIME_REASONING.md §5): a late async result is recorded to
// history and turn-id-gated — volunteered while the spawning turn is still the
// latest, suppressed once the conversation has moved on (M6).
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn s3_async_result_volunteered_when_idle_gated_when_superseded() {
    use waav_gateway::core::llm::AsyncToolResult;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let st = LlmMockState::default();
    *st.reply.lock() = "Here is your async result.".to_string();
    let (url, _server) = start_llm_mock(st.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let cfg = ConversationConfig {
        latency_filler: LatencyFiller::Off,
        ..conv_config(url, false)
    };
    let orch = Arc::new(ConversationOrchestrator::new("s3-async", cfg, vm.clone()).expect("orch"));

    // Turn 0 runs and ends → next_turn_id advances to 1, line goes idle.
    orch.run_turn("hello")
        .await
        .expect("async-result seed turn should complete");

    // CASE A — async final for the STILL-LATEST turn 0, line idle → VOLUNTEERED.
    reset_tts_stats();
    let before = st.requests.lock().len();
    orch.handle_async_final(AsyncToolResult {
        session_id: "s3-async".into(),
        turn_id: 0,
        tool_call_id: "call-1".into(),
        name: "lookup".into(),
        is_final: true,
        run_llm: true,
        value: serde_json::json!({ "answer": 42 }),
    })
    .await;
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken
            .iter()
            .any(|s| s.contains("Here is your async result")),
        "idle + still-latest turn ⇒ the async result is volunteered: {spoken:?}"
    );
    // The follow-up inference must have run AND carried the recorded result note.
    let reqs = st.requests.lock().clone();
    assert!(reqs.len() > before, "a follow-up inference must have run");
    assert!(
        reqs.last()
            .unwrap()
            .to_string()
            .contains("Async tool 'lookup' completed"),
        "the async result must be recorded into the conversation context"
    );

    // CASE B — a NEW turn starts (conversation moves on), then the OLD async final
    // arrives → SUPERSEDED ⇒ recorded but NOT spoken (never talks over the topic).
    orch.run_turn("completely different question")
        .await
        .expect("superseding async-result turn should complete");
    reset_tts_stats();
    orch.handle_async_final(AsyncToolResult {
        session_id: "s3-async".into(),
        turn_id: 0, // stale: spawned in turn 0, but the conversation moved on
        tool_call_id: "call-2".into(),
        name: "lookup".into(),
        is_final: true,
        run_llm: true,
        value: serde_json::json!({ "answer": 7 }),
    })
    .await;
    let spoken_b = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken_b.is_empty(),
        "a superseded async result must NOT talk over the new topic: {spoken_b:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// TEST 3: multi-turn history is preserved (turn 2 includes turn 1).
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn multi_turn_history_preserved() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Reply one.".to_string();
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let orchestrator =
        ConversationOrchestrator::new("session-multi", conv_config(base_url, false), vm.clone())
            .expect("orchestrator");

    // Turn 1.
    orchestrator
        .on_stt_result(&final_result("my name is Ada"))
        .await;
    // Change the canned reply so we can distinguish responses if needed.
    *llm_state.reply.lock() = "Reply two.".to_string();
    // Turn 2.
    orchestrator
        .on_stt_result(&final_result("what is my name?"))
        .await;

    let requests = llm_state.requests.lock().clone();
    assert_eq!(requests.len(), 2, "two finalized turns → two LLM requests");

    // The SECOND request's messages must include turn 1's user message AND the
    // assistant reply from turn 1, proving conversation history is carried.
    let second = &requests[1];
    let messages = second["messages"].as_array().expect("messages array");

    let contents: Vec<String> = messages
        .iter()
        .filter_map(|m| m["content"].as_str().map(|s| s.to_string()))
        .collect();

    assert!(
        contents.iter().any(|c| c.contains("my name is Ada")),
        "turn 2 request must include turn 1's user message, got: {:?}",
        contents
    );
    assert!(
        contents.iter().any(|c| c.contains("Reply one")),
        "turn 2 request must include turn 1's assistant reply, got: {:?}",
        contents
    );
    assert!(
        contents.iter().any(|c| c.contains("what is my name?")),
        "turn 2 request must include turn 2's user message, got: {:?}",
        contents
    );

    // System prompt seeded on turn 1 must persist.
    assert!(
        contents
            .iter()
            .any(|c| c.contains("You are a helpful assistant.")),
        "system prompt must persist across turns, got: {:?}",
        contents
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Bonus: the LlmClient streams tokens to a callback (the mechanism the
// orchestrator uses to pipe tokens to TTS). Uses an SSE mock.
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
async fn llm_client_streams_tokens_to_callback() {
    use axum::response::sse::{Event, Sse};
    use futures::stream;

    async fn sse_chat() -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>
    {
        let chunks = vec![
            json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}),
            json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":" world"},"finish_reason":"stop"}]}),
        ];
        let events = chunks
            .into_iter()
            .map(|c| Ok(Event::default().data(c.to_string())))
            .chain(std::iter::once(Ok(Event::default().data("[DONE]"))))
            .collect::<Vec<_>>();
        Sse::new(stream::iter(events))
    }

    let app = Router::new().route("/chat/completions", post(sse_chat));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = spawn_test_server("llm_client_streams_tokens_to_callback", async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let client = LlmClient::new(LlmClientConfig {
        base_url,
        model: "m".to_string(),
        streaming: true,
        ..Default::default()
    });

    let collected = Arc::new(Mutex::new(String::new()));
    let collected_cb = collected.clone();
    let cb: waav_gateway::core::llm::TokenCallback =
        Arc::new(move |delta: &str| collected_cb.lock().push_str(delta));

    let cancel = tokio_util::sync::CancellationToken::new();
    let resp = client
        .complete("s", "hi", Some("k"), &cancel, Some(cb))
        .await
        .expect("stream ok");

    assert_eq!(resp.content, "Hello world");
    assert_eq!(*collected.lock(), "Hello world");
}

// ──────────────────────────────────────────────────────────────────────────────
// P1.2b — EAGER end-of-turn: speculative LLM start with STAGED history.
// ──────────────────────────────────────────────────────────────────────────────

fn eager_config(base_url: String) -> ConversationConfig {
    ConversationConfig {
        eager_eot: true,
        ..conv_config(base_url, false)
    }
}

/// Confirmed speculation: the held reply is committed + spoken; the LLM is
/// called exactly ONCE; history carries the turn exactly once.
#[tokio::test]
#[serial_test::serial]
async fn eager_confirmed_speaks_and_commits_once() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "The capital is Paris.".to_string();
    llm_state.delay_ms.store(150, Ordering::SeqCst); // speculation in flight at confirm
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let saw_audio = wire_audio_egress(&vm).await;

    let orch = ConversationOrchestrator::new("eager-1", eager_config(base_url), vm).expect("orch");

    // Prediction fires before the provider final…
    orch.trigger_eager_turn("what is the capital");
    tokio::time::sleep(Duration::from_millis(30)).await;
    // …and the provider final CONFIRMS the same transcript.
    orch.on_stt_result(&final_result("what is the capital"))
        .await;

    assert_eq!(
        llm_state.requests.lock().len(),
        1,
        "confirmed eager turn must reuse the speculative call (exactly one LLM request)"
    );
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("Paris")),
        "held eager reply must be spoken on confirmation, got {spoken:?}"
    );
    assert!(saw_audio.load(Ordering::SeqCst), "audio must egress");

    // History committed exactly once: turn 2's request carries turn 1 user+assistant once.
    *llm_state.reply.lock() = "Second reply.".to_string();
    llm_state.delay_ms.store(0, Ordering::SeqCst);
    orch.on_stt_result(&final_result("and italy?")).await;
    let requests = llm_state.requests.lock().clone();
    let second = &requests[1];
    let contents: Vec<String> = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["content"].as_str().map(String::from))
        .collect();
    assert_eq!(
        contents
            .iter()
            .filter(|c| c.contains("what is the capital"))
            .count(),
        1,
        "confirmed turn committed exactly once, got {contents:?}"
    );
    assert_eq!(
        contents
            .iter()
            .filter(|c| c.contains("The capital is Paris."))
            .count(),
        1,
        "assistant reply committed exactly once, got {contents:?}"
    );
}

/// Divergent final (user kept talking): speculation is DISCARDED with zero
/// history pollution; the real turn runs with the full transcript.
#[tokio::test]
#[serial_test::serial]
async fn eager_divergent_transcript_discards_without_history_pollution() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Full answer.".to_string();
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let orch = ConversationOrchestrator::new("eager-2", eager_config(base_url), vm).expect("orch");

    orch.trigger_eager_turn("tell me about");
    tokio::time::sleep(Duration::from_millis(30)).await;
    // The user kept talking — the final transcript is longer.
    orch.on_stt_result(&final_result("tell me about France please"))
        .await;

    let requests = llm_state.requests.lock().clone();
    assert_eq!(requests.len(), 2, "speculative + real call");
    // The REAL turn's request must contain only the full transcript — the
    // discarded speculation's prefix must NOT appear as a separate user msg.
    let real = &requests[1];
    let contents: Vec<String> = real["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str().map(String::from))
        .collect();
    assert_eq!(
        contents,
        vec!["tell me about France please".to_string()],
        "staged speculation must leave NO orphan user message"
    );
}

/// A-G4 supersede: a fuller prediction REPLACES an earlier one (instead of
/// being ignored), so a case-/space-differing final still CONFIRMS — the
/// eager latency win survives the user appending words. The held reply is
/// spoken and committed exactly once with NO orphan user message.
#[tokio::test]
#[serial_test::serial]
async fn eager_supersede_confirms_on_fuller_prediction() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Paris is the capital.".to_string();
    llm_state.delay_ms.store(120, Ordering::SeqCst);
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let saw_audio = wire_audio_egress(&vm).await;

    let orch = ConversationOrchestrator::new("eager-3", eager_config(base_url), vm).expect("orch");

    // First (short) prediction…
    orch.trigger_eager_turn("what is the");
    tokio::time::sleep(Duration::from_millis(15)).await;
    // …superseded by a FULLER prediction (the user kept speaking).
    orch.trigger_eager_turn("what is the capital of france");
    tokio::time::sleep(Duration::from_millis(30)).await;
    // The provider final differs only in CASE → A-G4 normalized confirm.
    orch.on_stt_result(&final_result("What is the capital of France"))
        .await;

    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("Paris is the capital")),
        "the superseding speculation's held reply must be spoken: {spoken:?}"
    );
    assert!(saw_audio.load(Ordering::SeqCst));

    // No orphan user message: the next turn's history carries the FINAL
    // transcript exactly once (the superseded short prediction left nothing).
    *llm_state.reply.lock() = "Next.".to_string();
    llm_state.delay_ms.store(0, Ordering::SeqCst);
    orch.on_stt_result(&final_result("thanks")).await;
    let requests = llm_state.requests.lock().clone();
    let last = requests.last().unwrap();
    let users: Vec<String> = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "user")
        .filter_map(|m| m["content"].as_str().map(String::from))
        .collect();
    assert_eq!(
        users
            .iter()
            .filter(|c| c.contains("capital of France"))
            .count(),
        1,
        "exactly one capital-question user message (the FINAL transcript): {users:?}"
    );
    assert!(
        !users.iter().any(|c| c == "what is the"),
        "the superseded SHORT prediction must never be committed: {users:?}"
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// B-G5 — barge-in commits the PARTIAL assistant reply to context.
// ──────────────────────────────────────────────────────────────────────────────

/// Slow SSE mock: emits an early delta, then stalls long enough for the test
/// to barge in mid-stream, then (if still connected) finishes.
async fn start_slow_sse_llm_mock() -> (String, TestServer) {
    use axum::response::sse::{Event, Sse};
    use futures::stream;

    async fn sse_chat() -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>
    {
        let early = json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m",
            "choices":[{"index":0,"delta":{"content":"The capital of France is"},"finish_reason":null}]});
        let late = json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m",
            "choices":[{"index":0,"delta":{"content":" Paris, a city of light."},"finish_reason":"stop"}]});
        let events = stream::unfold(0u8, move |step| {
            let early = early.clone();
            let late = late.clone();
            async move {
                match step {
                    0 => Some((Ok(Event::default().data(early.to_string())), 1)),
                    1 => {
                        // Stall mid-stream: the barge-in lands here.
                        tokio::time::sleep(Duration::from_millis(2_000)).await;
                        Some((Ok(Event::default().data(late.to_string())), 2))
                    }
                    2 => Some((Ok(Event::default().data("[DONE]")), 3)),
                    _ => None,
                }
            }
        });
        Sse::new(events)
    }

    let app = Router::new().route("/chat/completions", post(sse_chat));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server("slow_sse_llm_mock", async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), server)
}

/// A7 SSE mock: streams ONE complete sentence (spoken immediately — the trailing
/// "Now" confirms the boundary), then STALLS far past the budget, then more.
async fn start_sentence_then_stall_sse_mock() -> (String, TestServer) {
    use axum::response::sse::{Event, Sse};
    use futures::stream;
    async fn sse_chat() -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>
    {
        let early = json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m",
            "choices":[{"index":0,"delta":{"content":"The answer is forty two. Now"},"finish_reason":null}]});
        let late = json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m",
            "choices":[{"index":0,"delta":{"content":" let me continue reasoning."},"finish_reason":"stop"}]});
        let events = stream::unfold(0u8, move |step| {
            let (early, late) = (early.clone(), late.clone());
            async move {
                match step {
                    0 => Some((Ok(Event::default().data(early.to_string())), 1)),
                    1 => {
                        tokio::time::sleep(Duration::from_millis(3_000)).await; // ≫ budget
                        Some((Ok(Event::default().data(late.to_string())), 2))
                    }
                    2 => Some((Ok(Event::default().data("[DONE]")), 3)),
                    _ => None,
                }
            }
        });
        Sse::new(events)
    }
    let app = Router::new().route("/chat/completions", post(sse_chat));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server("sentence_then_stall_sse_mock", async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), server)
}

/// A7 SSE mock: streams 5 sentences ~80ms apart — a HEALTHY reasoner that must
/// NOT trip a 300ms stall watchdog (each chunk resets the silence gap).
async fn start_steady_stream_sse_mock() -> (String, TestServer) {
    use axum::response::sse::{Event, Sse};
    use futures::stream;
    async fn sse_chat() -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>>
    {
        let events = stream::unfold(0u8, move |step| async move {
            if step < 5 {
                tokio::time::sleep(Duration::from_millis(80)).await;
                let fin = if step == 4 { "stop" } else { "null" };
                let chunk = json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m",
                    "choices":[{"index":0,
                        "delta":{"content":format!("Sentence number {step} here. ")},
                        "finish_reason": if fin=="stop" {serde_json::Value::String("stop".into())} else {serde_json::Value::Null}}]});
                Some((Ok(Event::default().data(chunk.to_string())), step + 1))
            } else if step == 5 {
                Some((Ok(Event::default().data("[DONE]")), 6))
            } else {
                None
            }
        });
        Sse::new(events)
    }
    let app = Router::new().route("/chat/completions", post(sse_chat));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = spawn_test_server("steady_stream_sse_mock", async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://127.0.0.1:{}", addr.port()), server)
}

/// A7: a reasoner that streams a coherent partial sentence then STALLS past the
/// budget must commit the partial and NOT restart on the fast tier (no
/// talk-over) — the dead-air-mid-sentence case D3 masking can't cover.
#[tokio::test]
#[serial_test::serial]
async fn a7_reasoner_stall_after_partial_commits_not_restart() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast = LlmMockState::default();
    *fast.reply.lock() = "Fast draft answer.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast.clone()).await;
    let (reason_url, _reason_server) = start_sentence_then_stall_sse_mock().await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Always,
        reasoning_budget_ms: 300, // tiny silence-gap bound
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, true) // STREAMING reasoning tier
    };
    let orch = Arc::new(ConversationOrchestrator::new("a7-stall", cfg, vm.clone()).expect("orch"));

    let t0 = std::time::Instant::now();
    orch.run_turn("reason about this please")
        .await
        .expect("stalling reasoner turn should complete");
    let elapsed = t0.elapsed();

    assert!(
        elapsed < Duration::from_millis(2_500),
        "the watchdog must fire well before the 3s stall ends, took {elapsed:?}"
    );
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("forty two")),
        "the coherent partial must have been spoken: {spoken:?}"
    );
    assert!(
        !spoken.iter().any(|s| s.contains("Fast draft answer")),
        "a stall AFTER partial audio must NOT restart on the fast tier: {spoken:?}"
    );
    assert!(
        !spoken.iter().any(|s| s.contains("continue reasoning")),
        "the post-stall remainder must never be spoken: {spoken:?}"
    );
}

/// A7: a healthy reasoner streaming steadily (gaps under the budget) must NOT
/// trip the stall watchdog — it speaks its whole answer, never degrades.
#[tokio::test]
#[serial_test::serial]
async fn a7_healthy_stream_does_not_trip_watchdog() {
    use waav_gateway::core::conversation::RoutingMode;
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let fast = LlmMockState::default();
    *fast.reply.lock() = "Fast draft answer.".to_string();
    let (fast_url, _fast_server) = start_llm_mock(fast.clone()).await;
    let (reason_url, _reason_server) = start_steady_stream_sse_mock().await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let cfg = ConversationConfig {
        reasoning_model: Some("reasoning-mock".to_string()),
        reasoning_base_url: Some(reason_url),
        reasoning_route: RoutingMode::Always,
        reasoning_budget_ms: 300, // each ~80ms chunk resets the gap
        latency_filler: LatencyFiller::Off,
        ..conv_config(fast_url, true)
    };
    let orch =
        Arc::new(ConversationOrchestrator::new("a7-healthy", cfg, vm.clone()).expect("orch"));

    orch.run_turn("reason about this please")
        .await
        .expect("healthy streaming reasoner turn should complete");

    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("Sentence number 4")),
        "a steadily-streaming reasoner must speak its whole answer: {spoken:?}"
    );
    assert!(
        !spoken.iter().any(|s| s.contains("Fast draft answer")),
        "a healthy stream must NOT degrade to the fast tier: {spoken:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn barge_in_commits_partial_assistant_text() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let (base_url, _server) = start_slow_sse_llm_mock().await;
    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = conv_config(base_url.clone(), true);
    let llm = Arc::new(LlmClient::new(LlmClientConfig {
        base_url,
        model: "mock-llm".to_string(),
        api_key: Some("test".to_string()),
        system_prompt: cfg.system_prompt.clone(),
        streaming: true,
        ..Default::default()
    }));
    let orchestrator = Arc::new(ConversationOrchestrator::with_client(
        "session-partial",
        cfg,
        llm.clone(),
        vm.clone(),
    ));

    let orch1 = orchestrator.clone();
    let turn = tokio::spawn(async move { orch1.run_turn("what is the capital of France?").await });
    // Let the first delta arrive, then barge in mid-stream.
    tokio::time::sleep(Duration::from_millis(400)).await;
    orchestrator.handle_barge_in().await;
    tokio::time::timeout(Duration::from_secs(3), turn)
        .await
        .expect("partial-history turn should settle within 3s")
        .expect("partial-history turn task should not panic")
        .expect("partial-history turn should complete successfully");

    let history = llm.history_snapshot("session-partial").await;
    let last = history.last().expect("history must not be empty");
    assert!(
        matches!(last.role, waav_gateway::core::llm::MessageRole::Assistant),
        "the cut-off turn must end with the PARTIAL assistant message, got {history:?}"
    );
    let content = last.content.as_deref().unwrap_or_default();
    assert!(
        content.contains("The capital of France is") && !content.contains("city of light"),
        "history must hold exactly the streamed portion, got {content:?}"
    );
    // Exactly ONE assistant message (no double commit).
    let assistants = history
        .iter()
        .filter(|m| matches!(m.role, waav_gateway::core::llm::MessageRole::Assistant))
        .count();
    assert_eq!(assistants, 1);
}

#[tokio::test]
#[serial_test::serial]
async fn barge_in_before_any_token_commits_nothing() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    // Slow NON-streamed body: cancelled before any token is seen.
    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Never delivered.".to_string();
    llm_state.delay_ms.store(1_500, Ordering::SeqCst);
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = conv_config(base_url.clone(), true);
    let llm = Arc::new(LlmClient::new(LlmClientConfig {
        base_url,
        model: "mock-llm".to_string(),
        api_key: Some("test".to_string()),
        system_prompt: cfg.system_prompt.clone(),
        streaming: true,
        ..Default::default()
    }));
    let orchestrator = Arc::new(ConversationOrchestrator::with_client(
        "session-nopartial",
        cfg,
        llm.clone(),
        vm.clone(),
    ));

    let orch1 = orchestrator.clone();
    let turn = tokio::spawn(async move { orch1.run_turn("hello?").await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    orchestrator.handle_barge_in().await;
    tokio::time::timeout(Duration::from_secs(3), turn)
        .await
        .expect("non-partial-history turn should settle within 3s")
        .expect("non-partial-history turn task should not panic")
        .expect("non-partial-history turn should complete successfully");

    let history = llm.history_snapshot("session-nopartial").await;
    assert!(
        !history
            .iter()
            .any(|m| matches!(m.role, waav_gateway::core::llm::MessageRole::Assistant)),
        "no tokens streamed → nothing to commit, got {history:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn normal_completion_does_not_double_commit() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Complete answer.".to_string();
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let cfg = conv_config(base_url.clone(), false);
    let llm = Arc::new(LlmClient::new(LlmClientConfig {
        base_url,
        model: "mock-llm".to_string(),
        api_key: Some("test".to_string()),
        system_prompt: cfg.system_prompt.clone(),
        streaming: false,
        ..Default::default()
    }));
    let orchestrator = Arc::new(ConversationOrchestrator::with_client(
        "session-noduple",
        cfg,
        llm.clone(),
        vm.clone(),
    ));
    orchestrator.run_turn("hi").await.expect("turn");

    let history = llm.history_snapshot("session-noduple").await;
    let assistants: Vec<_> = history
        .iter()
        .filter(|m| matches!(m.role, waav_gateway::core::llm::MessageRole::Assistant))
        .collect();
    assert_eq!(
        assistants.len(),
        1,
        "exactly one assistant message: {history:?}"
    );
    assert_eq!(assistants[0].content.as_deref(), Some("Complete answer."));
}

// ──────────────────────────────────────────────────────────────────────────────
// Review wf_6783a4b3: tool-loop turns must SPEAK the final answer even when
// the round-0 response streamed a preamble, and a cancel inside the loop
// must not double-commit the preamble.
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn tool_loop_final_answer_is_spoken_after_streamed_preamble() {
    use serde_json::json;
    use waav_gateway::core::llm::{FunctionRegistry, ToolDefinition};

    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    // Scripted mock: req 1 → SSE preamble + tool call; req 2 → final answer.
    use axum::response::sse::{Event, Sse};
    use futures::stream;
    #[derive(Clone, Default)]
    struct Hits(Arc<AtomicUsize>);
    async fn sse_chat(
        axum::extract::State(hits): axum::extract::State<Hits>,
    ) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
        let n = hits.0.fetch_add(1, Ordering::SeqCst);
        let chunks: Vec<Value> = if n == 0 {
            vec![
                json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m",
                    "choices":[{"index":0,"delta":{"content":"Let me check the weather."},"finish_reason":null}]}),
                json!({"id":"1","object":"chat.completion.chunk","created":1,"model":"m",
                    "choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_w","type":"function",
                        "function":{"name":"get_weather","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
            ]
        } else {
            vec![
                json!({"id":"2","object":"chat.completion.chunk","created":1,"model":"m",
                "choices":[{"index":0,"delta":{"content":"It is sunny and minus three."},"finish_reason":"stop"}]}),
            ]
        };
        let events = chunks
            .into_iter()
            .map(|c| Ok(Event::default().data(c.to_string())))
            .chain(std::iter::once(Ok(Event::default().data("[DONE]"))))
            .collect::<Vec<_>>();
        Sse::new(stream::iter(events))
    }
    let hits = Hits::default();
    let app = Router::new()
        .route("/chat/completions", post(sse_chat))
        .with_state(hits.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = spawn_test_server("tool_loop_streaming_mock", async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let registry = Arc::new(FunctionRegistry::new());
    registry.register(
        ToolDefinition::function("get_weather", "weather", json!({"type":"object"})),
        Arc::new(|_p| Box::pin(async { Ok(json!({"weather":"sunny","temp_c":-3})) })),
    );
    let cfg = conv_config(base_url.clone(), true);
    let llm = Arc::new(
        LlmClient::new(LlmClientConfig {
            base_url,
            model: "mock-llm".to_string(),
            api_key: Some("test".into()),
            system_prompt: cfg.system_prompt.clone(),
            streaming: true,
            ..Default::default()
        })
        .with_functions(Arc::clone(&registry)),
    );
    let orchestrator = Arc::new(ConversationOrchestrator::with_client(
        "session-toolspeak",
        cfg,
        llm.clone(),
        vm.clone(),
    ));
    orchestrator
        .run_turn("what's the weather?")
        .await
        .expect("turn");

    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken
            .iter()
            .any(|s| s.contains("Let me check the weather")),
        "preamble spoken: {spoken:?}"
    );
    assert!(
        spoken.iter().any(|s| s.contains("sunny and minus three")),
        "the tool loop's FINAL answer must reach TTS (spoke-flag fix): {spoken:?}"
    );

    // History shape: user, assistant{preamble+call}, tool, assistant(final)
    // — and the preamble appears EXACTLY once (double-commit fix).
    let history = llm.history_snapshot("session-toolspeak").await;
    let preambles = history
        .iter()
        .filter(|m| {
            m.content
                .as_deref()
                .unwrap_or_default()
                .contains("Let me check the weather")
        })
        .count();
    assert_eq!(preambles, 1, "preamble committed exactly once: {history:?}");
}

// ──────────────────────────────────────────────────────────────────────────────
// A-G7 — idle re-engagement.
// ──────────────────────────────────────────────────────────────────────────────
fn idle_config(base_url: String, timeout_ms: u64) -> ConversationConfig {
    ConversationConfig {
        user_idle_timeout_ms: timeout_ms,
        ..conv_config(base_url, false)
    }
}

#[tokio::test]
#[serial_test::serial]
async fn idle_fires_after_bot_silence() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "Are you still there?".to_string();
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;

    let orchestrator = Arc::new(
        ConversationOrchestrator::new("session-idle", idle_config(base_url, 80), vm.clone())
            .expect("orchestrator"),
    );

    // User activity arms the timer; then silence.
    orchestrator.poke_idle_timer();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let requests = llm_state.requests.lock().clone();
    assert_eq!(
        requests.len(),
        1,
        "exactly one idle re-engagement inference"
    );
    let msgs = requests[0]["messages"].as_array().unwrap();
    assert!(
        msgs.iter().any(|m| m["content"]
            .as_str()
            .unwrap_or_default()
            .contains("silent for a while")),
        "re-engagement prompt present: {msgs:?}"
    );
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("Are you still there")),
        "the re-engagement reply must be spoken: {spoken:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn idle_suppressed_mid_turn_and_disabled_when_zero() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    // Disabled (0): no inference ever.
    let llm_state = LlmMockState::default();
    *llm_state.reply.lock() = "never".to_string();
    let (base_url, _server) = start_llm_mock(llm_state.clone()).await;
    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let orch = Arc::new(
        ConversationOrchestrator::new("session-idle-off", idle_config(base_url, 0), vm.clone())
            .expect("orchestrator"),
    );
    orch.poke_idle_timer();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(llm_state.requests.lock().is_empty(), "0 = disabled");

    // Mid-turn suppression: a slow LLM turn is in flight when the timer
    // fires — it must NOT inject a second inference until the turn ends.
    let llm_state2 = LlmMockState::default();
    *llm_state2.reply.lock() = "slow answer".to_string();
    llm_state2.delay_ms.store(300, Ordering::SeqCst);
    let (base_url2, _server2) = start_llm_mock(llm_state2.clone()).await;
    let orch2 = Arc::new(
        ConversationOrchestrator::new("session-idle-busy", idle_config(base_url2, 60), vm.clone())
            .expect("orchestrator"),
    );
    let o = orch2.clone();
    let turn = tokio::spawn(async move { o.run_turn("a question").await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    orch2.poke_idle_timer(); // fires at +60ms — mid-turn
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(
        llm_state2.requests.lock().len(),
        1,
        "no idle inference while a turn is active"
    );
    tokio::time::timeout(Duration::from_secs(2), turn)
        .await
        .expect("active turn should settle within 2s")
        .expect("active turn task should not panic")
        .expect("active turn should complete successfully");
}

// ──────────────────────────────────────────────────────────────────────────────
// D-G3 — fatal vs recoverable turn errors.
// ──────────────────────────────────────────────────────────────────────────────
#[tokio::test]
#[serial_test::serial]
async fn recoverable_stage_error_aborts_turn_not_session() {
    use axum::http::StatusCode;

    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    // Mock that 500s once, then answers.
    #[derive(Clone, Default)]
    struct FlakyState {
        hits: Arc<AtomicUsize>,
    }
    async fn chat(
        State(st): State<FlakyState>,
        Json(_req): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        if st.hits.fetch_add(1, Ordering::SeqCst) == 0 {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error":"boom"})),
            );
        }
        (
            StatusCode::OK,
            Json(json!({
                "id":"r","object":"chat.completion","created":0,"model":"m",
                "choices":[{"index":0,"message":{"role":"assistant","content":"recovered"},"finish_reason":"stop"}]
            })),
        )
    }
    let st = FlakyState::default();
    let app = Router::new()
        .route("/chat/completions", post(chat))
        .with_state(st.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = spawn_test_server("recoverable_stage_error_mock", async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let orch = Arc::new(
        ConversationOrchestrator::new("session-flaky", conv_config(base_url, false), vm.clone())
            .expect("orchestrator"),
    );
    let fatal_fired = Arc::new(AtomicUsize::new(0));
    let ff = Arc::clone(&fatal_fired);
    orch.set_fatal_handler(Arc::new(move |_e| {
        ff.fetch_add(1, Ordering::SeqCst);
    }));

    // Turn 1: 500 → recoverable, no fatal, session continues.
    orch.on_stt_result(&final_result("first question")).await;
    assert_eq!(
        fatal_fired.load(Ordering::SeqCst),
        0,
        "a 500 is RECOVERABLE"
    );
    // Turn 2 on the SAME session: works.
    orch.on_stt_result(&final_result("second question")).await;
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("recovered")),
        "the session must survive a transient turn error: {spoken:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn fatal_stage_error_fires_handler_once() {
    use axum::http::StatusCode;

    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mock_tts();
    reset_tts_stats();

    async fn chat(Json(_req): Json<Value>) -> (StatusCode, Json<Value>) {
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":{"message":"Incorrect API key provided"}})),
        )
    }
    let app = Router::new().route("/chat/completions", post(chat));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _server = spawn_test_server("fatal_stage_error_mock", async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base_url = format!("http://127.0.0.1:{}", addr.port());

    let vm = build_voice_manager();
    vm.start().await.expect("vm start");
    let _ = wire_audio_egress(&vm).await;
    let orch = Arc::new(
        ConversationOrchestrator::new("session-fatal", conv_config(base_url, false), vm.clone())
            .expect("orchestrator"),
    );
    let fatal_fired = Arc::new(AtomicUsize::new(0));
    let ff = Arc::clone(&fatal_fired);
    orch.set_fatal_handler(Arc::new(move |e| {
        assert!(e.to_lowercase().contains("401") || e.contains("Incorrect API key"));
        ff.fetch_add(1, Ordering::SeqCst);
    }));

    orch.on_stt_result(&final_result("hello?")).await;
    assert_eq!(
        fatal_fired.load(Ordering::SeqCst),
        1,
        "401 fires the fatal handler"
    );
    // P1 composes with the fatal-stop: the caller hears the apology BEFORE the
    // session tears down — a fatal stop is never silent.
    let spoken = TTS_STATS.spoken.lock().clone();
    assert!(
        spoken.iter().any(|s| s.contains("having trouble")),
        "P1: even a fatal-stop turn speaks the canned apology, got: {spoken:?}"
    );
    // A second failing turn does NOT re-fire (handler taken once).
    orch.on_stt_result(&final_result("still there?")).await;
    assert_eq!(
        fatal_fired.load(Ordering::SeqCst),
        1,
        "fatal fires exactly once"
    );
}
