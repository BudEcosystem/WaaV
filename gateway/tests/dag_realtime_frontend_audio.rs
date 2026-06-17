//! FE-RT — REAL AUDIO through the VoiceManager AUDIO FRONT-END into a realtime (S2S)
//! DAG node, END-TO-END through the FULL running gateway.
//!
//! GAP this closes: `dag_realtime_session.rs` proves the persistent realtime DAG path,
//! but it drives the DAG with a mock STT firing CANNED transcripts — NO test feeds REAL
//! AUDIO through the VoiceManager front-end (noise reduction / silero-VAD / smart-turn /
//! turn-detect) INTO a realtime node. That seam — "real audio exercises the front-end,
//! the front-end produces a finalized turn, the turn drives the realtime node, the
//! realtime audio rides back to the client" — was untested. This test exercises it.
//!
//! WHAT THE FRONT-END DOES ON THE `/ws` AUDIO PATH (verified against the code):
//!   - `audio_handler::handle_audio_message` → `VoiceManager::receive_audio(audio)`.
//!   - In `receive_audio`, when `smart_turn_config` is set (here via the WS
//!     `turn_detection.enabled=true` knob, wired in `config_handler.rs` ~line 907),
//!     EVERY real audio frame is run through the `SmartTurnProcessor` — ingress-resample,
//!     then silero-VAD (feature "silero-vad") + smart-turn ML (feature "smart-turn"),
//!     then `notify_smart_turn(latency_us, …)`.
//!     This happens BEFORE `stt.send_audio(audio)`, so the mock STT below does NOT
//!     bypass the front-end — the VAD + smart-turn models genuinely run on this audio.
//!   - `notify_smart_turn` (latency_us > 0 ⇒ inference actually executed) reaches
//!     `FrameProfiler::on_smart_turn` → `record_smart_turn` → the Prometheus histogram
//!     `waav_smart_turn_inference_ms`, scrapable at `/metrics`. We assert its `_count`
//!     went from 0 → >0 across the audio feed: HARD, decoupled proof the VAD+smart-turn
//!     front-end ran on the real audio (independent of the mock STT).
//!   - `turn-detect` (the text end-of-turn model, `livekit/turn-detector`) is loaded by
//!     `CoreState` and attached to the VoiceManager turn path; we assert it is present
//!     (so the build's text turn-detection is live, not a timer fallback).
//!   - `noise-filter` (DeepFilterNet via the `deep_filter` crate) is applied on the
//!     gateway INGRESS in the LiveKit path (`livekit/client/events.rs`) through the
//!     `pub` production fn `crate::utils::noise_filter::reduce_noise_async`. The direct
//!     `/ws` path does not denoise, so to exercise the SAME production noise codepath on
//!     this audio we pre-process every frame through `reduce_noise_async` (exactly as the
//!     LiveKit ingress does) BEFORE sending it to `/ws`, and assert the DeepFilterNet
//!     model measurably ALTERED a noisy frame (output ≠ input) — proof the noise model
//!     actually ran on the audio that then drove the front-end + realtime node.
//!
//! THE REALTIME NODE: a DAG `realtime_provider` node FORCES `realtime_endpoint_override =
//! None` (SSRF guard), so it cannot be redirected to a URL mock. We register an IN-PROCESS
//! mock-realtime provider via `registry.register_realtime` (same pattern as
//! dag_realtime_session.rs). On each finalized turn the StreamDriver runs the DAG; the
//! realtime node emits a distinctive audio marker that rides the cascade
//! `DagOutput::Audio` sink → the DAG `audio_output` node → the client WS.
//!
//! CREDENTIAL-FREE: in-process mock STT + mock realtime + mock TTS. NO vendor keys, NO
//! network. The silero-VAD / smart-turn / turn-detect ONNX models are read from the box's
//! model cache (no download). If they are absent, the processor errors are swallowed on
//! the audio hot path and the smart-turn metric stays 0 — which this test asserts AGAINST,
//! so a missing model fails the test loudly rather than passing as a silent downgrade.

#![cfg(all(feature = "dag-routing", feature = "silero-vad", feature = "smart-turn"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    core::realtime::{
        AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback,
        RealtimeAudioData, RealtimeConfig, RealtimeErrorCallback, RealtimeResult,
        ReconnectionCallback, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
        TranscriptResult, TranscriptRole,
    },
    core::stt::{BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback},
    core::tts::{AudioCallback, BaseTTS, TTSConfig, TTSResult},
    global_registry,
    middleware::auth::auth_middleware,
    plugin::metadata::ProviderMetadata,
    routes,
    state::AppState,
};

// Process-global connect counter on the mock realtime provider: reachable only through
// `production` DAG-init inserting the `RealtimeSessionMap`, so a non-zero value proves the
// finalized turn really reached the realtime node through the live gateway.
static RT_CONNECTS: AtomicUsize = AtomicUsize::new(0);

/// Distinctive assistant-audio marker — the DAG `audio_output` node delivers exactly these
/// bytes per turn, so the WS-egress assertion is unambiguous.
const RT_AUDIO_MARKER: &[u8] = b"FE_RT_REALTIME_AUDIO_MARKER";

// ──────────────────────────────────────────────────────────────────────────────
// Mock STT: fire ONE finalized (final + speech_final) transcript per audio frame.
//
// IMPORTANT: this does NOT bypass the front-end. The VoiceManager runs the
// SmartTurnProcessor (silero-VAD + smart-turn) on each frame in `receive_audio`
// BEFORE calling `stt.send_audio`. The mock STT only stands in for the cloud STT
// network round-trip; the audio front-end still executes on the real PCM.
// ──────────────────────────────────────────────────────────────────────────────
struct MockStt {
    ready: bool,
    callback: Option<STTResultCallback>,
    turn: AtomicUsize,
}

#[async_trait::async_trait]
impl BaseSTT for MockStt {
    fn new(_config: STTConfig) -> Result<Self, STTError> {
        Ok(Self { ready: false, callback: None, turn: AtomicUsize::new(0) })
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
    async fn send_audio(&mut self, _audio_data: Bytes) -> Result<(), STTError> {
        // Each frame → one finalized turn (non-empty + speech_final drives the DAG
        // StreamDriver's AnySpeechStart→LegacySpeechFinalStop → Stopped → run DAG).
        if let Some(cb) = &self.callback {
            let n = self.turn.fetch_add(1, Ordering::SeqCst) + 1;
            let result = STTResult::new(format!("turn {n}"), true, true, 0.99);
            cb(result).await;
        }
        Ok(())
    }
    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        self.callback = Some(callback);
        Ok(())
    }
    async fn on_error(&mut self, _callback: STTErrorCallback) -> Result<(), STTError> {
        Ok(())
    }
    fn get_config(&self) -> Option<&STTConfig> {
        None
    }
    async fn update_config(&mut self, _config: STTConfig) -> Result<(), STTError> {
        Ok(())
    }
    fn get_provider_info(&self) -> &'static str {
        "mock-stt"
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Mock realtime (S2S) provider: counts connect process-globally and, on each
// `create_response`, emits — in order, all awaited — one audio chunk (the marker),
// a finalized assistant transcript, then response-done LAST (so the persistent
// node's `response_done` wait completes immediately).
// ──────────────────────────────────────────────────────────────────────────────
struct CountingRealtime {
    audio_cb: Option<AudioOutputCallback>,
    transcript_cb: Option<TranscriptCallback>,
    done_cb: Option<ResponseDoneCallback>,
    turn: usize,
    connected: bool,
}

#[async_trait::async_trait]
impl BaseRealtime for CountingRealtime {
    fn new(_config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self {
            audio_cb: None,
            transcript_cb: None,
            done_cb: None,
            turn: 0,
            connected: false,
        })
    }
    async fn connect(&mut self) -> RealtimeResult<()> {
        RT_CONNECTS.fetch_add(1, Ordering::SeqCst);
        self.connected = true;
        Ok(())
    }
    async fn disconnect(&mut self) -> RealtimeResult<()> {
        self.connected = false;
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.connected
    }
    fn get_connection_state(&self) -> ConnectionState {
        if self.connected {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }
    async fn send_audio(&mut self, _a: Bytes) -> RealtimeResult<()> {
        Ok(())
    }
    async fn send_text(&mut self, _t: &str) -> RealtimeResult<()> {
        Ok(())
    }
    async fn create_response(&mut self) -> RealtimeResult<()> {
        self.turn += 1;
        let turn = self.turn;
        if let Some(cb) = &self.audio_cb {
            cb(RealtimeAudioData {
                data: Bytes::from_static(RT_AUDIO_MARKER),
                sample_rate: 24_000,
                item_id: Some(format!("item_{turn}")),
                response_id: Some(format!("resp_{turn}")),
            })
            .await;
        }
        if let Some(cb) = &self.transcript_cb {
            cb(TranscriptResult {
                text: format!("assistant answer {turn}"),
                role: TranscriptRole::Assistant,
                is_final: true,
                item_id: Some(format!("item_{turn}")),
            })
            .await;
        }
        if let Some(cb) = &self.done_cb {
            cb(format!("resp_{turn}")).await;
        }
        Ok(())
    }
    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        Ok(())
    }
    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        Ok(())
    }
    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        Ok(())
    }
    fn on_transcript(&mut self, c: TranscriptCallback) -> RealtimeResult<()> {
        self.transcript_cb = Some(c);
        Ok(())
    }
    fn on_audio(&mut self, c: AudioOutputCallback) -> RealtimeResult<()> {
        self.audio_cb = Some(c);
        Ok(())
    }
    fn on_error(&mut self, _c: RealtimeErrorCallback) -> RealtimeResult<()> {
        Ok(())
    }
    fn on_function_call(&mut self, _c: FunctionCallCallback) -> RealtimeResult<()> {
        Ok(())
    }
    fn on_speech_event(&mut self, _c: SpeechEventCallback) -> RealtimeResult<()> {
        Ok(())
    }
    fn on_response_done(&mut self, c: ResponseDoneCallback) -> RealtimeResult<()> {
        self.done_cb = Some(c);
        Ok(())
    }
    fn on_reconnection(&mut self, _c: ReconnectionCallback) -> RealtimeResult<()> {
        Ok(())
    }
    async fn update_session(&mut self, _c: RealtimeConfig) -> RealtimeResult<()> {
        Ok(())
    }
    async fn submit_function_result(&mut self, _i: &str, _r: &str) -> RealtimeResult<()> {
        Ok(())
    }
    fn get_provider_info(&self) -> Value {
        json!({ "provider": "mock-realtime" })
    }
    // Manual mode: the gateway drives commit + create_response on the persistent node.
    fn emits_user_turn_frames(&self) -> bool {
        false
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Mock TTS: required to BOOT the VoiceManager (from `tts_config`) though the realtime
// DAG never routes through a TTS node. No-op; assistant audio comes from the realtime node.
// ──────────────────────────────────────────────────────────────────────────────
struct MockTts {
    ready: bool,
}

#[async_trait::async_trait]
impl BaseTTS for MockTts {
    fn new(_config: TTSConfig) -> TTSResult<Self> {
        Ok(Self { ready: false })
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
        Ok(())
    }
    async fn flush(&self) -> TTSResult<()> {
        Ok(())
    }
    fn on_audio(&mut self, _callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        Ok(())
    }
    fn get_provider_info(&self) -> Value {
        json!({ "provider": "mock-tts" })
    }
}

fn register_mocks() {
    let registry = global_registry();
    registry.register_stt(
        "mock-stt",
        Arc::new(|c: STTConfig| MockStt::new(c).map(|s| Box::new(s) as Box<dyn BaseSTT>)),
        ProviderMetadata::stt("mock-stt", "Mock STT"),
    );
    registry.register_tts(
        "mock-tts",
        Arc::new(|c: TTSConfig| MockTts::new(c).map(|t| Box::new(t) as Box<dyn BaseTTS>)),
        ProviderMetadata::tts("mock-tts", "Mock TTS"),
    );
    registry.register_realtime(
        "mock-realtime",
        Arc::new(|c: RealtimeConfig| {
            CountingRealtime::new(c).map(|r| Box::new(r) as Box<dyn BaseRealtime>)
        }),
        ProviderMetadata::realtime("mock-realtime", "Mock Realtime"),
    );
}

fn test_server_config() -> ServerConfig {
    ServerConfig {
        host: "127.0.0.1".to_string(),
        livekit_api_key: None,
        livekit_api_secret: None,
        port: 0,
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
        rate_limit_requests_per_second: 1000,
        rate_limit_burst_size: 1000,
        max_websocket_connections: None,
        max_connections_per_ip: 1000,
        ws_processing_timeout_secs: 10,
        realtime_processing_timeout_secs: 30,
        sip_max_participants: 3,
        realtime_endpoint_overrides: Default::default(),
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
        aliases: Default::default(),
    }
}

/// Boot the full gateway. Returns (ws_url, http_base) — the HTTP base serves `/metrics`.
async fn boot_gateway() -> (String, String, Arc<AppState>) {
    let config = test_server_config();
    let app_state = AppState::new(config).await;

    let ws_routes = routes::ws::create_ws_router().layer(axum::middleware::from_fn_with_state(
        app_state.clone(),
        auth_middleware,
    ));
    // NOTE: `create_api_router()` does NOT register `/metrics` (the binary's main() adds it
    // separately), so we add the public metrics route here exactly as `tests/metrics_endpoint.rs`
    // does — this is how the live gateway exposes `waav_smart_turn_inference_ms`.
    let app = axum::Router::new()
        .route(
            "/metrics",
            axum::routing::get(waav_gateway::handlers::api::metrics_handler),
        )
        .merge(routes::api::create_api_router())
        .merge(ws_routes)
        .with_state(app_state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    (
        format!("ws://127.0.0.1:{}/ws", addr.port()),
        format!("http://127.0.0.1:{}", addr.port()),
        app_state,
    )
}

/// Synthesize a NOISY speech-like 16 kHz/16-bit mono PCM utterance of `total_ms` ms:
/// a 220 Hz vowel-ish tone (fundamental + 2 harmonics → speech-like spectrum) amplitude-
/// modulated into syllables, mixed with substantial broadband noise. Two properties matter:
///   - VAD/smart-turn: the speech content is well above the silero-VAD energy floor, so the
///     models detect speech and the mel extractor sees real spectral structure.
///   - noise-filter: the signal is deliberately kept at moderate level with real broadband
///     noise so its RMS stays BELOW the DeepFilterNet high-SNR passthrough gate
///     (`rms > 0.15`), and the utterance is ≥ 1 s — together these route it through the FULL
///     DeepFilterNet `df.process` path (not the light-processing / passthrough branches), so
///     the noise model genuinely transforms the audio.
fn synth_noisy_speech(total_ms: usize) -> Vec<u8> {
    let sr = 16_000.0_f64;
    let n = (sr * total_ms as f64 / 1000.0) as usize;
    let f0 = 220.0_f64;
    let two_pi = std::f64::consts::TAU;
    // Cheap deterministic LCG for reproducible broadband noise (no rand dep).
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next_noise = || {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((rng >> 33) as f64 / (1u64 << 31) as f64) - 1.0 // ~[-1,1] white noise
    };
    let mut pcm = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / sr;
        // ~4 Hz syllabic amplitude envelope (speech-like on/off), never fully silent.
        let env = 0.55 + 0.45 * (two_pi * 4.0 * t).sin().abs();
        let voiced = 0.16 * (two_pi * f0 * t).sin()
            + 0.08 * (two_pi * (f0 * 2.0) * t).sin()
            + 0.04 * (two_pi * (f0 * 3.0) * t).sin();
        // Broadband noise at a level that lowers SNR enough to force full DeepFilterNet
        // processing while keeping overall RMS under the 0.15 high-SNR passthrough gate.
        let noise = 0.06 * next_noise();
        let s = env * voiced + noise;
        let v = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    pcm
}

fn realtime_dag_config() -> Value {
    // DAG: audio_input → stt(mock-stt) → realtime_provider(mock-realtime) → audio_output.
    let dag_definition = json!({
        "id": "fe-rt-pipeline",
        "name": "Frontend→Realtime Pipeline",
        "version": "1.0.0",
        "nodes": [
            { "id": "audio_input", "type": "audio_input" },
            { "id": "stt", "type": "stt_provider", "provider": "mock-stt" },
            { "id": "rt", "type": "realtime_provider", "provider": "mock-realtime",
              "config": { "voice": "alloy" } },
            { "id": "audio_output", "type": "audio_output", "destination": "web_socket" }
        ],
        "edges": [
            { "from": "audio_input", "to": "stt" },
            { "from": "stt", "to": "rt" },
            { "from": "rt", "to": "audio_output" }
        ],
        "entry_node": "audio_input",
        "exit_nodes": ["audio_output"]
    });

    json!({
        "type": "config",
        "audio": true,
        "stt_config": {
            "provider": "mock-stt",
            "api_key": "test_key",
            "language": "en-US",
            "sample_rate": 16000,
            "channels": 1,
            "punctuation": true,
            "model": "mock-stt-model",
            // ── FRONT-END ACTIVATION: turns on the gateway's provider-agnostic ML
            //    turn detection → builds the SmartTurnProcessor (silero-VAD + smart-turn)
            //    that runs on every real audio frame in `receive_audio`. ──
            "turn_detection": { "enabled": true, "threshold": 0.5 }
        },
        "tts_config": {
            "provider": "mock-tts",
            "api_key": "test_key",
            "voice_id": "mock-voice",
            "audio_format": "pcm",
            "sample_rate": 24000,
            "model": "mock-tts-model"
        },
        "dag_config": { "definition": dag_definition }
    })
}

/// Scrape `/metrics` over the live gateway and return the integer `_count` of the
/// `waav_smart_turn_inference_ms` histogram (0 if the series is absent).
async fn smart_turn_inference_count(http_base: &str) -> u64 {
    let body = reqwest::get(format!("{http_base}/metrics"))
        .await
        .expect("scrape /metrics")
        .text()
        .await
        .expect("read /metrics body");
    parse_histogram_count(&body, "waav_smart_turn_inference_ms")
}

/// Parse `<metric>_count <value>` from a Prometheus text exposition (no label set —
/// this histogram has no labels). Returns 0 when the series is not present yet.
fn parse_histogram_count(exposition: &str, metric: &str) -> u64 {
    let needle = format!("{metric}_count");
    for line in exposition.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        // Match `<metric>_count <val>` or `<metric>_count{...} <val>`.
        if let Some(rest) = line.strip_prefix(&needle) {
            let rest = rest.trim_start();
            // Skip an optional `{labels}` block.
            let val = if let Some(after_braces) = rest.strip_prefix('{') {
                after_braces.split('}').nth(1).unwrap_or("").trim()
            } else {
                rest
            };
            if let Some(tok) = val.split_whitespace().next()
                && let Ok(n) = tok.parse::<f64>()
            {
                return n as u64;
            }
        }
    }
    0
}

/// FE-RT END-TO-END: real audio → front-end (noise + silero-VAD + smart-turn + turn-detect)
/// → finalized turn → realtime DAG node → assistant audio back to the client.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_audio_drives_frontend_into_realtime_dag_node() {
    RT_CONNECTS.store(0, Ordering::SeqCst);
    register_mocks();

    let (ws_url, http_base, app_state) = boot_gateway().await;

    // ── FRONT-END PROOF #1 (turn-detect): the text end-of-turn model loaded into
    //    CoreState. If this is None the build degraded to a timer fallback — assert it
    //    is live so we are honestly claiming text turn-detection coverage. ──
    let turn_detector_present = app_state.core_state.get_turn_detector().is_some();
    assert!(
        turn_detector_present,
        "turn-detect feature is built but CoreState has no TurnDetector — the \
         livekit/turn-detector model failed to load from the cache; text turn detection \
         would silently fall back to a timer. (cache: ~/.cache/waav-gateway/turn_detect/)"
    );

    // ── FRONT-END PROOF #2 (noise-filter): run the SAME production DeepFilterNet codepath
    //    the LiveKit ingress uses (`reduce_noise_async`) on a NOISY ~1.6 s speech utterance,
    //    and prove the model actually transformed it (a measurable fraction of samples
    //    changed, not a byte-identical pass-through). The DENOISED buffer is what we then
    //    feed to `/ws`, so the audio driving the rest of the front-end (VAD/smart-turn) has
    //    genuinely been through the real noise model — exactly as LiveKit ingress would. ──
    //    ~1.6 s also comfortably exceeds smart-turn's min_frames (≈50 mel frames ≈ 500 ms)
    //    and gives silero-VAD many 512-sample chunks.
    const TURN_MS: usize = 1600;
    const FRAME_MS: usize = 100;
    let raw_utterance = synth_noisy_speech(TURN_MS);
    let denoised_utterance = waav_gateway::utils::noise_filter::reduce_noise_async(
        Bytes::from(raw_utterance.clone()),
        16_000,
    )
    .await
    .expect("noise reduction must succeed");
    assert_eq!(
        denoised_utterance.len(),
        raw_utterance.len(),
        "DeepFilterNet must return the same number of PCM bytes it was given"
    );
    // Count how many 16-bit samples the model changed — full DeepFilterNet processing
    // rewrites essentially the whole buffer; a stub / pass-through changes nothing.
    let changed_samples = raw_utterance
        .chunks_exact(2)
        .zip(denoised_utterance.chunks_exact(2))
        .filter(|(a, b)| a != b)
        .count();
    let total_samples = raw_utterance.len() / 2;
    let changed_frac = changed_samples as f64 / total_samples as f64;
    assert!(
        changed_frac > 0.10,
        "noise-filter (DeepFilterNet) altered only {changed_samples}/{total_samples} samples \
         ({changed_frac:.3}) — the noise model did not actually process the audio (a \
         pass-through stub would change ~0). Expected the full DeepFilterNet path to rewrite \
         most of a noisy 1.6 s utterance."
    );
    let any_frame_changed = changed_samples > 0;

    // Chunk the DENOISED utterance into 100 ms frames for the WS feed.
    let bytes_per_frame = (16_000 * FRAME_MS / 1000) * 2; // samples * 2 bytes
    let denoised_frames: Vec<Vec<u8>> = denoised_utterance
        .chunks(bytes_per_frame)
        .map(|c| c.to_vec())
        .collect();

    // Baseline smart-turn inference count BEFORE we feed audio (should be 0 for a fresh
    // process; we assert the DELTA, so any leakage from other tests is tolerated).
    let baseline_smart_turn = smart_turn_inference_count(&http_base).await;

    // ── Open the session ──
    let (ws_stream, _) = connect_async(ws_url).await.expect("connect");
    let (mut write, mut read) = ws_stream.split();
    write
        .send(Message::Text(realtime_dag_config().to_string().into()))
        .await
        .unwrap();

    // Wait for `ready` (DAG boot complete → RealtimeSessionMap inserted, StreamDriver up,
    // SmartTurnProcessor initialized, observer registry attached).
    let mut ready = false;
    for _ in 0..50 {
        match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let parsed: Value = serde_json::from_str(&text).unwrap();
                match parsed["type"].as_str() {
                    Some("ready") => {
                        ready = true;
                        break;
                    }
                    Some("error") => panic!("gateway error during DAG boot: {}", parsed["message"]),
                    _ => {}
                }
            }
            Ok(Some(Ok(_))) => {}
            _ => break,
        }
    }
    assert!(ready, "gateway never sent `ready` for the frontend→realtime DAG session");

    // ── Drive TWO turns of REAL (denoised) speech-like audio. Each turn streams the 1.6 s
    //    of frames through the front-end; the per-frame mock STT finalizes, the StreamDriver
    //    runs the DAG, and the realtime node's audio marker must ride back to the client. ──
    const TURNS: usize = 2;
    let mut audio_marker_egresses = 0usize;
    for t in 0..TURNS {
        for frame in &denoised_frames {
            write
                .send(Message::Binary(frame.clone().into()))
                .await
                .unwrap();
            // Tiny gap so frames pace like a real stream and the smart-turn write-lock
            // isn't perpetually contended (contention would skip inference on a frame).
            tokio::time::sleep(Duration::from_millis(3)).await;
        }

        // Wait until THIS turn's audio marker rides back over the WS.
        let mut got = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
                Ok(Some(Ok(Message::Binary(bytes)))) => {
                    if bytes.windows(RT_AUDIO_MARKER.len()).any(|w| w == RT_AUDIO_MARKER) {
                        got = true;
                        audio_marker_egresses += 1;
                        break;
                    }
                }
                Ok(Some(Ok(Message::Text(_)))) => {} // stt_result / transcript egress etc.
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => break,
            }
        }
        assert!(
            got,
            "turn {t}: the realtime audio marker did not ride back through the DAG \
             audio_output sink — the front-end→realtime chain did not complete"
        );
    }

    // ── FRONT-END PROOF #3 (silero-VAD + smart-turn): the smart-turn inference histogram
    //    advanced. `record_smart_turn` is emitted by FrameProfiler::on_smart_turn ONLY when
    //    `process_audio` actually ran the models (latency_us > 0). The 1-second /metrics
    //    cache means we may need to wait out a stale window, so poll. ──
    let mut smart_turn_after = baseline_smart_turn;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline {
        smart_turn_after = smart_turn_inference_count(&http_base).await;
        if smart_turn_after > baseline_smart_turn {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let smart_turn_delta = smart_turn_after.saturating_sub(baseline_smart_turn);

    // ── EVIDENCE ──
    let connects = RT_CONNECTS.load(Ordering::SeqCst);
    eprintln!(
        "[FE-RT EVIDENCE] turns={TURNS} audio_per_turn={TURN_MS}ms frames_per_turn={} \
         audio_marker_egresses={audio_marker_egresses} RT_CONNECTS={connects} \
         turn_detector_present={turn_detector_present} \
         noise_filter_changed_samples={changed_samples}/{total_samples} ({changed_frac:.3}) \
         any_frame_changed={any_frame_changed} \
         smart_turn_inference_count: {baseline_smart_turn} -> {smart_turn_after} (delta={smart_turn_delta})",
        denoised_frames.len(),
    );

    // (a) End-to-end: assistant audio rode back once per turn.
    assert_eq!(
        audio_marker_egresses, TURNS,
        "expected exactly one realtime assistant-audio egress per turn through the cascade sink"
    );
    // (b) The finalized turns really reached the realtime node (persistent session connected
    //     once through production DAG-init — request-scoped legacy would also be >0).
    assert!(
        connects >= 1,
        "the realtime provider never connected — the finalized turn did not reach the \
         realtime DAG node"
    );
    // (c) silero-VAD + smart-turn genuinely executed on the real audio.
    assert!(
        smart_turn_delta >= 1,
        "waav_smart_turn_inference_ms_count did not advance ({baseline_smart_turn} -> \
         {smart_turn_after}) — silero-VAD + smart-turn inference did NOT run on the audio. \
         The VAD/smart-turn front-end was not exercised (missing model, or turn_detection \
         not wired). This is the core seam under test."
    );

    let _ = write.close().await;
}
