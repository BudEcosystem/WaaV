//! Mock protocol + transport for driving `RealtimeSession<P>` hermetically in
//! unit tests — no network, no provider. Proves the generic machinery (dispatch,
//! barge-in/truncate, reconnect, quick-failure cutoff, replay, multi-event)
//! ONCE, so it is never re-tested per provider.
#![cfg(test)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;

use super::super::base::{
    FunctionCallRequest, RealtimeConfig, RealtimeError, RealtimeResponseOverride, RealtimeResult,
    ReplayConversationItem, SpeechEvent, TranscriptRole,
};
use super::event::{ConnectSpec, Inbound, OutFrame, ProtocolCaps, S2sEvent};
use super::protocol::RealtimeProtocol;
use super::transport::{RealtimeTransport, RealtimeTransportFactory};

/// One scripted inbound step for the mock transport.
#[derive(Clone)]
pub enum Step {
    /// A text payload the protocol will map to events.
    Text(String),
    /// A transport error (triggers the driver's reconnect path).
    Error(String),
    /// A clean close (recv returns None).
    Close,
}

/// Shared test observation handle.
#[derive(Clone, Default)]
pub struct MockObserver {
    pub sent: Arc<Mutex<Vec<OutFrame>>>,
    pub connects: Arc<AtomicUsize>,
}

impl MockObserver {
    pub fn sent_text(&self) -> Vec<String> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter_map(|f| match f {
                OutFrame::Text(s) => Some(s.clone()),
                OutFrame::Binary(_) => None,
            })
            .collect()
    }
    pub fn connect_count(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }
}

/// Transport that drains a script then stays open (pending) so the session lives
/// until `disconnect()`.
struct MockTransport {
    script: VecDeque<Step>,
    obs: MockObserver,
}

#[async_trait]
impl RealtimeTransport for MockTransport {
    async fn send(&mut self, frame: OutFrame) -> RealtimeResult<()> {
        self.obs.sent.lock().unwrap().push(frame);
        Ok(())
    }
    async fn recv(&mut self) -> Option<RealtimeResult<OutFrame>> {
        match self.script.pop_front() {
            Some(Step::Text(s)) => Some(Ok(OutFrame::Text(s))),
            Some(Step::Error(e)) => Some(Err(RealtimeError::WebSocketError(e))),
            Some(Step::Close) => None,
            None => std::future::pending().await, // stay open until aborted
        }
    }
}

/// Factory yielding one scripted transport per (re)connect.
struct MockFactory {
    scripts: Arc<Mutex<VecDeque<Vec<Step>>>>,
    obs: MockObserver,
}

#[async_trait]
impl RealtimeTransportFactory for MockFactory {
    async fn connect(&self, _spec: ConnectSpec) -> RealtimeResult<Box<dyn RealtimeTransport>> {
        self.obs.connects.fetch_add(1, Ordering::SeqCst);
        let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(MockTransport {
            script: script.into(),
            obs: self.obs.clone(),
        }))
    }
}

/// Mock protocol. `Wire = String`; serialize → `OutFrame::Text`. Inbound text is a
/// tiny command language so tests can script server events:
/// `session:<id>` · `tx:user|asst:interim|final:<text>` · `audio:<n>` ·
/// `done:<rid>` · `call:<id>:<name>:<args>` · `track:<id>:<name>` ·
/// `speech:start|stop` · `interrupt` · `resume:<h>` · `send:<text>` (an
/// inbound-triggered OUTBOUND frame — the ping→pong keystone) · `err:<msg>` ·
/// `multi:<a>;;<b>`.
pub struct MockProtocol {
    caps: ProtocolCaps,
    scripts: Arc<Mutex<VecDeque<Vec<Step>>>>,
    obs: MockObserver,
}

impl MockProtocol {
    pub fn new(caps: ProtocolCaps, scripts: Vec<Vec<Step>>) -> (Self, MockObserver) {
        let obs = MockObserver::default();
        (
            Self {
                caps,
                scripts: Arc::new(Mutex::new(scripts.into())),
                obs: obs.clone(),
            },
            obs,
        )
    }

    fn map_one(cmd: &str) -> S2sEvent {
        let parts: Vec<&str> = cmd.splitn(4, ':').collect();
        match parts.as_slice() {
            ["session", id] => S2sEvent::SessionReady {
                session_id: Some(id.to_string()),
            },
            ["tx", role, fin, text] => S2sEvent::Transcript {
                role: if *role == "asst" {
                    TranscriptRole::Assistant
                } else {
                    TranscriptRole::User
                },
                text: text.to_string(),
                is_final: *fin == "final",
                item_id: None,
            },
            ["audio", n] => S2sEvent::Audio {
                data: Bytes::from(vec![0u8; n.parse().unwrap_or(0)]),
                item_id: Some("item1".into()),
                response_id: Some("resp1".into()),
            },
            ["done", rid] => S2sEvent::ResponseDone {
                response_id: rid.to_string(),
            },
            ["call", id, name, args] => S2sEvent::FunctionCall(FunctionCallRequest {
                call_id: id.to_string(),
                name: name.to_string(),
                arguments: args.to_string(),
                item_id: None,
            }),
            ["track", id, name] => S2sEvent::TrackPendingCall {
                call_id: id.to_string(),
                name: name.to_string(),
            },
            ["speech", "start"] => S2sEvent::Speech(SpeechEvent::Started {
                audio_start_ms: 0,
                item_id: None,
            }),
            ["speech", "stop"] => S2sEvent::Speech(SpeechEvent::Stopped {
                audio_end_ms: 0,
                item_id: None,
            }),
            ["interrupt"] => S2sEvent::InterruptedByServer,
            // Inbound-triggered OUTBOUND frame: the keystone ping→pong path. The
            // driver must SEND this (not call a callback) — proven by the
            // `inbound_command_triggers_outbound_send_frame` test below.
            ["send", text] => S2sEvent::SendFrame(OutFrame::Text(text.to_string())),
            ["resume", h] => S2sEvent::ResumptionHandle(h.to_string()),
            ["err", msg] => S2sEvent::Error(RealtimeError::ProviderError(msg.to_string())),
            _ => S2sEvent::Ignore,
        }
    }
}

impl RealtimeProtocol for MockProtocol {
    type Wire = String;

    fn from_config(_cfg: &RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self::new(ProtocolCaps::default(), Vec::new()).0)
    }

    fn transport_factory(&self) -> Arc<dyn RealtimeTransportFactory> {
        Arc::new(MockFactory {
            scripts: self.scripts.clone(),
            obs: self.obs.clone(),
        })
    }

    fn provider_id(&self) -> &'static str {
        "mock"
    }
    fn caps(&self) -> ProtocolCaps {
        self.caps
    }
    fn connect_spec(&self, _cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        Ok(ConnectSpec::WebSocket {
            url: "wss://mock/realtime".into(),
            headers: vec![],
        })
    }
    fn build_session_config(&self, _cfg: &RealtimeConfig, resumption: Option<&str>) -> Vec<String> {
        vec![match resumption {
            Some(h) => format!("session_update:resume={h}"),
            None => "session_update".into(),
        }]
    }
    fn map_server_event(&self, raw: Inbound<'_>) -> Vec<S2sEvent> {
        match raw {
            Inbound::Text(t) => {
                if let Some(rest) = t.strip_prefix("multi:") {
                    rest.split(";;").map(Self::map_one).collect()
                } else {
                    vec![Self::map_one(t)]
                }
            }
            Inbound::Binary(b) => vec![S2sEvent::Audio {
                data: Bytes::copy_from_slice(b),
                item_id: None,
                response_id: None,
            }],
        }
    }
    fn encode_user_audio(&self, pcm: &[u8]) -> String {
        format!("audio_in:{}", pcm.len())
    }
    fn send_text(&self, text: &str) -> Vec<String> {
        vec![format!("text:{text}")]
    }
    fn create_response(&self, ov: Option<&RealtimeResponseOverride>) -> Vec<String> {
        vec![match ov {
            Some(o) => format!("response_create:oob={}", o.out_of_band),
            None => "response_create".into(),
        }]
    }
    fn commit_turn(&self) -> Vec<String> {
        vec!["commit".into()]
    }
    fn cancel_response(&self) -> Vec<String> {
        vec!["cancel".into()]
    }
    fn truncate(&self, item_id: &str, audio_end_ms: u64) -> Vec<String> {
        vec![format!("truncate:{item_id}:{audio_end_ms}")]
    }
    fn clear_input_buffer(&self) -> Vec<String> {
        vec!["clear".into()]
    }
    fn format_tool_result(&self, call_id: &str, result: &str) -> Vec<String> {
        vec![format!("tool_result:{call_id}:{result}")]
    }
    fn replay_item(&self, item: &ReplayConversationItem) -> Vec<String> {
        vec![format!("replay:{}:{}", item.role, item.text)]
    }
    fn serialize(&self, msg: &String) -> RealtimeResult<OutFrame> {
        Ok(OutFrame::Text(msg.clone()))
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::scaffold::session::RealtimeSession;
    use crate::core::realtime::{BaseRealtime, ConnectionState, TranscriptResult};
    use std::time::Duration;
    use tokio::sync::Mutex as TMutex;

    fn cfg() -> RealtimeConfig {
        RealtimeConfig {
            api_key: "k".into(),
            model: "m".into(),
            ..Default::default()
        }
    }

    /// Poll until `pred` holds or a timeout (lets the supervisor task run).
    async fn until<F: Fn() -> bool>(pred: F) {
        for _ in 0..200 {
            if pred() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!("condition not met within timeout");
    }

    #[tokio::test]
    async fn connects_sends_session_config_and_becomes_ready() {
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![vec![]]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| s.is_ready()).await;
        assert_eq!(s.get_connection_state(), ConnectionState::Connected);
        until(|| obs.sent_text().contains(&"session_update".to_string())).await;
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn dispatches_transcript_audio_and_done_to_callbacks() {
        let script = vec![
            Step::Text("tx:user:final:hello world".into()),
            Step::Text("audio:96".into()),
            Step::Text("done:resp1".into()),
        ];
        let (proto, _obs) = MockProtocol::new(ProtocolCaps::default(), vec![script]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();

        let transcripts: Arc<TMutex<Vec<TranscriptResult>>> = Arc::new(TMutex::new(Vec::new()));
        let audio_bytes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let dones: Arc<TMutex<Vec<String>>> = Arc::new(TMutex::new(Vec::new()));
        {
            let t = transcripts.clone();
            s.on_transcript(Arc::new(move |r| {
                let t = t.clone();
                Box::pin(async move { t.lock().await.push(r) })
            }))
            .unwrap();
            let a = audio_bytes.clone();
            s.on_audio(Arc::new(move |d| {
                let a = a.clone();
                Box::pin(async move {
                    a.fetch_add(d.data.len(), Ordering::SeqCst);
                })
            }))
            .unwrap();
            let d = dones.clone();
            s.on_response_done(Arc::new(move |rid| {
                let d = d.clone();
                Box::pin(async move { d.lock().await.push(rid) })
            }))
            .unwrap();
        }
        s.connect().await.unwrap();
        until(|| audio_bytes.load(Ordering::SeqCst) == 96).await;
        let tx = transcripts.lock().await;
        assert_eq!(tx.len(), 1);
        assert_eq!(tx[0].text, "hello world");
        assert!(tx[0].is_final);
        assert_eq!(dones.lock().await.as_slice(), &["resp1".to_string()]);
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn interim_assistant_deltas_accumulate_running_text() {
        let script = vec![
            Step::Text("tx:asst:interim:He".into()),
            Step::Text("tx:asst:interim:llo".into()),
            Step::Text("tx:asst:final:Hello.".into()),
        ];
        let (proto, _o) = MockProtocol::new(ProtocolCaps::default(), vec![script]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let c = seen.clone();
        s.on_transcript(Arc::new(move |r| {
            let c = c.clone();
            Box::pin(async move { c.lock().unwrap().push(r.text) })
        }))
        .unwrap();
        s.connect().await.unwrap();
        until(|| seen.lock().unwrap().len() >= 3).await;
        let v = seen.lock().unwrap();
        assert_eq!(v.as_slice(), &["He".to_string(), "Hello".to_string(), "Hello.".to_string()]);
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn barge_in_truncates_at_received_duration_and_clears_playback() {
        // 96 bytes @ 48 B/ms = 2ms of audio for item "item1".
        let script = vec![Step::Text("audio:96".into())];
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![script]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| s.is_ready()).await;
        // let the audio chunk dispatch + set playback
        tokio::time::sleep(Duration::from_millis(20)).await;
        let truncated = s.truncate_current_response().await.unwrap();
        assert_eq!(truncated, Some(("item1".to_string(), 2)));
        // wire emitted + a second truncate is now a no-op (playback cleared)
        until(|| obs.sent_text().iter().any(|w| w == "truncate:item1:2")).await;
        assert_eq!(s.truncate_current_response().await.unwrap(), None);
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn create_response_and_overrides_emit_wire() {
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![vec![]]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| s.is_ready()).await;
        s.create_response().await.unwrap();
        s.create_response_with(RealtimeResponseOverride {
            out_of_band: true,
            ..Default::default()
        })
        .await
        .unwrap();
        s.cancel_response().await.unwrap();
        until(|| {
            let w = obs.sent_text();
            w.contains(&"response_create".to_string())
                && w.contains(&"response_create:oob=true".to_string())
                && w.contains(&"cancel".to_string())
        })
        .await;
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn one_message_can_yield_multiple_events() {
        let script = vec![Step::Text("multi:tx:user:final:hi;;done:r9".into())];
        let (proto, _o) = MockProtocol::new(ProtocolCaps::default(), vec![script]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let dn = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let n2 = n.clone();
        s.on_transcript(Arc::new(move |_| {
            let n2 = n2.clone();
            Box::pin(async move {
                n2.fetch_add(1, Ordering::SeqCst);
            })
        }))
        .unwrap();
        let d2 = dn.clone();
        s.on_response_done(Arc::new(move |_| {
            let d2 = d2.clone();
            Box::pin(async move {
                d2.fetch_add(1, Ordering::SeqCst);
            })
        }))
        .unwrap();
        s.connect().await.unwrap();
        until(|| n.load(Ordering::SeqCst) == 1 && dn.load(Ordering::SeqCst) == 1).await;
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn quick_failure_cutoff_stops_the_storm() {
        // Every connect immediately errors → quick failure. After 3, give up.
        let scripts = vec![
            vec![Step::Error("boom".into())],
            vec![Step::Error("boom".into())],
            vec![Step::Error("boom".into())],
            vec![Step::Error("boom".into())],
        ];
        let mut c = cfg();
        c.reconnection = Some(crate::core::realtime::base::ReconnectionConfig {
            initial_delay_ms: 1,
            max_delay_ms: 2,
            ..Default::default()
        });
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), scripts);
        let mut s = RealtimeSession::from_parts(proto, c).unwrap();
        s.connect().await.unwrap();
        until(|| s.get_connection_state() == ConnectionState::Failed).await;
        // Stopped at the cutoff (3 quick failures), not hammering forever.
        assert!(obs.connect_count() <= 4, "connects={}", obs.connect_count());
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn connect_is_synchronously_ready() {
        // Review finding #1: after connect().await returns Ok, the session MUST be
        // ready (the consumer sends audio immediately) — no polling.
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![vec![]]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        assert!(s.is_ready(), "must be ready immediately after connect()");
        assert_eq!(s.get_connection_state(), ConnectionState::Connected);
        // session config was sent before readiness was signaled
        assert!(obs.sent_text().contains(&"session_update".to_string()));
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn state_changing_methods_fail_fast_when_not_connected() {
        // Review finding #6: sends before connect must error, not silently buffer.
        let (proto, _o) = MockProtocol::new(ProtocolCaps::default(), vec![vec![]]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        assert!(matches!(
            s.create_response().await,
            Err(crate::core::realtime::RealtimeError::NotConnected)
        ));
        assert!(matches!(
            s.cancel_response().await,
            Err(crate::core::realtime::RealtimeError::NotConnected)
        ));
        assert!(matches!(
            s.send_text("hi").await,
            Err(crate::core::realtime::RealtimeError::NotConnected)
        ));
    }

    #[tokio::test]
    async fn server_interruption_emits_cancel_then_truncate() {
        // Review finding #5: InterruptedByServer must cancel + truncate, not no-op.
        let script = vec![
            Step::Text("audio:96".into()), // playback: item1, 2ms received
            Step::Text("interrupt".into()),
        ];
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![script]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| {
            let w = obs.sent_text();
            w.iter().any(|x| x == "cancel") && w.iter().any(|x| x.starts_with("truncate:item1:"))
        })
        .await;
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn inbound_command_triggers_outbound_send_frame() {
        // THE KEYSTONE GATE (Task A): an inbound event that maps to
        // `S2sEvent::SendFrame` must be SENT by the driver on the outbound path
        // (ConvAI server `ping` → client `pong`), NOT delivered to a callback.
        // The MockObserver records every frame the transport sent, so seeing
        // "pong" there proves the supervisor routed SendFrame to `transport.send`.
        let script = vec![Step::Text("send:pong".into())];
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![script]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        // The pong shows up among the sent text frames (alongside session_update).
        until(|| obs.sent_text().iter().any(|w| w == "pong")).await;
        s.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn truncate_noop_when_provider_lacks_support() {
        let caps = ProtocolCaps {
            supports_truncate: false,
            ..Default::default()
        };
        let (proto, obs) = MockProtocol::new(caps, vec![vec![Step::Text("audio:96".into())]]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| s.is_ready()).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(s.truncate_current_response().await.unwrap(), None);
        assert!(!obs.sent_text().iter().any(|w| w.starts_with("truncate")));
        s.disconnect().await.unwrap();
    }
}
