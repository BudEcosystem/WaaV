//! LIVE LATENCY HARNESS — attributes the end-of-speech → first-audio budget.
//!
//! Goal: natural conversation wants response latency < ~200 ms. This harness
//! measures WHERE the wall-clock goes through the REAL WaaV pipeline so we can
//! separate the part WaaV controls (orchestration glue, sentence assembly,
//! streaming overlap) from the part the providers/network impose.
//!
//! Two tiers:
//!   * Scenario A (default suite, no network): the real `ConversationOrchestrator`
//!     + real `VoiceManager` driven against an in-process OpenAI-compatible LLM
//!     mock with CALIBRATED delays + an instant TTS mock. Subtracting the injected
//!     delays isolates WaaV's own overhead, and streaming-vs-batch quantifies the
//!     token-level overlap win.
//!   * Scenario B (`#[ignore]`, needs SARVAM_API_KEY + network): WaaV's real
//!     `LlmClient` streaming against live Sarvam — the true provider TTFT measured
//!     THROUGH WaaV's SSE parsing.
//!
//! Run:
//!   cargo test --features dag-routing,openapi --test latency_harness -- --nocapture
//!   SARVAM_API_KEY=… cargo test --features dag-routing,openapi --test latency_harness -- --ignored --nocapture

use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::response::sse::{Event, Sse};
use axum::{Json, Router, extract::State, routing::post};
use futures::stream;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use waav_gateway::core::conversation::{ConversationConfig, ConversationOrchestrator};
use waav_gateway::core::llm::{LlmClient, LlmClientConfig, TokenCallback};
use waav_gateway::core::stt::{BaseSTT, STTConfig, STTError, STTResult};
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSResult};
use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
use waav_gateway::global_registry;
use waav_gateway::plugin::metadata::ProviderMetadata;

// ──────────────────────────────────────────────────────────────────────────────
// Shared wall-clock recorder: one monotonic t0 ("end of speech") + named marks.
// ──────────────────────────────────────────────────────────────────────────────
#[derive(Default)]
struct Recorder {
    t0: Mutex<Option<Instant>>,
    marks: Mutex<Vec<(String, f64)>>,
}
impl Recorder {
    fn start(&self) {
        *self.t0.lock() = Some(Instant::now());
        self.marks.lock().clear();
    }
    fn mark(&self, label: &str) {
        let t0 = *self.t0.lock();
        if let Some(t0) = t0 {
            // first write wins per label (first-of-turn semantics)
            let mut m = self.marks.lock();
            if !m.iter().any(|(l, _)| l == label) {
                m.push((label.to_string(), t0.elapsed().as_secs_f64() * 1000.0));
            }
        }
    }
    fn get(&self, label: &str) -> Option<f64> {
        self.marks
            .lock()
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| *v)
    }
}

static REC: once_cell::sync::Lazy<Arc<Recorder>> =
    once_cell::sync::Lazy::new(|| Arc::new(Recorder::default()));

// ──────────────────────────────────────────────────────────────────────────────
// Instant TTS mock: marks `tts_request` on speak() and `audio_out` on first frame.
// ──────────────────────────────────────────────────────────────────────────────
const TTS_MARKER: &[u8] = b"HARNESS_AUDIO";

struct MockTts {
    ready: bool,
    callback: Option<Arc<dyn AudioCallback>>,
}
#[async_trait::async_trait]
impl BaseTTS for MockTts {
    fn new(_c: TTSConfig) -> TTSResult<Self> {
        Ok(Self { ready: false, callback: None })
    }
    async fn connect(&mut self) -> TTSResult<()> {
        self.ready = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> TTSResult<()> {
        self.ready = false;
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.ready
    }
    async fn speak(&mut self, _text: &str, _flush: bool) -> TTSResult<()> {
        REC.mark("tts_request");
        if let Some(cb) = &self.callback {
            cb.on_audio(AudioData {
                data: TTS_MARKER.to_vec(),
                sample_rate: 24000,
                format: "pcm16".to_string(),
                duration_ms: Some(20),
            })
            .await;
            cb.on_complete().await;
        }
        Ok(())
    }
    async fn flush(&self) -> TTSResult<()> {
        Ok(())
    }
    async fn clear(&mut self) -> TTSResult<()> {
        Ok(())
    }
    fn on_audio(&mut self, cb: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.callback = Some(cb);
        Ok(())
    }
    fn get_provider_info(&self) -> serde_json::Value {
        json!({ "provider": "mock-tts-harness" })
    }
}

struct NoopStt {
    ready: bool,
}
#[async_trait::async_trait]
impl waav_gateway::core::stt::BaseSTT for NoopStt {
    fn new(_c: STTConfig) -> Result<Self, STTError> {
        Ok(Self { ready: false })
    }
    async fn connect(&mut self) -> Result<(), STTError> {
        self.ready = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> Result<(), STTError> {
        self.ready = false;
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.ready
    }
    async fn send_audio(&mut self, _a: bytes::Bytes) -> Result<(), STTError> {
        Ok(())
    }
    async fn on_result(
        &mut self,
        _cb: waav_gateway::core::stt::STTResultCallback,
    ) -> Result<(), STTError> {
        Ok(())
    }
    async fn on_error(
        &mut self,
        _cb: waav_gateway::core::stt::STTErrorCallback,
    ) -> Result<(), STTError> {
        Ok(())
    }
    fn get_config(&self) -> Option<&STTConfig> {
        None
    }
    async fn update_config(&mut self, _c: STTConfig) -> Result<(), STTError> {
        Ok(())
    }
    fn get_provider_info(&self) -> &'static str {
        "mock-stt-harness"
    }
}

fn register_mocks() {
    let reg = global_registry();
    reg.register_tts(
        "mock-tts-harness",
        Arc::new(|c: TTSConfig| MockTts::new(c).map(|t| Box::new(t) as Box<dyn BaseTTS>)),
        ProviderMetadata::tts("mock-tts-harness", "Mock TTS (harness)"),
    );
    reg.register_stt(
        "mock-stt-harness",
        Arc::new(|c: STTConfig| {
            NoopStt::new(c).map(|s| Box::new(s) as Box<dyn waav_gateway::core::stt::BaseSTT>)
        }),
        ProviderMetadata::stt("mock-stt-harness", "No-op STT (harness)"),
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// OpenAI-compatible LLM mock with calibrated timing.
//   ttft_ms     — delay before the FIRST token (models provider TTFT)
//   inter_ms    — delay between subsequent tokens (models generation speed)
//   The reply is tokenized so the FIRST sentence completes early → the streaming
//   orchestrator can speak it while the rest still "generates".
// ──────────────────────────────────────────────────────────────────────────────
#[derive(Clone)]
struct LlmMock {
    ttft_ms: Arc<AtomicUsize>,
    inter_ms: Arc<AtomicUsize>,
    tokens: Arc<Mutex<Vec<String>>>,
}
impl Default for LlmMock {
    fn default() -> Self {
        Self {
            ttft_ms: Arc::new(AtomicUsize::new(0)),
            inter_ms: Arc::new(AtomicUsize::new(0)),
            tokens: Arc::new(Mutex::new(default_tokens())),
        }
    }
}
fn default_tokens() -> Vec<String> {
    // First sentence ends at "fifteen." so the streaming path can speak early.
    vec![
        "You ", "should ", "leave ", "by ", "eight ", "fifteen. ", "That ", "gives ", "you ",
        "plenty ", "of ", "time.",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

async fn start_llm_mock(state: LlmMock) -> String {
    async fn chat(
        State(state): State<LlmMock>,
        Json(req): Json<Value>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        REC.mark("llm_request");
        let streaming = req.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
        let ttft = state.ttft_ms.load(Ordering::SeqCst) as u64;
        let inter = state.inter_ms.load(Ordering::SeqCst) as u64;
        let tokens = state.tokens.lock().clone();

        if streaming {
            let n = tokens.len();
            let s = stream::unfold(0usize, move |i| {
                let tokens = tokens.clone();
                async move {
                    if i > n {
                        return None;
                    }
                    if i == 0 {
                        if ttft > 0 {
                            tokio::time::sleep(Duration::from_millis(ttft)).await;
                        }
                        REC.mark("llm_first_token");
                    } else if i < n {
                        if inter > 0 {
                            tokio::time::sleep(Duration::from_millis(inter)).await;
                        }
                    }
                    if i == n {
                        return Some((
                            Ok::<Event, Infallible>(Event::default().data("[DONE]")),
                            i + 1,
                        ));
                    }
                    let last = i == n - 1;
                    let chunk = json!({
                        "id":"chatcmpl-harness","object":"chat.completion.chunk","created":1,"model":"mock",
                        "choices":[{"index":0,"delta":{"content":tokens[i]},
                            "finish_reason": if last {json!("stop")} else {Value::Null}}]
                    });
                    Some((Ok(Event::default().data(chunk.to_string())), i + 1))
                }
            });
            Sse::new(s).into_response()
        } else {
            // Non-streaming: the client waits for the WHOLE reply.
            if ttft > 0 {
                tokio::time::sleep(Duration::from_millis(ttft)).await;
            }
            if inter > 0 {
                tokio::time::sleep(Duration::from_millis(inter * (tokens.len() as u64 - 1))).await;
            }
            REC.mark("llm_first_token");
            let content: String = tokens.concat();
            Json(json!({
                "id":"chatcmpl-harness","object":"chat.completion","created":1,"model":"mock",
                "choices":[{"index":0,"message":{"role":"assistant","content":content},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":1,"completion_tokens":12,"total_tokens":13}
            }))
            .into_response()
        }
    }
    let app = Router::new()
        .route("/chat/completions", post(chat))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{}", addr.port())
}

fn conv_config(base_url: String, streaming: bool) -> ConversationConfig {
    ConversationConfig {
        base_url,
        model: "mock".to_string(),
        system_prompt: Some("You are a concise voice assistant.".to_string()),
        api_key: Some("test".to_string()),
        temperature: Some(0.2),
        max_tokens: Some(64),
        streaming,
        max_history: 8,
        allow_interruption: true,
        eager_eot: false,
        provider_kind: None,
        barge_in_min_words: None,
        summarize_target_tokens: 0,
        mute_strategy: None,
        strip_markdown: true,
    }
}

fn build_vm() -> Arc<VoiceManager> {
    let stt = STTConfig {
        provider: "mock-stt-harness".to_string(),
        api_key: "test".to_string(),
        ..Default::default()
    };
    let tts = TTSConfig {
        provider: "mock-tts-harness".to_string(),
        api_key: "test".to_string(),
        ..Default::default()
    };
    Arc::new(VoiceManager::new(VoiceManagerConfig::new(stt, tts), None).expect("vm"))
}

async fn wire_egress(vm: &Arc<VoiceManager>) {
    vm.on_tts_audio(move |audio: AudioData| {
        Box::pin(async move {
            if audio.data.windows(TTS_MARKER.len()).any(|w| w == TTS_MARKER) {
                REC.mark("audio_out");
            }
        })
    })
    .await
    .expect("egress cb");
}

fn final_result(text: &str) -> STTResult {
    STTResult::new(text.to_string(), true, true, 0.99)
}

/// Drive one orchestrated turn and return the recorded stage marks (ms from EOS).
async fn run_one_turn(streaming: bool, ttft_ms: usize, inter_ms: usize) -> Vec<(String, f64)> {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mocks();
    let llm = LlmMock::default();
    llm.ttft_ms.store(ttft_ms, Ordering::SeqCst);
    llm.inter_ms.store(inter_ms, Ordering::SeqCst);
    let base_url = start_llm_mock(llm).await;

    let vm = build_vm();
    vm.start().await.expect("vm start");
    wire_egress(&vm).await;

    let orch = ConversationOrchestrator::new("lat-session", conv_config(base_url, streaming), vm)
        .expect("orchestrator");

    REC.start(); // t0 = end of speech (stt_final)
    orch.on_stt_result(&final_result("what time should I leave?")).await;

    // The streaming path speaks via a SPAWNED pump task that runs concurrently
    // with (and outlives) on_stt_result — poll for first audio rather than racing it.
    for _ in 0..400 {
        if REC.get("audio_out").is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    REC.marks.lock().clone()
}

fn pctl(mut v: Vec<f64>, p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((v.len() as f64 * p) as usize).min(v.len() - 1);
    v[idx]
}

// ══════════════════════════════════════════════════════════════════════════════
// SCENARIO A1 — PURE GATEWAY OVERHEAD (zero injected provider delay).
// Everything the wall-clock shows here is WaaV's own orchestration cost.
// ══════════════════════════════════════════════════════════════════════════════
#[tokio::test]
#[serial_test::serial]
async fn a1_pure_gateway_overhead_streaming() {
    let mut totals = Vec::new();
    let mut to_llm = Vec::new();
    for _ in 0..20 {
        let m = run_one_turn(true, 0, 0).await;
        let get = |l: &str| m.iter().find(|(k, _)| k == l).map(|(_, v)| *v);
        if let Some(a) = get("audio_out") {
            totals.push(a);
        }
        if let Some(r) = get("llm_request") {
            to_llm.push(r);
        }
    }
    println!("\n=== A1 PURE GATEWAY OVERHEAD (streaming, 0ms provider) ===");
    println!(
        "  stt_final→llm_request : p50={:.2}ms p90={:.2}ms",
        pctl(to_llm.clone(), 0.5),
        pctl(to_llm.clone(), 0.9)
    );
    println!(
        "  stt_final→audio_out   : p50={:.2}ms p90={:.2}ms p99={:.2}ms  (n={})",
        pctl(totals.clone(), 0.5),
        pctl(totals.clone(), 0.9),
        pctl(totals.clone(), 0.99),
        totals.len()
    );
    assert!(!totals.is_empty(), "no audio_out recorded");
    // WaaV's own glue must be a small fraction of a 200ms budget.
    assert!(
        pctl(totals.clone(), 0.9) < 100.0,
        "gateway overhead unexpectedly high: p90={:.2}ms",
        pctl(totals, 0.9)
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// SCENARIO A2 — STREAMING-vs-BATCH OVERLAP with a realistic LLM TTFT.
// Shows how much earlier first-audio starts when the first sentence is spoken
// while the LLM is still generating.
// ══════════════════════════════════════════════════════════════════════════════
#[tokio::test]
#[serial_test::serial]
async fn a2_streaming_vs_batch_overlap() {
    // Realistic: 300ms LLM TTFT, 25ms/token generation.
    let (ttft, inter) = (300usize, 25usize);

    let mut stream_audio = Vec::new();
    let mut stream_speak = Vec::new();
    for _ in 0..8 {
        let m = run_one_turn(true, ttft, inter).await;
        let get = |l: &str| m.iter().find(|(k, _)| k == l).map(|(_, v)| *v);
        if let Some(a) = get("audio_out") {
            stream_audio.push(a);
        }
        if let Some(s) = get("tts_request") {
            stream_speak.push(s);
        }
    }

    let mut batch_audio = Vec::new();
    for _ in 0..8 {
        let m = run_one_turn(false, ttft, inter).await;
        if let Some(a) = m.iter().find(|(k, _)| k == "audio_out").map(|(_, v)| *v) {
            batch_audio.push(a);
        }
    }

    let total_tokens = default_tokens().len() as f64;
    let full_gen_ms = ttft as f64 + inter as f64 * (total_tokens - 1.0);
    println!("\n=== A2 STREAMING vs BATCH (TTFT={ttft}ms, {inter}ms/token, {total_tokens} tokens) ===");
    println!("  LLM full-generation time          : {full_gen_ms:.0}ms");
    println!(
        "  STREAMING first-audio (audio_out) : p50={:.1}ms  (first sentence spoken at p50={:.1}ms)",
        pctl(stream_audio.clone(), 0.5),
        pctl(stream_speak.clone(), 0.5)
    );
    println!(
        "  BATCH first-audio (audio_out)     : p50={:.1}ms  (waits for full reply)",
        pctl(batch_audio.clone(), 0.5)
    );
    let saved = pctl(batch_audio.clone(), 0.5) - pctl(stream_audio.clone(), 0.5);
    println!("  >>> streaming overlap saves ~{saved:.0}ms of first-audio latency");
    assert!(!stream_audio.is_empty() && !batch_audio.is_empty());
    // Streaming must deliver first audio before the full generation completes.
    assert!(
        pctl(stream_audio.clone(), 0.5) < full_gen_ms,
        "streaming first-audio should precede full generation"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// SCENARIO A3 — GATEWAY OVERHEAD UNDER CONCURRENT SESSIONS.
// Each session is independent; this checks the per-turn glue doesn't degrade
// under load (lock contention, scheduler pressure).
// ══════════════════════════════════════════════════════════════════════════════
#[tokio::test]
#[serial_test::serial]
async fn a3_overhead_under_concurrency() {
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }
    register_mocks();
    let llm = LlmMock::default(); // 0 delay → measures pure concurrent glue
    let base_url = start_llm_mock(llm).await;

    for &n in &[1usize, 8, 32] {
        let start = Instant::now();
        let mut handles = Vec::new();
        for i in 0..n {
            let url = base_url.clone();
            handles.push(tokio::spawn(async move {
                let vm = build_vm();
                vm.start().await.unwrap();
                let saw = Arc::new(AtomicBool::new(false));
                let f = saw.clone();
                vm.on_tts_audio(move |audio: AudioData| {
                    let f = f.clone();
                    Box::pin(async move {
                        if audio.data.windows(TTS_MARKER.len()).any(|w| w == TTS_MARKER) {
                            f.store(true, Ordering::SeqCst);
                        }
                    })
                })
                .await
                .unwrap();
                let orch = ConversationOrchestrator::new(
                    format!("conc-{i}"),
                    conv_config(url, true),
                    vm,
                )
                .unwrap();
                let t = Instant::now();
                orch.on_stt_result(&final_result("hi")).await;
                (t.elapsed().as_secs_f64() * 1000.0, saw.load(Ordering::SeqCst))
            }));
        }
        let mut per_turn = Vec::new();
        let mut ok = 0;
        for h in handles {
            let (ms, got) = h.await.unwrap();
            per_turn.push(ms);
            if got {
                ok += 1;
            }
        }
        println!(
            "  concurrency={n:>3}: per-turn p50={:.1}ms p90={:.1}ms | wall={:.0}ms | audio_ok={ok}/{n}",
            pctl(per_turn.clone(), 0.5),
            pctl(per_turn.clone(), 0.9),
            start.elapsed().as_millis()
        );
        assert_eq!(ok, n, "every concurrent session must produce audio");
    }
    println!("(A3: per-turn glue under concurrent load)");
}

// ══════════════════════════════════════════════════════════════════════════════
// SCENARIO B — REAL SARVAM LLM TTFT through WaaV's LlmClient (live, billed).
// ══════════════════════════════════════════════════════════════════════════════
#[tokio::test]
#[ignore = "Requires SARVAM_API_KEY + network; real billed Sarvam calls"]
async fn b_real_sarvam_llm_ttft_through_waav() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let key = match std::env::var("SARVAM_API_KEY") {
        Ok(k) if !k.is_empty() => k,
        _ => {
            eprintln!("SKIP: SARVAM_API_KEY not set");
            return;
        }
    };
    let client = Arc::new(LlmClient::new(LlmClientConfig {
        base_url: "https://api.sarvam.ai/v1".to_string(),
        model: "sarvam-30b".to_string(),
        api_key: Some(key.clone()),
        system_prompt: Some("You are a concise voice assistant. Answer in one short sentence.".to_string()),
        max_tokens: Some(60),
        streaming: true,
        ..Default::default()
    }));

    let mut ttfts = Vec::new();
    let mut totals = Vec::new();
    for i in 0..5 {
        let first = Arc::new(Mutex::new(None::<f64>));
        let first_cb = first.clone();
        let t0 = Instant::now();
        let cb: TokenCallback = Arc::new(move |_delta: &str| {
            let mut f = first_cb.lock();
            if f.is_none() {
                *f = Some(t0.elapsed().as_secs_f64() * 1000.0);
            }
        });
        let cancel = tokio_util::sync::CancellationToken::new();
        let resp = client
            .complete("bench", "What time should I leave for a 9am meeting downtown?", Some(&key), &cancel, Some(cb))
            .await
            .expect("sarvam complete");
        let total = t0.elapsed().as_secs_f64() * 1000.0;
        let ttft = first.lock().unwrap_or(total);
        ttfts.push(ttft);
        totals.push(total);
        println!(
            "  run {i}: TTFT={ttft:.0}ms total={total:.0}ms reply_len={}",
            resp.content.len()
        );
    }
    println!("\n=== B REAL SARVAM (sarvam-30b, streaming, through WaaV LlmClient) ===");
    println!(
        "  TTFT  p50={:.0}ms p90={:.0}ms  | total p50={:.0}ms p90={:.0}ms",
        pctl(ttfts.clone(), 0.5),
        pctl(ttfts.clone(), 0.9),
        pctl(totals.clone(), 0.5),
        pctl(totals.clone(), 0.9)
    );
    println!("  NOTE: sarvam-30b is a REASONING model — first streamed token is reasoning, not the spoken answer.");
}

// ══════════════════════════════════════════════════════════════════════════════
// SCENARIO C — REAL COMPOSED PIPELINE through WaaV's DAG executor (live, billed).
// Deepgram STT (English) → Sarvam LLM → Deepgram Aura TTS (English). Reports the
// real per-node durations + the DAG executor's own overhead. Tolerant of the
// reasoning-model empty-content failure (still prints STT+LLM node timings).
// ══════════════════════════════════════════════════════════════════════════════
#[cfg(feature = "dag-routing")]
mod dag_e2e {
    use std::time::Instant;
    use waav_gateway::dag::compiler::DAGCompiler;
    use waav_gateway::dag::context::DAGContext;
    use waav_gateway::dag::definition::{DAGDefinition, EdgeDefinition, NodeDefinition, NodeType};
    use waav_gateway::dag::executor::DAGExecutor;
    use waav_gateway::dag::nodes::DAGData;

    const SR: u32 = 16000;
    const UTTERANCE: &str = "What time should I leave for my nine a.m. meeting downtown?";

    fn key(var: &str) -> Option<String> {
        match std::env::var(var) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => {
                eprintln!("SKIP: {var} not set");
                None
            }
        }
    }

    async fn synth_english_pcm(text: &str, dg: &str) -> Vec<u8> {
        let url = format!(
            "https://api.deepgram.com/v1/speak?model=aura-asteria-en&encoding=linear16&container=none&sample_rate={SR}"
        );
        let resp = reqwest::Client::new()
            .post(url)
            .header("Authorization", format!("Token {dg}"))
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .expect("deepgram /v1/speak");
        assert!(resp.status().is_success(), "aura: {}", resp.status());
        resp.bytes().await.unwrap().to_vec()
    }

    #[tokio::test]
    #[ignore = "Requires DEEPGRAM_API_KEY + SARVAM_API_KEY + network; real billed calls"]
    async fn c_real_composed_dag_stt_llm_tts() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (Some(dg), Some(sarvam)) = (key("DEEPGRAM_API_KEY"), key("SARVAM_API_KEY")) else {
            return;
        };

        let pcm = synth_english_pcm(UTTERANCE, &dg).await;
        println!("\n# synth English PCM: {} bytes ({:.2}s)", pcm.len(), pcm.len() as f32 / 2.0 / SR as f32);

        let stt = NodeDefinition::new(
            "stt",
            NodeType::SttProvider {
                provider: "deepgram".to_string(),
                model: Some("nova-2".to_string()),
                language: Some("en-US".to_string()),
            },
        )
        .with_config(serde_json::json!({ "api_key": "${DEEPGRAM_API_KEY}" }));

        let llm = NodeDefinition::new(
            "llm",
            NodeType::LlmEndpoint {
                base_url: "https://api.sarvam.ai/v1".to_string(),
                model: "sarvam-30b".to_string(),
                api_key: Some("${SARVAM_API_KEY}".to_string()),
                system_prompt: Some(
                    "You are a voice assistant. Reply with ONE short spoken sentence only."
                        .to_string(),
                ),
                temperature: Some(0.2),
                max_tokens: Some(3000), // reasoning model: needs headroom or content comes back empty
                streaming: false,
                tools: None,
                assistant_id: None,
                timeout_ms: Some(120_000),
                headers: Default::default(),
            },
        );

        let tts = NodeDefinition::new(
            "tts",
            NodeType::TtsProvider {
                provider: "deepgram".to_string(),
                voice_id: None,
                model: Some("aura-asteria-en".to_string()),
            },
        )
        .with_config(serde_json::json!({ "api_key": "${DEEPGRAM_API_KEY}" }));

        let mut dag = DAGDefinition::new("composed", "Deepgram STT -> Sarvam LLM -> Aura TTS");
        dag.add_node(stt);
        dag.add_node(llm);
        dag.add_node(tts);
        dag.add_edge(EdgeDefinition::new("stt", "llm"));
        dag.add_edge(EdgeDefinition::new("llm", "tts"));
        dag.with_entry("stt");
        dag.add_exit("tts");
        let compiled = DAGCompiler::new().compile(dag).expect("compile");

        // Make the env vars visible for `${VAR}` resolution in node configs.
        unsafe {
            std::env::set_var("DEEPGRAM_API_KEY", &dg);
            std::env::set_var("SARVAM_API_KEY", &sarvam);
        }

        let exec = DAGExecutor::new();
        let mut ctx = DAGContext::new("composed-latency");
        let t0 = Instant::now();
        let out = exec
            .execute_from(&compiled, "stt", DAGData::Audio(bytes::Bytes::from(pcm)), &mut ctx)
            .await;
        let total_ms = t0.elapsed().as_secs_f64() * 1000.0;

        let node_ms = |id: &str| {
            ctx.timing
                .node_durations
                .get(id)
                .map(|d| d.as_secs_f64() * 1000.0)
        };
        println!("\n=== C REAL COMPOSED DAG (Deepgram STT → Sarvam LLM → Aura TTS) ===");
        println!("  node[stt] : {:?} ms", node_ms("stt"));
        println!("  node[llm] : {:?} ms", node_ms("llm"));
        println!("  node[tts] : {:?} ms", node_ms("tts"));
        let measured: f64 = ["stt", "llm", "tts"].iter().filter_map(|n| node_ms(n)).sum();
        println!("  Σ nodes   : {measured:.0} ms | execute_from total : {total_ms:.0} ms | DAG overhead : {:.1} ms", total_ms - measured);
        match out {
            Ok(DAGData::TTSAudio(a)) => println!(
                "  ✓ full pipeline produced {} bytes of audio ({} Hz, {})",
                a.data.len(),
                a.sample_rate,
                a.format
            ),
            Ok(other) => println!("  NOTE: final output was {} (not audio)", other.type_name()),
            Err(e) => println!(
                "  NOTE: pipeline errored after node timings captured (likely reasoning-model empty content): {e}"
            ),
        }
    }

    /// Composed LLM → TTS (both HTTP) from a text turn — the response half of the
    /// pipeline, real providers, measured through the DAG executor.
    #[tokio::test]
    #[ignore = "Requires SARVAM_API_KEY + DEEPGRAM_API_KEY + network; real billed calls"]
    async fn c2_real_llm_to_tts_dag() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let (Some(dg), Some(sarvam)) = (key("DEEPGRAM_API_KEY"), key("SARVAM_API_KEY")) else {
            return;
        };
        unsafe {
            std::env::set_var("DEEPGRAM_API_KEY", &dg);
            std::env::set_var("SARVAM_API_KEY", &sarvam);
        }

        let llm = NodeDefinition::new(
            "llm",
            NodeType::LlmEndpoint {
                base_url: "https://api.sarvam.ai/v1".to_string(),
                model: "sarvam-30b".to_string(),
                api_key: Some("${SARVAM_API_KEY}".to_string()),
                system_prompt: Some("Reply with ONE short spoken sentence only.".to_string()),
                temperature: Some(0.2),
                max_tokens: Some(3000),
                streaming: false,
                tools: None,
                assistant_id: None,
                timeout_ms: Some(120_000),
                headers: Default::default(),
            },
        );
        let tts = NodeDefinition::new(
            "tts",
            NodeType::TtsProvider {
                provider: "deepgram".to_string(),
                voice_id: None,
                model: Some("aura-asteria-en".to_string()),
            },
        )
        .with_config(serde_json::json!({ "api_key": "${DEEPGRAM_API_KEY}" }));

        let mut dag = DAGDefinition::new("llm-tts", "Sarvam LLM -> Aura TTS");
        dag.add_node(llm);
        dag.add_node(tts);
        dag.add_edge(EdgeDefinition::new("llm", "tts"));
        dag.with_entry("llm");
        dag.add_exit("tts");
        let compiled = DAGCompiler::new().compile(dag).expect("compile");

        let exec = DAGExecutor::new();
        let mut ctx = DAGContext::new("llm-tts-latency");
        let t0 = Instant::now();
        let out = exec
            .execute_from(&compiled, "llm", DAGData::Text(UTTERANCE.to_string()), &mut ctx)
            .await;
        let total_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let node_ms = |id: &str| {
            ctx.timing.node_durations.get(id).map(|d| d.as_secs_f64() * 1000.0)
        };
        println!("\n=== C2 REAL LLM → TTS DAG (Sarvam LLM → Aura TTS) ===");
        println!("  node[llm] : {:?} ms", node_ms("llm"));
        println!("  node[tts] : {:?} ms", node_ms("tts"));
        println!("  execute_from total : {total_ms:.0} ms");
        match out {
            Ok(DAGData::TTSAudio(a)) => println!("  ✓ produced {} bytes of audio", a.data.len()),
            Ok(other) => println!("  NOTE: final output was {}", other.type_name()),
            Err(e) => println!("  NOTE: pipeline errored: {e}"),
        }
    }
}
