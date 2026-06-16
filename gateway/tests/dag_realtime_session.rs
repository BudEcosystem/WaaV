//! B-G2 — PERSISTENT realtime (S2S) DAG path, END-TO-END through the FULL gateway.
//!
//! This proves the change under review: the persistent realtime path
//! (`RealtimeProviderNode::execute_session_scoped` + `SessionRealtime` +
//! `RealtimeSessionMap`) is WIRED INTO PRODUCTION and tears down cleanly.
//!
//! It boots the real gateway over a WebSocket with a DAG
//! `audio_input → stt(mock-stt) → realtime_provider(mock-realtime) → audio_output`,
//! drives MULTIPLE finalized STT turns (each audio frame the mock STT turns into a
//! `speech_final` transcript → one DAG turn at the realtime node), and asserts the
//! THREE B-G2 invariants:
//!
//!   (a) SESSION REUSED — the realtime provider `connect()`s EXACTLY ONCE across all
//!       turns (the whole point of B-G2; the legacy request-scoped path connected
//!       once PER turn). Measured by an atomic CONNECT counter on the mock provider,
//!       which is reachable only through `production` DAG-init inserting the
//!       `RealtimeSessionMap` (`initialize_dag_routing`).
//!   (b) AUDIO RODE BACK — the assistant audio the persistent session emits rides the
//!       cascade `DagOutput::Audio` sink → the DAG `audio_output` node → the client
//!       WS, byte-for-byte (a distinctive marker), once per turn.
//!   (c) TEARDOWN DISCONNECTED — on client WS disconnect, `handle_disconnect` calls
//!       `disconnect_realtime_sessions`, which `disconnect()`s the persistent session
//!       and drains the map. Measured by an atomic DISCONNECT counter on the mock.
//!
//! CREDENTIAL-FREE: the realtime upstream is an IN-PROCESS registry-registered mock
//! provider (`mock-realtime`), so NO vendor key and NO network are involved.
//!
//! WHY NOT the OPENAI_REALTIME_URL + python-mock route: the DAG realtime node's
//! `build_node_realtime_config()` FORCES `realtime_endpoint_override = None` (an SSRF
//! guard, provider.rs ~line 1056) and the DAG path NEVER injects the server-config
//! `realtime_endpoint_overrides` map (only the HTTP `/realtime` handler does). So
//! `OPENAI_REALTIME_URL` cannot redirect the openai DAG node to a mock upstream — the
//! openai node would always dial `wss://api.openai.com`. A registry mock provider is
//! the correct credential-free harness for the DAG realtime path.

#![cfg(feature = "dag-routing")]

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

// ──────────────────────────────────────────────────────────────────────────────
// Process-global counters on the mock realtime provider. Statics (not instance
// fields) so the teardown assertion can read them after the provider is dropped,
// and so the test sees connects/disconnects no matter how many DAGContext clones
// route through the session. This test binary runs these two tests; to avoid
// cross-test races on the shared statics, the second (negative-control) test is
// gated behind the same provider but uses its own provider name + counters.
// ──────────────────────────────────────────────────────────────────────────────
static RT_CONNECTS: AtomicUsize = AtomicUsize::new(0);
static RT_DISCONNECTS: AtomicUsize = AtomicUsize::new(0);

/// Distinctive assistant-audio marker so the WS-egress assertion is unambiguous
/// (the DAG `audio_output` node delivers exactly these bytes per turn).
const RT_AUDIO_MARKER: &[u8] = b"BG2_REALTIME_AUDIO_MARKER";

// ──────────────────────────────────────────────────────────────────────────────
// Mock STT: fire ONE finalized (final + speech_final) transcript per audio frame,
// so N audio frames drive N finalized turns the StreamDriver runs through the DAG.
// (The shared MockStt in dag_dataplane fires only ONCE — here we need ≥2 turns to
// prove the session is REUSED, so this one fires per-frame.)
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
        // Each frame → a distinct finalized turn. A non-empty + speech_final
        // result drives the turn controller through start→stop in one feed
        // (AnySpeechStart opens, LegacySpeechFinalStop closes), emitting one
        // `Stopped` event → one DAG turn at the realtime node.
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
// Mock realtime (S2S) provider: counts connect/disconnect process-globally and, on
// each `create_response`, emits — IN ORDER, all awaited before returning — one audio
// chunk (the marker) through the audio callback, a finalized ASSISTANT transcript,
// then response-done. Firing done LAST and synchronously means the persistent node's
// `response_done` wait completes immediately (no lost-wakeup, no 30s timeout).
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
        RT_DISCONNECTS.fetch_add(1, Ordering::SeqCst);
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
    // Manual mode: the gateway drives commit + create_response (the persistent
    // node commits the buffer then calls create_response). Returning false makes
    // the node call `commit_audio_buffer()` explicitly before `create_response`.
    fn emits_user_turn_frames(&self) -> bool {
        false
    }
}

fn register_mock_realtime() {
    let registry = global_registry();
    registry.register_realtime(
        "mock-realtime",
        Arc::new(|c: RealtimeConfig| {
            CountingRealtime::new(c).map(|r| Box::new(r) as Box<dyn BaseRealtime>)
        }),
        ProviderMetadata::realtime("mock-realtime", "Mock Realtime"),
    );
}

fn register_mock_stt() {
    let registry = global_registry();
    registry.register_stt(
        "mock-stt",
        Arc::new(|config: STTConfig| {
            MockStt::new(config).map(|s| Box::new(s) as Box<dyn BaseSTT>)
        }),
        ProviderMetadata::stt("mock-stt", "Mock STT"),
    );
}

// ──────────────────────────────────────────────────────────────────────────────
// Mock TTS: the VoiceManager requires a TTS provider to BOOT (from `tts_config`)
// even though the realtime DAG never routes through a TTS node. It is a no-op
// (never invoked by this DAG); the assistant audio comes from the realtime node.
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

fn register_mock_tts() {
    let registry = global_registry();
    registry.register_tts(
        "mock-tts",
        Arc::new(|config: TTSConfig| {
            MockTts::new(config).map(|t| Box::new(t) as Box<dyn BaseTTS>)
        }),
        ProviderMetadata::tts("mock-tts", "Mock TTS"),
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
    }
}

/// Boot the full gateway, return (ws_url, addr). The server task is spawned and
/// detached (process teardown reclaims it).
async fn boot_gateway() -> String {
    let config = test_server_config();
    let app_state = AppState::new(config).await;

    let ws_routes = routes::ws::create_ws_router().layer(axum::middleware::from_fn_with_state(
        app_state.clone(),
        auth_middleware,
    ));
    let app = routes::api::create_api_router()
        .merge(ws_routes)
        .with_state(app_state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    format!("ws://127.0.0.1:{}/ws", addr.port())
}

fn realtime_dag_config() -> Value {
    // DAG: audio_input → stt(mock-stt) → realtime_provider(mock-realtime) → audio_output
    // The StreamDriver injects each finalized STT turn at the post-STT node = the
    // realtime node, so the persistent session runs once per turn.
    let dag_definition = json!({
        "id": "rt-pipeline",
        "name": "Realtime Pipeline",
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
            "model": "mock-stt-model"
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

/// B-G2 END-TO-END: persistent realtime DAG path through the FULL gateway.
/// Asserts (a) ONE connect across ≥2 turns, (b) audio rode back per turn,
/// (c) client disconnect → exactly ONE provider disconnect (teardown drained).
#[tokio::test]
async fn persistent_realtime_dag_reuses_session_streams_audio_and_disconnects_on_teardown() {
    RT_CONNECTS.store(0, Ordering::SeqCst);
    RT_DISCONNECTS.store(0, Ordering::SeqCst);
    register_mock_stt();
    register_mock_tts();
    register_mock_realtime();

    let url = boot_gateway().await;
    let (ws_stream, _) = connect_async(url).await.expect("connect");
    let (mut write, mut read) = ws_stream.split();

    write
        .send(Message::Text(realtime_dag_config().to_string().into()))
        .await
        .unwrap();

    // Wait for `ready` (DAG boot complete → RealtimeSessionMap inserted, StreamDriver up).
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
    assert!(ready, "gateway never sent `ready` for the realtime DAG session");

    // Drive THREE turns: one audio frame each → one finalized STT turn each →
    // one persistent-session response each. Collect the audio-marker egress per turn.
    const TURNS: usize = 3;
    let mut audio_marker_egresses = 0usize;
    for t in 0..TURNS {
        let audio_frame = vec![0u8; 3200]; // 100ms 16k/16-bit mono silence
        write.send(Message::Binary(audio_frame.into())).await.unwrap();

        // Wait until THIS turn's audio marker rides back over the WS.
        let mut got = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
                Ok(Some(Ok(Message::Binary(bytes)))) => {
                    if bytes.windows(RT_AUDIO_MARKER.len()).any(|w| w == RT_AUDIO_MARKER) {
                        got = true;
                        audio_marker_egresses += 1;
                        break;
                    }
                }
                Ok(Some(Ok(Message::Text(_)))) => {} // transcript egress etc.
                Ok(Some(Ok(_))) => {}
                Ok(Some(Err(_))) | Ok(None) => break,
                Err(_) => break,
            }
        }
        assert!(got, "turn {t}: expected the realtime audio marker to ride back through the DAG audio_output sink");
    }

    // (b) audio rode back, once per turn.
    eprintln!(
        "[B-G2 EVIDENCE] after {TURNS} turns: audio_marker_egresses={audio_marker_egresses} \
         RT_CONNECTS={} RT_DISCONNECTS={}",
        RT_CONNECTS.load(Ordering::SeqCst),
        RT_DISCONNECTS.load(Ordering::SeqCst),
    );
    assert_eq!(
        audio_marker_egresses, TURNS,
        "expected exactly one assistant-audio egress per turn through the cascade sink"
    );

    // (a) SESSION REUSED: the provider connected EXACTLY ONCE across all turns.
    // Request-scoped (legacy) would be TURNS connects; B-G2 persistent is 1.
    let connects = RT_CONNECTS.load(Ordering::SeqCst);
    assert_eq!(
        connects, 1,
        "persistent session must connect ONCE across {TURNS} turns (got {connects}); \
         >1 means the session is being rebuilt per turn (B-G2 not in effect)"
    );

    // Not yet disconnected — the client is still connected.
    assert_eq!(
        RT_DISCONNECTS.load(Ordering::SeqCst),
        0,
        "the persistent session must stay connected while the client WS is open"
    );

    // (c) TEARDOWN: close the client WS → handle_disconnect → disconnect_realtime_sessions.
    let _ = write.close().await;
    drop(write);
    drop(read);

    // Poll for the disconnect (teardown is async on the server side).
    let mut disconnected = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if RT_DISCONNECTS.load(Ordering::SeqCst) >= 1 {
            disconnected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        disconnected,
        "client disconnect must trigger disconnect_realtime_sessions → provider.disconnect() \
         (B-G2 teardown owner). Saw {} disconnect(s)",
        RT_DISCONNECTS.load(Ordering::SeqCst)
    );

    eprintln!(
        "[B-G2 EVIDENCE] after client disconnect: RT_CONNECTS={} RT_DISCONNECTS={}",
        RT_CONNECTS.load(Ordering::SeqCst),
        RT_DISCONNECTS.load(Ordering::SeqCst),
    );
    // Exactly one disconnect for the one persistent session (no double-close, the
    // connect count is unchanged — no reconnect storm).
    assert_eq!(
        RT_DISCONNECTS.load(Ordering::SeqCst),
        1,
        "exactly ONE disconnect for the ONE persistent session"
    );
    assert_eq!(
        RT_CONNECTS.load(Ordering::SeqCst),
        1,
        "connect count unchanged after teardown (no reconnect)"
    );
}
