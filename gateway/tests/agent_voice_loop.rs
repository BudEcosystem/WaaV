//! Spec 025 — a voice agent's turn, end to end inside the gateway: a real `VoiceManager` (mock
//! vendors registered in the provider registry), the real [`AgentBrain`] HTTP+SSE client, and an
//! in-process mock of budgateway's `/v1/responses`. What is asserted is what crosses each seam: the
//! request budgateway receives (the caller's credential, `prompt`, `conversation`, `bud_channel`,
//! `bud_truncate`), what the TTS vendor is asked to say, and the cancel a barge-in sends.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use waav_gateway::auth::SessionCredential;
use waav_gateway::core::agent::{
    AgentBrain, AgentEngine, AgentSessionConfig, AgentSignal, SpeechOut, TurnBackend, TurnStatus,
};
use waav_gateway::core::stt::{BaseSTT, STTConfig};
use waav_gateway::core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSResult};
use waav_gateway::core::voice_manager::{VoiceManager, VoiceManagerConfig};
use waav_gateway::global_registry;
use waav_gateway::plugin::metadata::ProviderMetadata;

// ------------------------------------------------------------------------------------------------
// Mock vendors
// ------------------------------------------------------------------------------------------------

static SPOKEN: once_cell::sync::Lazy<Mutex<Vec<String>>> =
    once_cell::sync::Lazy::new(Mutex::default);
static CLEARS: AtomicUsize = AtomicUsize::new(0);

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
        Ok(())
    }
    fn is_ready(&self) -> bool {
        self.ready
    }
    async fn speak(&mut self, text: &str, _flush: bool) -> TTSResult<()> {
        SPOKEN.lock().push(text.to_string());
        if let Some(cb) = &self.callback {
            // ~50 ms of audio per chunk, delivered at once (faster than real time, as vendors do).
            cb.on_audio(AudioData {
                data: vec![0u8; 2400],
                sample_rate: 24000,
                format: "pcm16".into(),
                duration_ms: Some(50),
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
        CLEARS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.callback = Some(callback);
        Ok(())
    }
    fn get_provider_info(&self) -> Value {
        json!({"provider": "mock-tts-agent"})
    }
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
    async fn send_audio(&mut self, _audio: Bytes) -> Result<(), waav_gateway::core::stt::STTError> {
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
        "mock-stt-agent"
    }
}

async fn voice_manager() -> Arc<VoiceManager> {
    let registry = global_registry();
    registry.register_tts(
        "mock-tts-agent",
        Arc::new(|c: TTSConfig| MockTts::new(c).map(|t| Box::new(t) as Box<dyn BaseTTS>)),
        ProviderMetadata::tts("mock-tts-agent", "Mock TTS (agent)"),
    );
    registry.register_stt(
        "mock-stt-agent",
        Arc::new(|c: STTConfig| {
            NoopStt::new(c).map(|s| Box::new(s) as Box<dyn waav_gateway::core::stt::BaseSTT>)
        }),
        ProviderMetadata::stt("mock-stt-agent", "No-op STT (agent)"),
    );
    let vm = Arc::new(
        VoiceManager::new(
            VoiceManagerConfig::new(
                STTConfig {
                    provider: "mock-stt-agent".into(),
                    api_key: "t".into(),
                    ..Default::default()
                },
                TTSConfig {
                    provider: "mock-tts-agent".into(),
                    api_key: "t".into(),
                    ..Default::default()
                },
            ),
            None,
        )
        .expect("voice manager"),
    );
    vm.on_tts_audio(|_a| Box::pin(async {})).await.unwrap();
    vm.start().await.expect("start");
    vm
}

// ------------------------------------------------------------------------------------------------
// Mock budgateway
// ------------------------------------------------------------------------------------------------

#[derive(Clone)]
enum Reply {
    /// SSE frames, with an optional pause (ms) before each.
    Sse(Vec<(u64, Value)>),
    Status(u16, Value),
}

#[derive(Clone, Default)]
struct Gateway {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
    auth: Arc<Mutex<Vec<String>>>,
    cancels: Arc<Mutex<Vec<(String, String)>>>,
}

async fn responses(
    State(g): State<Gateway>,
    headers: HeaderMap,
    body: axum::Json<Value>,
) -> Response {
    g.bodies.lock().push(body.0);
    g.auth.lock().push(
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string(),
    );
    match g.replies.lock().pop_front() {
        Some(Reply::Status(code, body)) => {
            (StatusCode::from_u16(code).unwrap(), axum::Json(body)).into_response()
        }
        Some(Reply::Sse(frames)) => {
            let stream = futures::stream::unfold(frames.into_iter(), |mut it| async move {
                let (pause, frame) = it.next()?;
                if pause > 0 {
                    tokio::time::sleep(Duration::from_millis(pause)).await;
                }
                let kind = frame["type"].as_str().unwrap_or("x").to_string();
                let chunk = format!("event: {kind}\ndata: {frame}\n\n");
                Some((Ok::<_, Infallible>(Bytes::from(chunk)), it))
            });
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(Body::from_stream(stream))
                .unwrap()
        }
        None => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn cancel(
    State(g): State<Gateway>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> StatusCode {
    let model = headers
        .get("x-model-name")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    g.cancels.lock().push((id, model));
    StatusCode::OK
}

async fn gateway() -> (String, Gateway) {
    let g = Gateway::default();
    let app = Router::new()
        .route("/v1/responses", post(responses))
        .route("/v1/responses/{id}/cancel", post(cancel))
        .with_state(g.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/v1"), g)
}

fn created(id: &str) -> Value {
    json!({"type": "response.created", "response": {"id": id, "status": "in_progress", "conversation": {"id": "conv_voice_s"}}})
}
fn delta(t: &str) -> Value {
    json!({"type": "response.output_text.delta", "item_id": "msg_1", "delta": t})
}
fn completed() -> Value {
    json!({"type": "response.completed", "response": {"status": "completed", "output": [], "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}}})
}

fn entry() -> Arc<bud_auth::VoiceAgentEntry> {
    Arc::new(
        bud_auth::parse_voice_agent_blob(
            &json!({"prompt_id": "c0de", "version": 2, "stt": {"endpoint_id": "s"}, "tts": {"endpoint_id": "t"},
                    "fillers": {"tool_call_after_ms": 0, "slow_response_after_ms": 0},
                    "degradation_message": "Sorry, try again."})
            .to_string(),
        )
        .unwrap(),
    )
}

async fn engine(
    base: &str,
    vm: &Arc<VoiceManager>,
) -> (Arc<AgentEngine>, mpsc::UnboundedReceiver<AgentSignal>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let engine = AgentEngine::new(
        AgentSessionConfig {
            session_id: "s".into(),
            prompt_name: "support".into(),
            version: 2,
            entry: entry(),
            conversation_id: "conv_voice_s".into(),
            variables: None,
            text_only: false,
        },
        Arc::new(AgentBrain::new(base)) as Arc<dyn TurnBackend>,
        Arc::clone(vm) as Arc<dyn SpeechOut>,
        SessionCredential::new("bud-caller-key"),
        tx,
    );
    (engine, rx)
}

async fn done(rx: &mut mpsc::UnboundedReceiver<AgentSignal>, turn: u64) -> Vec<AgentSignal> {
    let mut out = Vec::new();
    loop {
        let s = tokio::time::timeout(Duration::from_secs(20), rx.recv())
            .await
            .expect("signal")
            .expect("open");
        let end = matches!(&s, AgentSignal::ResponseDone { turn: t, .. } if *t == turn);
        out.push(s);
        if end {
            return out;
        }
    }
}

fn status(signals: &[AgentSignal]) -> TurnStatus {
    signals
        .iter()
        .find_map(|s| match s {
            AgentSignal::ResponseDone { status, .. } => Some(*status),
            _ => None,
        })
        .unwrap()
}

// One test owns the global mock TTS log: the cases run in sequence inside it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_voice_agent_turn_end_to_end() {
    let vm = voice_manager().await;
    let (base, gw) = gateway().await;
    let (engine, mut rx) = engine(&base, &vm).await;

    // 1. One turn: the caller's credential and the agent turn reach budgateway; the reply is spoken.
    gw.replies.lock().push_back(Reply::Sse(vec![
        (0, created("resp_1")),
        (0, delta("Your order shipped on Monday")),
        (0, delta(", and it arrives Friday. ")),
        (0, delta("Anything else?")),
        (0, completed()),
    ]));
    engine.start_turn("where is my order".into()).await;
    let s = done(&mut rx, 0).await;
    assert_eq!(status(&s), TurnStatus::Completed);
    let body = gw.bodies.lock()[0].clone();
    assert_eq!(body["prompt"], json!({"id": "support", "version": "2"}));
    assert_eq!(body["input"], "where is my order");
    assert_eq!(body["conversation"], "conv_voice_s");
    assert_eq!(body["stream"], true);
    assert_eq!(body["bud_channel"], "voice");
    assert_eq!(
        gw.auth.lock()[0],
        "Bearer bud-caller-key",
        "the caller's own credential (S-1)"
    );
    assert_eq!(
        SPOKEN.lock().clone(),
        // The comma and the sentence end arrived in one delta: the whole sentence is one chunk.
        vec![
            "Your order shipped on Monday, and it arrives Friday.",
            "Anything else?"
        ]
    );

    // 2. A barge-in mid-reply: speech stops, the run is cancelled at budgateway with the agent name,
    //    and the next turn carries the truncate for that run.
    SPOKEN.lock().clear();
    gw.replies.lock().push_back(Reply::Sse(vec![
        (0, created("resp_2")),
        (0, delta("Let me explain this in some detail, ")),
        (3_000, delta("which takes a while.")),
        (0, completed()),
    ]));
    engine.start_turn("explain".into()).await;
    for _ in 0..100 {
        if !SPOKEN.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let clears_before = CLEARS.load(Ordering::SeqCst);
    engine.barge_in().await;
    let s = done(&mut rx, 1).await;
    assert_eq!(status(&s), TurnStatus::Cancelled);
    assert!(s.iter().any(|x| matches!(x, AgentSignal::Truncated { response_id, .. } if response_id.as_deref() == Some("resp_2"))));
    assert!(
        CLEARS.load(Ordering::SeqCst) > clears_before,
        "the TTS vendor was told to stop"
    );
    for _ in 0..50 {
        if !gw.cancels.lock().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        gw.cancels.lock()[0],
        ("resp_2".to_string(), "support".to_string())
    );

    gw.replies.lock().push_back(Reply::Sse(vec![
        (0, created("resp_3")),
        (0, delta("Okay.")),
        (0, completed()),
    ]));
    engine.start_turn("never mind".into()).await;
    let _ = done(&mut rx, 2).await;
    let body = gw.bodies.lock()[2].clone();
    assert_eq!(body["bud_truncate"]["response_id"], "resp_2");
    assert!(body["bud_truncate"]["output_text"].is_string());

    // 3. budgateway refuses with 429: the caller hears the degradation message, the turn fails.
    SPOKEN.lock().clear();
    gw.replies.lock().push_back(Reply::Status(
        429,
        json!({"error": {"code": "rate_limit_exceeded", "message": "slow down"}}),
    ));
    engine.start_turn("again".into()).await;
    let s = done(&mut rx, 3).await;
    assert_eq!(status(&s), TurnStatus::Failed);
    assert!(s.iter().any(|x| matches!(
        x,
        AgentSignal::Error {
            code: "rate_limit_exceeded",
            ..
        }
    )));
    assert_eq!(SPOKEN.lock().clone(), vec!["Sorry, try again."]);

    // 4. An expired credential: told, not spoken; the session continues.
    SPOKEN.lock().clear();
    gw.replies.lock().push_back(Reply::Status(
        401,
        json!({"error": {"code": "invalid_api_key", "message": "expired"}}),
    ));
    engine.start_turn("hello".into()).await;
    let s = done(&mut rx, 4).await;
    assert!(s.iter().any(|x| matches!(
        x,
        AgentSignal::Error {
            code: "auth_expired",
            ..
        }
    )));
    assert!(SPOKEN.lock().is_empty());

    engine.shutdown().await;
    vm.stop().await.ok();
}
