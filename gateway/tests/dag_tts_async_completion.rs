//! A DAG `tts_provider` node against a provider that behaves like a real one: its audio and its
//! completion arrive ASYNCHRONOUSLY, after `speak()` has returned (an HTTP vendor streams the body
//! from a dispatcher task). The in-process mock of `tests/dag_dataplane.rs` fires `on_audio` and
//! `on_complete` inside `speak()`, before the node ever waits, which hid a lost completion signal:
//! every real turn ran into the executor's 30 s node timeout and its audio was discarded.
//!
//! Also covers the client-facing half: a failed turn names its node (`DAGContext::failed_node`).
#![cfg(feature = "dag-routing")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::mpsc;
use waav_gateway::{
    core::tts::{AudioCallback, AudioData, BaseTTS, TTSConfig, TTSError, TTSResult},
    dag::{
        DAGCompiler, DAGContext, DAGDefinition, DAGError, DAGExecutor, DagOutput,
        nodes::{DAGData, DAGNode, TTSProviderNode},
    },
    global_registry,
    plugin::metadata::ProviderMetadata,
};

const CHUNK: usize = 480;
const CHUNKS: usize = 5;

/// Emits `CHUNKS` chunks 20 ms apart from a spawned task, then completes — or, as
/// `mock-tts-silent`, never says anything, or as `mock-tts-refuses`, reports an error.
struct AsyncTts {
    mode: &'static str,
    ready: bool,
    callback: Option<Arc<dyn AudioCallback>>,
}

#[async_trait::async_trait]
impl BaseTTS for AsyncTts {
    fn new(_config: TTSConfig) -> TTSResult<Self> {
        Ok(Self {
            mode: "async",
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
        let Some(cb) = self.callback.clone() else {
            return Ok(());
        };
        let mode = self.mode;
        tokio::spawn(async move {
            match mode {
                "silent" => {}
                "slow" => {
                    // A long answer: 2 s of speech, produced over 2 s.
                    for _ in 0..20 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        let audio = AudioData {
                            data: vec![1u8; CHUNK],
                            sample_rate: 24_000,
                            format: "linear16".to_string(),
                            duration_ms: Some(100),
                        };
                        cb.on_audio(audio).await;
                    }
                    cb.on_complete().await;
                }
                "refuses" => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    cb.on_error(TTSError::ProviderError("voice not found".into()))
                        .await;
                }
                _ => {
                    for _ in 0..CHUNKS {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        let audio = AudioData {
                            data: vec![1u8; CHUNK],
                            sample_rate: 24_000,
                            format: "linear16".to_string(),
                            duration_ms: Some(10),
                        };
                        cb.on_audio(audio).await;
                    }
                    cb.on_complete().await;
                }
            }
        });
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
        json!({ "provider": "mock-tts-async" })
    }
}

fn register() {
    let registry = global_registry();
    for (name, mode) in [
        ("mock-tts-async", "async"),
        ("mock-tts-silent", "silent"),
        ("mock-tts-refuses", "refuses"),
        ("mock-tts-slow", "slow"),
    ] {
        registry.register_tts(
            name,
            Arc::new(move |config: TTSConfig| {
                AsyncTts::new(config).map(|mut t| {
                    t.mode = mode;
                    Box::new(t) as Box<dyn BaseTTS>
                })
            }),
            ProviderMetadata::tts(name, "Async mock TTS"),
        );
    }
}

#[tokio::test]
async fn tts_node_returns_on_asynchronous_completion() {
    register();
    let node = TTSProviderNode::new("tts", "mock-tts-async");
    let mut ctx = DAGContext::new("s");
    let started = Instant::now();
    let out = tokio::time::timeout(
        Duration::from_secs(5),
        node.execute(DAGData::Text("hello there".into()), &mut ctx),
    )
    .await
    .expect("the node must return once the provider completes, not at its own deadline")
    .expect("synthesis succeeds");
    match out {
        DAGData::TTSAudio(audio) => {
            assert_eq!(audio.data.len(), CHUNK * CHUNKS, "every chunk is kept");
            assert!(audio.is_final);
        }
        other => panic!("expected audio, got {}", other.type_name()),
    }
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn tts_node_that_hears_nothing_fails_instead_of_returning_silence() {
    register();
    let node = TTSProviderNode::new("tts", "mock-tts-silent");
    let mut ctx = DAGContext::new("s").with_timeout(Duration::from_millis(300));
    let result = node
        .execute(DAGData::Text("hello".into()), &mut ctx)
        .await;
    assert!(
        matches!(result, Err(DAGError::TTSProviderError { .. })),
        "a synthesis with no audio and no completion is an error: {result:?}"
    );
}

fn speech_dag(tts_provider: &str, with_output: bool) -> DAGDefinition {
    let mut nodes = vec![
        json!({ "id": "in", "type": "text_input" }),
        json!({ "id": "tts", "type": "tts_provider", "provider": tts_provider }),
    ];
    let mut edges = vec![json!({ "from": "in", "to": "tts" })];
    let mut exit = "tts";
    if with_output {
        nodes.push(json!({ "id": "out", "type": "audio_output", "destination": "web_socket" }));
        edges.push(json!({ "from": "tts", "to": "out" }));
        exit = "out";
    }
    serde_json::from_value(json!({
        "id": "speech", "name": "speech", "nodes": nodes, "edges": edges,
        "entry_node": "in", "exit_nodes": [exit]
    }))
    .unwrap()
}

/// `text → tts → audio_output`: the audio reaches the session sink well inside the 30 s node
/// timeout, as one final utterance.
#[tokio::test]
async fn dag_turn_delivers_speech_to_the_session_sink() {
    register();
    let dag = DAGCompiler::new()
        .compile(speech_dag("mock-tts-async", true))
        .unwrap();
    let (tx, mut rx) = mpsc::channel(16);
    let mut ctx = DAGContext::new("s").with_output_tx(tx);
    let started = Instant::now();
    DAGExecutor::new()
        .execute_from(&dag, "tts", DAGData::Text("hi".into()), &mut ctx)
        .await
        .expect("the turn completes");
    assert!(started.elapsed() < Duration::from_secs(2));
    match rx.try_recv() {
        Ok(DagOutput::Audio(audio)) => assert_eq!(audio.data.len(), CHUNK * CHUNKS),
        other => panic!("expected the speech on the sink, got {other:?}"),
    }
}

/// A graph that ends in `tts` (no output node) is heard once the session routes its speech.
#[tokio::test]
async fn unread_speech_is_routed_and_delivered() {
    register();
    let mut def = speech_dag("mock-tts-async", false);
    assert_eq!(def.route_unread_speech().len(), 1);
    let dag = DAGCompiler::new().compile(def).unwrap();
    let (tx, mut rx) = mpsc::channel(16);
    let mut ctx = DAGContext::new("s").with_output_tx(tx);
    DAGExecutor::new()
        .execute_from(&dag, "tts", DAGData::Text("hi".into()), &mut ctx)
        .await
        .unwrap();
    assert!(matches!(rx.try_recv(), Ok(DagOutput::Audio(_))));
}

/// A node's failure is attributed to it, so the session can tell the client which node failed.
#[tokio::test]
async fn failed_turn_names_its_node() {
    register();
    let dag = DAGCompiler::new()
        .compile(speech_dag("mock-tts-refuses", true))
        .unwrap();
    let mut ctx = DAGContext::new("s");
    let err = DAGExecutor::new()
        .execute_from(&dag, "tts", DAGData::Text("hi".into()), &mut ctx)
        .await
        .expect_err("the provider refused");
    assert!(matches!(err, DAGError::TTSProviderError { .. }), "{err}");
    let failed = ctx.failed_node.expect("the failing node is recorded");
    assert_eq!(failed.id, "tts");
    assert_eq!(failed.node_type, "tts_provider");
}

/// Barge-in reaches a node that is mid-call: cancelling the turn's token ends the turn at once with
/// `Cancelled`, instead of waiting for the TTS call to finish (batch executor).
#[tokio::test]
async fn a_cancelled_turn_stops_mid_node_in_the_batch_executor() {
    register();
    let dag = DAGCompiler::new()
        .compile(speech_dag("mock-tts-slow", true))
        .unwrap();
    let (tx, _rx) = mpsc::channel(64);
    let mut ctx = DAGContext::new("s").with_output_tx(tx);
    let cancel = ctx.cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    });
    let started = Instant::now();
    let err = DAGExecutor::new()
        .execute_from(&dag, "tts", DAGData::Text("a long answer".into()), &mut ctx)
        .await
        .expect_err("the turn was cancelled");
    assert!(matches!(err, DAGError::Cancelled), "{err}");
    assert!(started.elapsed() < Duration::from_millis(800), "stopped mid-speech: {:?}", started.elapsed());
    assert!(ctx.failed_node.is_none(), "a cancellation is not a node failure");
}

/// The streaming executor (what a session's turns run on) reports a cancelled turn as `Cancelled`
/// — not as a failed node the client would be told about — and stops promptly.
#[tokio::test]
async fn a_cancelled_streaming_turn_is_cancelled_not_failed() {
    register();
    let dag = DAGCompiler::new()
        .compile(speech_dag("mock-tts-slow", true))
        .unwrap();
    let (tx, _rx) = mpsc::channel(64);
    let mut ctx = DAGContext::new("s").with_output_tx(tx);
    let cancel = ctx.cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
    });
    let started = Instant::now();
    let err = DAGExecutor::new()
        .execute_streaming_from(&dag, "tts", DAGData::Text("a long answer".into()), &mut ctx)
        .await
        .expect_err("the turn was cancelled");
    assert!(matches!(err, DAGError::Cancelled), "{err}");
    assert!(started.elapsed() < Duration::from_millis(800), "{:?}", started.elapsed());
    assert!(ctx.failed_node.is_none());
}

/// On the streaming executor the speech reaches the session sink exactly once (the terminal
/// forwarder delivers it; the output node does not deliver it a second time).
#[tokio::test]
async fn a_streaming_turn_delivers_its_speech_once() {
    register();
    let dag = DAGCompiler::new()
        .compile(speech_dag("mock-tts-async", true))
        .unwrap();
    let (tx, mut rx) = mpsc::channel(64);
    let mut ctx = DAGContext::new("s").with_output_tx(tx);
    DAGExecutor::new()
        .execute_streaming_from(&dag, "tts", DAGData::Text("hi".into()), &mut ctx)
        .await
        .expect("the turn completes");
    drop(ctx);
    let mut bytes = 0;
    while let Ok(out) = rx.try_recv() {
        if let DagOutput::Audio(audio) = out {
            bytes += audio.data.len();
        }
    }
    assert_eq!(bytes, CHUNK * CHUNKS, "every chunk once, none twice");
}

/// A node that fails on the streaming executor is still named, as on the batch one.
#[tokio::test]
async fn a_failed_streaming_turn_names_its_node() {
    register();
    let dag = DAGCompiler::new()
        .compile(speech_dag("mock-tts-refuses", true))
        .unwrap();
    let mut ctx = DAGContext::new("s");
    DAGExecutor::new()
        .execute_streaming_from(&dag, "tts", DAGData::Text("hi".into()), &mut ctx)
        .await
        .expect_err("the provider refused");
    assert_eq!(ctx.failed_node.expect("named").id, "tts");
}
