//! W-O1 — DAG data-plane wiring (extreme TDD RED→GREEN).
//!
//! PROBLEM (verified): `DAGExecutor::execute` had zero non-test call sites.
//! `config_handler.rs` compiled+stored `compiled_dag`/`dag_executor`/`dag_context` in
//! `ConnectionState` but nothing read them; `audio_handler.rs` sent audio straight to
//! `voice_manager.receive_audio()`, bypassing the DAG. Output nodes wrote
//! `output_destination` into metadata the executor never read, so they never delivered.
//!
//! This test boots the real gateway over a WebSocket with a `dag_config`
//! (`audio_input → llm → tts → audio_output`), streams audio, and asserts that BOTH a
//! transcript egress (the STT result) AND a TTS audio egress (binary frame produced by the
//! DAG's audio_output node) arrive over the WS. Before the wiring it fails because the
//! executor is never invoked and the audio_output node has no delivery channel.
//!
//! Providers are mocked in-process:
//! - STT: a registry-registered mock that emits a final transcript when it receives audio
//!   (drives the VoiceManager turn → StreamDriver → executor injection).
//! - LLM: a tiny in-process OpenAI-compatible HTTP mock (the DAG `llm` node POSTs to it).
//! - TTS: a registry-registered mock that emits audio bytes on `speak` (the DAG `tts` node).

#![cfg(feature = "dag-routing")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::{Json, Router, routing::post};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

use waav_gateway::{
    ServerConfig,
    config::{DAGTimeoutsConfig, PluginConfig},
    core::stt::{BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback},
    core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSResult},
    global_registry,
    middleware::auth::auth_middleware,
    plugin::metadata::ProviderMetadata,
    routes,
    state::AppState,
};

// ──────────────────────────────────────────────────────────────────────────────
// Mock STT provider: on the first audio frame, fire a final/speech-final transcript.
// ──────────────────────────────────────────────────────────────────────────────
struct MockStt {
    ready: bool,
    callback: Option<STTResultCallback>,
    fired: AtomicBool,
}

#[async_trait::async_trait]
impl BaseSTT for MockStt {
    fn new(_config: STTConfig) -> Result<Self, STTError> {
        Ok(Self {
            ready: false,
            callback: None,
            fired: AtomicBool::new(false),
        })
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
        // Emit a single final, speech-final transcript on the first frame so the
        // VoiceManager produces a finalized turn the StreamDriver can act on.
        if self
            .fired
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            if let Some(cb) = &self.callback {
                let result = STTResult::new("hello agent".to_string(), true, true, 0.99);
                cb(result).await;
            }
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
// Mock TTS provider: emit a distinctive audio frame on `speak`.
// ──────────────────────────────────────────────────────────────────────────────
const TTS_MARKER: &[u8] = b"DAG_TTS_AUDIO_MARKER";

struct MockTts {
    ready: bool,
    callback: Option<Arc<dyn AudioCallback>>,
}

#[async_trait::async_trait]
impl BaseTTS for MockTts {
    fn new(_config: TTSConfig) -> TTSResult<Self> {
        Ok(Self {
            ready: false,
            callback: None,
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

    async fn speak(&mut self, _text: &str, _flush: bool) -> TTSResult<()> {
        if let Some(cb) = &self.callback {
            let audio = AudioData {
                data: TTS_MARKER.to_vec(),
                sample_rate: 24000,
                format: "pcm16".to_string(),
                duration_ms: Some(100),
            };
            cb.on_audio(audio).await;
            // Signal end-of-synthesis so the DAG TTS provider node stops collecting
            // and returns promptly (a real provider's dispatcher fires this).
            cb.on_complete().await;
        }
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.callback = Some(callback);
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        json!({ "provider": "mock-tts" })
    }
}

fn register_mock_providers() {
    let registry = global_registry();
    registry.register_stt(
        "mock-stt",
        Arc::new(|config: STTConfig| {
            MockStt::new(config).map(|s| Box::new(s) as Box<dyn BaseSTT>)
        }),
        ProviderMetadata::stt("mock-stt", "Mock STT"),
    );
    registry.register_tts(
        "mock-tts",
        Arc::new(|config: TTSConfig| {
            MockTts::new(config).map(|t| Box::new(t) as Box<dyn BaseTTS>)
        }),
        ProviderMetadata::tts("mock-tts", "Mock TTS"),
    );
}

/// Start a tiny OpenAI-compatible chat-completions mock. Returns its base URL.
async fn start_llm_mock() -> String {
    async fn chat(Json(_req): Json<Value>) -> Json<Value> {
        Json(json!({
            "id": "chatcmpl-mock",
            "object": "chat.completion",
            "created": 1_700_000_000u64,
            "model": "mock-llm",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "Hi there, how can I help?" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        }))
    }

    let app = Router::new().route("/chat/completions", post(chat));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://127.0.0.1:{}", addr.port())
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
        plugins: PluginConfig::default(),
        dag_timeouts: DAGTimeoutsConfig::default(),
    }
}

#[tokio::test]
async fn dag_session_transcribes_and_speaks() {
    // Allow the in-process LLM mock at 127.0.0.1 to pass DAG SSRF validation.
    // Opt-in, OFF by default; this is a separate test binary so it does not affect
    // the lib's SSRF unit tests (which run in their own process).
    unsafe {
        std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", "1");
    }

    register_mock_providers();
    let llm_base = start_llm_mock().await;

    let config = test_server_config();
    let app_state = AppState::new(config.clone()).await;

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

    let url = format!("ws://127.0.0.1:{}/ws", addr.port());
    let (ws_stream, _) = connect_async(url).await.expect("Failed to connect");
    let (mut write, mut read) = ws_stream.split();

    // DAG: audio_input → llm → tts → audio_output.
    // STT runs in the VoiceManager (mock-stt); the StreamDriver injects the finalized
    // transcript at the post-STT node (the llm node) and runs the DAG once.
    let dag_definition = json!({
        "id": "test-pipeline",
        "name": "Test Pipeline",
        "version": "1.0.0",
        "nodes": [
            { "id": "audio_input", "type": "audio_input" },
            { "id": "stt", "type": "stt_provider", "provider": "mock-stt" },
            {
                "id": "llm",
                "type": "llm_endpoint",
                "base_url": llm_base,
                "model": "mock-llm",
                "api_key": "test",
                "streaming": false
            },
            { "id": "tts", "type": "tts_provider", "provider": "mock-tts" },
            { "id": "audio_output", "type": "audio_output", "destination": "web_socket" }
        ],
        "edges": [
            { "from": "audio_input", "to": "stt" },
            { "from": "stt", "to": "llm" },
            { "from": "llm", "to": "tts" },
            { "from": "tts", "to": "audio_output" }
        ],
        "entry_node": "audio_input",
        "exit_nodes": ["audio_output"]
    });

    let config_message = json!({
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
        "dag_config": {
            "definition": dag_definition
        }
    });

    write
        .send(Message::Text(config_message.to_string().into()))
        .await
        .unwrap();

    // Wait for the ready message (boot complete).
    let mut ready = false;
    for _ in 0..50 {
        if let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_secs(5), read.next()).await
        {
            if let Message::Text(text) = &msg {
                let parsed: Value = serde_json::from_str(text).unwrap();
                match parsed["type"].as_str() {
                    Some("ready") => {
                        ready = true;
                        break;
                    }
                    Some("error") => {
                        panic!("Gateway returned error during DAG boot: {}", parsed["message"]);
                    }
                    _ => {}
                }
            }
        } else {
            break;
        }
    }
    assert!(ready, "Gateway never sent a ready message for the DAG session");

    // Stream a single audio frame; the mock STT will produce a finalized turn.
    let audio_frame = vec![0u8; 3200]; // 100ms of 16kHz/16-bit mono silence
    write
        .send(Message::Binary(audio_frame.into()))
        .await
        .unwrap();

    // Collect egress until we observe BOTH a transcript and the TTS audio marker.
    let mut saw_transcript = false;
    let mut saw_tts_audio = false;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while tokio::time::Instant::now() < deadline && !(saw_transcript && saw_tts_audio) {
        match tokio::time::timeout(Duration::from_secs(5), read.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => {
                let parsed: Value = serde_json::from_str(&text).unwrap();
                if parsed["type"] == "stt_result" {
                    let transcript = parsed["transcript"].as_str().unwrap_or("");
                    if !transcript.is_empty() {
                        saw_transcript = true;
                    }
                }
            }
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if bytes.windows(TTS_MARKER.len()).any(|w| w == TTS_MARKER) {
                    saw_tts_audio = true;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(_))) | Ok(None) => break,
            Err(_) => break, // timeout on this recv
        }
    }

    assert!(
        saw_transcript,
        "Expected a transcript egress over the WS (STT result), got none"
    );
    assert!(
        saw_tts_audio,
        "Expected a TTS audio egress over the WS produced by the DAG audio_output node \
         (the executor must run on a finalized turn and the output node must deliver). \
         This is the W-O1 wiring under test."
    );

    let _ = write.close().await;
}

/// An `audio_output` node, given an output sink on the context, must deliver its audio
/// through the `DagOutput` channel (not the dead `output_destination` metadata path).
#[tokio::test]
async fn dag_output_node_delivers_to_channel() {
    use waav_gateway::dag::context::{DAGContext, DagOutput};
    use waav_gateway::dag::nodes::{AudioOutputNode, DAGData, DAGNode, TTSAudioData};

    let (tx, mut rx) = tokio::sync::mpsc::channel::<DagOutput>(4);
    let mut ctx = DAGContext::new("test-stream").with_output_tx(tx);

    let node = AudioOutputNode::websocket("audio_out");
    let input = DAGData::TTSAudio(TTSAudioData {
        data: Bytes::from_static(b"abc123"),
        sample_rate: 24000,
        format: "pcm16".to_string(),
        duration_ms: Some(50),
        is_final: true,
    });

    let out = node.execute(input, &mut ctx).await.expect("execute ok");
    assert!(matches!(out, DAGData::TTSAudio(_)));

    let delivered = tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("output should be delivered within 1s")
        .expect("channel should yield an output");

    match delivered {
        DagOutput::Audio(audio) => {
            assert_eq!(audio.data.as_ref(), b"abc123");
            assert_eq!(audio.sample_rate, 24000);
        }
        other => panic!("expected DagOutput::Audio, got {:?}", other),
    }
}
