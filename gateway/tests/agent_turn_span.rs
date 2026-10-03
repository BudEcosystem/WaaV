//! Spec 025: the exported `agent_turn` span covers the whole turn.
//!
//! `tracing-opentelemetry` ends a span at its LAST EXIT, not when it closes; the turn span is
//! entered only while the request opens, so without one more exit on the way out every exported
//! turn "lasted" until budprompt's response headers arrived (live: 1153 ms for a 6 s turn).
//!
//! Its own test binary on purpose: the span's callsite is shared with every other engine test,
//! and tracing caches callsite interest process-wide, so in the lib suite a parallel test without
//! a subscriber could leave the span disabled for this one (flaky). Here it is the only test.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SdkTracerProvider, SpanData, SpanExporter};
use serde_json::json;
use tokio::sync::mpsc;
use tracing_subscriber::layer::SubscriberExt;

use waav_gateway::auth::SessionCredential;
use waav_gateway::core::agent::{
    AgentEngine, AgentEvent, AgentSessionConfig, AgentSignal, AgentTurnRequest, SpeechOut,
    TurnBackend, TurnFailure,
};

#[derive(Debug, Clone, Default)]
struct Collect(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Collect {
    fn export(
        &self,
        batch: Vec<SpanData>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        self.0.lock().unwrap().extend(batch);
        std::future::ready(Ok(()))
    }
}

struct Speech;

#[async_trait]
impl SpeechOut for Speech {
    fn clear_epoch(&self) -> usize {
        0
    }
    async fn speak(&self, _text: &str, _epoch: usize, _interruptible: bool) -> bool {
        true
    }
    async fn clear(&self) {}
    fn is_audible(&self) -> bool {
        false
    }
    fn audio_out_ms(&self) -> u64 {
        0
    }
    fn playout_remaining_ms(&self) -> u64 {
        0
    }
}

/// A Bud gateway that answers after 400 ms.
struct SlowBackend;

#[async_trait]
impl TurnBackend for SlowBackend {
    async fn open(
        &self,
        _request: &AgentTurnRequest,
        _bearer: &str,
        _traceparent: Option<&str>,
    ) -> Result<BoxStream<'static, Result<AgentEvent, TurnFailure>>, TurnFailure> {
        let events = async_stream_events();
        Ok(Box::pin(events))
    }
    async fn cancel(&self, _response_id: &str, _prompt_name: &str, _bearer: &str) {}
}

fn async_stream_events() -> impl futures::Stream<Item = Result<AgentEvent, TurnFailure>> + Send {
    futures::stream::unfold(0u8, |step| async move {
        let ev = match step {
            0 => AgentEvent::Created {
                response_id: "resp_1".into(),
                conversation_id: None,
            },
            1 => {
                tokio::time::sleep(Duration::from_millis(400)).await;
                AgentEvent::TextDelta {
                    item_id: None,
                    delta: "Hello there.".into(),
                }
            }
            2 => AgentEvent::Completed {
                usage: None,
                output: vec![],
            },
            _ => return None,
        };
        Some((Ok(ev), step + 1))
    })
}

#[test]
fn the_agent_turn_span_lasts_as_long_as_the_turn() {
    let collect = Collect::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(collect.clone())
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("agent-turn-span")));
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let entry = bud_auth::voice_agent::parse_voice_agent_blob(
            &json!({"prompt_id": "c0de", "version": 1, "stt": {"endpoint_id": "s"}, "tts": {"endpoint_id": "t"},
                    "fillers": {"tool_call_after_ms": 0, "slow_response_after_ms": 0}}).to_string(),
        )
        .unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let engine = AgentEngine::new(
            AgentSessionConfig {
                session_id: "s1".into(),
                prompt_name: "support".into(),
                version: 1,
                entry: Arc::new(entry),
                conversation_id: "conv_voice_s1".into(),
                variables: None,
                text_only: false,
            },
            Arc::new(SlowBackend) as Arc<dyn TurnBackend>,
            Arc::new(Speech) as Arc<dyn SpeechOut>,
            SessionCredential::new("bud-key"),
            tx,
        );
        engine.start_turn("hi".into()).await;
        loop {
            match tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
                Ok(Some(AgentSignal::ResponseDone { .. })) => break,
                Ok(Some(_)) => {}
                other => panic!("no ResponseDone: {other:?}"),
            }
        }
        // The turn task drops its span right after `ResponseDone`.
        tokio::time::sleep(Duration::from_millis(100)).await;
    });

    let spans = collect.0.lock().unwrap().clone();
    let turn = spans
        .iter()
        .find(|s| {
            s.name == "voice.turn"
                && s.attributes.iter().any(|kv| {
                    kv.key.as_str() == "bud.voice.capability" && kv.value.as_str() == "agent_turn"
                })
        })
        .expect("an exported agent_turn span");
    let lasted = turn.end_time.duration_since(turn.start_time).unwrap();
    assert!(
        lasted >= Duration::from_millis(400),
        "the span must cover the turn, not just the request's opening: {lasted:?}"
    );
}
