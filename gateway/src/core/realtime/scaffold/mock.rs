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
    /// A close whose reason refuses the session's configuration.
    Refused(String),
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
            Some(Step::Refused(why)) => Some(Err(RealtimeError::Refused(why))),
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
    max_connection: Option<std::time::Duration>,
    /// [`RealtimeProtocol::awaits_session_ready`].
    awaits_ready: bool,
    /// [`RealtimeProtocol::update_by_reconnect`].
    update_reconnect: bool,
    /// [`RealtimeProtocol::after_replay`] and [`RealtimeProtocol::close_wires`]: `opened`
    /// and `closed`, as Nova Sonic's fixes have them.
    closing: bool,
}

impl MockProtocol {
    pub fn new(caps: ProtocolCaps, scripts: Vec<Vec<Step>>) -> (Self, MockObserver) {
        let obs = MockObserver::default();
        (
            Self {
                caps,
                scripts: Arc::new(Mutex::new(scripts.into())),
                obs: obs.clone(),
                max_connection: None,
                awaits_ready: false,
                update_reconnect: false,
                closing: false,
            },
            obs,
        )
    }

    /// `opened` follows the replayed history on every connect; `closed` precedes a close on
    /// purpose.
    pub fn closing(mut self) -> Self {
        self.closing = true;
        self
    }

    /// Nothing but the session config goes before `session:<id>` (Gemini's `setupComplete`).
    pub fn awaiting_ready(mut self) -> Self {
        self.awaits_ready = true;
        self
    }

    /// An update is taken on a new connection that resumes the session.
    pub fn updating_by_reconnect(mut self) -> Self {
        self.update_reconnect = true;
        self
    }

    /// A connection cap, as Nova Sonic has.
    pub fn with_max_connection(mut self, d: std::time::Duration) -> Self {
        self.max_connection = Some(d);
        self
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
            ["goaway", ms] => S2sEvent::GoAway {
                time_left: ms.parse().ok().map(std::time::Duration::from_millis),
            },
            ["usage", n] => S2sEvent::Usage(super::event::UsageReport {
                tokens: crate::core::realtime_cost::RealtimeUsage {
                    input_audio: n.parse().unwrap_or(0),
                    ..Default::default()
                },
                seconds: None,
                cumulative: false,
            }),
            ["err", msg] => S2sEvent::Error(RealtimeError::ProviderError(msg.to_string())),
            ["refused", msg] => S2sEvent::Error(RealtimeError::Refused(msg.to_string())),
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
    fn max_connection(&self) -> Option<std::time::Duration> {
        self.max_connection
    }
    fn awaits_session_ready(&self) -> bool {
        self.awaits_ready
    }
    fn update_by_reconnect(&self, _old: &RealtimeConfig, _new: &RealtimeConfig) -> bool {
        self.update_reconnect
    }
    fn connect_spec(&self, _cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
        Ok(ConnectSpec::WebSocket {
            url: "wss://mock/realtime".into(),
            headers: vec![],
        })
    }
    fn build_session_config(&self, cfg: &RealtimeConfig, resumption: Option<&str>) -> Vec<String> {
        let mut wire = String::from("session_update");
        if let Some(h) = resumption {
            wire.push_str(&format!(":resume={h}"));
        }
        // The one config field the mock wire shows, so a test can see which config was sent.
        if let Some(voice) = &cfg.voice {
            wire.push_str(&format!(":voice={voice}"));
        }
        vec![wire]
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
    fn after_replay(&self) -> Vec<String> {
        if self.closing {
            vec!["opened".into()]
        } else {
            Vec::new()
        }
    }
    fn close_wires(&self) -> Vec<String> {
        if self.closing {
            vec!["closed".into()]
        } else {
            Vec::new()
        }
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
    use crate::core::realtime::{BaseRealtime, ConnectionState, RealtimeVerb, TranscriptResult};
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
        // Scope the std::Mutex guard to this statement so it drops BEFORE the
        // `.await` below (clippy::await_holding_lock; the guard is otherwise
        // benign here — `disconnect()` never touches `seen`).
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &["He".to_string(), "Hello".to_string(), "Hello.".to_string()]
        );
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

    /// FRD-023 RT7.0 — the event tap sees every normalized event in wire order, including the
    /// ones no native callback carries (`Usage`).
    #[tokio::test]
    async fn the_event_tap_sees_every_event_in_order() {
        let script = vec![Step::Text("multi:usage:7;;audio:10;;done:r1".into())];
        let (proto, _o) = MockProtocol::new(ProtocolCaps::default(), vec![script]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let c = seen.clone();
        s.on_event(Arc::new(move |ev| {
            let label = match ev {
                S2sEvent::Usage(u) => format!("usage:{}", u.tokens.input_audio),
                S2sEvent::Audio { data, .. } => format!("audio:{}", data.len()),
                S2sEvent::ResponseDone { response_id } => format!("done:{response_id}"),
                other => format!("{other:?}"),
            };
            c.lock().unwrap().push(label);
            Box::pin(async {})
        }))
        .unwrap();
        s.connect().await.unwrap();
        until(|| seen.lock().unwrap().len() >= 3).await;
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &["usage:7", "audio:10", "done:r1"]
        );
        s.disconnect().await.unwrap();
    }

    /// TC-XL-04 (driver half) — a `goAway` replaces the connection AT ONCE (no backoff: the
    /// default first delay is ~1 s) carrying the resumption handle, and is not a failure.
    #[tokio::test]
    async fn go_away_reconnects_at_once_with_the_resumption_handle() {
        let scripts = vec![
            vec![
                Step::Text("resume:h1".into()),
                Step::Text("goaway:0".into()),
            ],
            vec![],
        ];
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), scripts);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        let reconnects = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let r = reconnects.clone();
        s.on_reconnection(Arc::new(move |ev| {
            assert!(ev.success);
            r.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }))
        .unwrap();
        s.connect().await.unwrap();
        until(|| obs.connect_count() == 2).await;
        until(|| {
            obs.sent_text()
                .contains(&"session_update:resume=h1".to_string())
        })
        .await;
        until(|| reconnects.load(Ordering::SeqCst) == 1).await;
        assert!(s.is_ready());
        s.disconnect().await.unwrap();
    }

    /// A `goAway` during a response waits for the response to finish (its deadline is far).
    #[tokio::test]
    async fn go_away_waits_for_the_response_in_flight() {
        let scripts = vec![
            vec![
                Step::Text("audio:10".into()),
                Step::Text("goaway:60000".into()),
            ],
            vec![],
        ];
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), scripts);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(obs.connect_count(), 1, "cut the response off");
        s.disconnect().await.unwrap();

        let scripts = vec![
            vec![
                Step::Text("audio:10".into()),
                Step::Text("goaway:60000".into()),
                Step::Text("done:r1".into()),
            ],
            vec![],
        ];
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), scripts);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| obs.connect_count() == 2).await;
        s.disconnect().await.unwrap();
    }

    /// `after_replay` follows the session config on the first connect and the replayed
    /// history on a reconnect; `close_wires` precede a planned replacement and a disconnect
    /// (Nova Sonic's fixes `nova_history_before_audio` and `nova_close_sequence`).
    #[tokio::test]
    async fn a_provider_opens_after_the_history_and_closes_on_purpose() {
        let (proto, obs) = MockProtocol::new(
            ProtocolCaps::default(),
            vec![vec![Step::Text("tx:user:final:hello".into())], vec![]],
        );
        let proto = proto
            .closing()
            .with_max_connection(Duration::from_millis(150));
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| obs.connect_count() >= 2 && obs.sent_text().len() >= 6).await;
        let sent = obs.sent_text();
        assert_eq!(sent[..2], ["session_update", "opened"], "{sent:?}");
        assert_eq!(
            sent[2], "closed",
            "the replaced connection closes first: {sent:?}"
        );
        assert_eq!(sent[3], "session_update", "{sent:?}");
        assert!(sent[4].starts_with("replay:"), "{sent:?}");
        assert_eq!(sent[5], "opened", "after the history: {sent:?}");
        s.disconnect().await.unwrap();
        assert_eq!(obs.sent_text().last().map(String::as_str), Some("closed"));
    }

    /// TC-XL-05 (driver half) — a connection cap replaces the connection before the vendor
    /// does, over and over, without ever counting as a quick failure (3 of which stop the
    /// session).
    #[tokio::test]
    async fn a_connection_cap_reconnects_proactively_and_is_not_a_failure() {
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![]);
        let proto = proto.with_max_connection(Duration::from_millis(30));
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| obs.connect_count() >= 5).await;
        assert_ne!(s.get_connection_state(), ConnectionState::Failed);
        s.disconnect().await.unwrap();
    }

    /// A session update survives a reconnect: the supervisor used to re-send the config it was
    /// given at `connect()`, so the voice, instructions and tools set since were lost.
    #[tokio::test]
    async fn a_reconnect_resends_the_updated_session_config() {
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![]);
        // The cap forces the reconnect, well after the update below.
        let proto = proto.with_max_connection(Duration::from_millis(150));
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        s.update_session(RealtimeConfig {
            voice: Some("nova".into()),
            ..Default::default()
        })
        .await
        .unwrap();
        let updated = || {
            obs.sent_text()
                .iter()
                .filter(|wire| *wire == "session_update:voice=nova")
                .count()
        };
        assert_eq!(obs.sent_text()[0], "session_update", "the first connect");
        // One from the update itself, then one per reconnect.
        until(|| obs.connect_count() >= 2 && updated() >= 2).await;
        let sent = obs.sent_text();
        assert_eq!(
            sent.iter().filter(|wire| *wire == "session_update").count(),
            1,
            "no reconnect sent the config of the first connect: {sent:?}"
        );
        s.disconnect().await.unwrap();
    }

    /// A provider that answers its session config first (Gemini's `setupComplete`, fix
    /// `gemini_await_setup`) gets nothing else before it: a connection that never answers is
    /// dropped after `SESSION_READY_TIMEOUT` and dialled again, and the frames queued meanwhile
    /// go on the connection that answered, after its config.
    #[tokio::test(start_paused = true)]
    async fn nothing_goes_before_the_providers_answer_to_its_config() {
        let (proto, obs) = MockProtocol::new(
            ProtocolCaps::default(),
            vec![vec![], vec![Step::Text("session:s2".into())]],
        );
        let mut s = RealtimeSession::from_parts(proto.awaiting_ready(), cfg()).unwrap();
        s.connect().await.unwrap();
        s.send_text("hello").await.unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(obs.sent_text(), ["session_update"], "held until the answer");
        // Past the timeout (paused clock): the next connection answers.
        tokio::time::sleep(crate::core::realtime::scaffold::session::SESSION_READY_TIMEOUT).await;
        until(|| obs.sent_text().iter().any(|w| w == "text:hello")).await;
        assert_eq!(obs.connect_count(), 2);
        assert_eq!(
            obs.sent_text(),
            ["session_update", "session_update", "text:hello"]
        );
        s.disconnect().await.unwrap();
    }

    /// An update the provider takes only by resuming (Gemini, fix `gemini_update_resume`)
    /// is a new connection carrying the handle and the new config, with nothing sent on the
    /// old one; before the provider gave a handle it is refused and the config is kept.
    #[tokio::test]
    async fn an_update_by_reconnect_resumes_with_the_new_config() {
        let nova = || RealtimeConfig {
            voice: Some("nova".into()),
            ..Default::default()
        };
        // No handle yet: refused, the config kept, nothing sent.
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![vec![]]);
        let mut s = RealtimeSession::from_parts(proto.updating_by_reconnect(), cfg()).unwrap();
        s.connect().await.unwrap();
        let refused = s.update_session(nova()).await;
        assert!(
            matches!(refused, Err(RealtimeError::InvalidConfiguration(_))),
            "{refused:?}"
        );
        assert!(s.config().voice.is_none(), "the session keeps its settings");
        assert_eq!(obs.sent_text(), ["session_update"]);
        s.disconnect().await.unwrap();

        // With the provider's handle the update is a new connection resuming the session.
        let (proto, obs) = MockProtocol::new(
            ProtocolCaps::default(),
            vec![vec![Step::Text("resume:h1".into())], vec![]],
        );
        let mut s = RealtimeSession::from_parts(proto.updating_by_reconnect(), cfg()).unwrap();
        s.connect().await.unwrap();
        until(|| obs.sent_text().len() == 1).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        s.update_session(nova()).await.unwrap();
        until(|| obs.connect_count() >= 2 && obs.sent_text().len() >= 2).await;
        assert_eq!(
            obs.sent_text(),
            ["session_update", "session_update:resume=h1:voice=nova"]
        );
        s.disconnect().await.unwrap();
    }

    /// An update by reconnect asked while a `goAway` had already planned the next connection
    /// rides on that connection; it does not replace the new one again.
    #[tokio::test]
    async fn an_update_during_a_planned_reconnect_rides_on_it() {
        let nova = || RealtimeConfig {
            voice: Some("nova".into()),
            ..Default::default()
        };
        // A response in flight holds the `goAway` reconnect until its deadline (~300 ms).
        let (proto, obs) = MockProtocol::new(
            ProtocolCaps::default(),
            vec![
                vec![
                    Step::Text("resume:h1".into()),
                    Step::Text("audio:10".into()),
                    Step::Text("goaway:800".into()),
                ],
                vec![],
                vec![],
            ],
        );
        let mut s = RealtimeSession::from_parts(proto.updating_by_reconnect(), cfg()).unwrap();
        s.connect().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        s.update_session(nova()).await.unwrap();
        until(|| obs.connect_count() >= 2).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(obs.connect_count(), 2, "{:?}", obs.sent_text());
        assert_eq!(
            obs.sent_text(),
            ["session_update", "session_update:resume=h1:voice=nova"]
        );
        s.disconnect().await.unwrap();
    }

    /// The interaction verbs this build lists for a provider are exactly those its protocol
    /// has a wire for (the mock is in no row, and has none).
    fn assert_table_lists_the_verbs_of<P: RealtimeProtocol>() {
        assert_table_lists_the_verbs_with::<P>(&cfg());
    }

    fn assert_table_lists_the_verbs_with<P: RealtimeProtocol>(cfg: &RealtimeConfig) {
        let protocol = P::from_config(cfg).unwrap();
        let has = [
            RealtimeVerb::SayExact("x"),
            RealtimeVerb::SayGuided("x"),
            RealtimeVerb::AddContext("x"),
            RealtimeVerb::StopOutput,
        ]
        .map(|verb| protocol.verb(&verb).is_some());
        let provider = protocol.provider_id();
        let listed = crate::core::interaction::regime::verbs(provider);
        assert_eq!(listed, has, "{provider}");
    }

    #[test]
    fn the_capability_table_lists_the_verbs_each_protocol_has() {
        assert_table_lists_the_verbs_of::<crate::core::realtime::openai::protocol::OpenAiProtocol>(
        );
        assert_table_lists_the_verbs_of::<crate::core::realtime::DeepgramProtocol>();
        assert_table_lists_the_verbs_of::<crate::core::realtime::NovaSonicProtocol>();
        assert_table_lists_the_verbs_of::<MockProtocol>();
        assert_table_lists_the_verbs_of::<crate::core::realtime::GeminiProtocol>();
        assert_table_lists_the_verbs_of::<crate::core::realtime::GrokProtocol>();
        assert_table_lists_the_verbs_with::<crate::core::realtime::AzureProtocol>(
            &RealtimeConfig {
                endpoint: Some("my-resource".into()),
                ..cfg()
            },
        );
        assert_table_lists_the_verbs_of::<crate::core::realtime::UltravoxProtocol>();
        assert_table_lists_the_verbs_of::<crate::core::realtime::ElevenLabsProtocol>();
        assert_table_lists_the_verbs_of::<crate::core::realtime::InworldProtocol>();
        assert_table_lists_the_verbs_of::<crate::core::realtime::hume::protocol::HumeProtocol>();
        assert_table_lists_the_verbs_with::<crate::core::realtime::YandexProtocol>(
            &RealtimeConfig {
                endpoint: Some("b1gfolder".into()),
                ..cfg()
            },
        );
    }

    /// The scaffold sends a verb's wires, and refuses a verb the protocol has no wire for.
    #[tokio::test]
    async fn apply_verb_is_refused_when_the_protocol_has_no_wire() {
        let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), vec![vec![]]);
        let mut s = RealtimeSession::from_parts(proto, cfg()).unwrap();
        s.connect().await.unwrap();
        for verb in [
            RealtimeVerb::SayExact("hello"),
            RealtimeVerb::SayGuided("greet"),
            RealtimeVerb::AddContext("note"),
            RealtimeVerb::StopOutput,
        ] {
            let refused = s.apply_verb(verb).await;
            assert!(
                matches!(refused, Err(RealtimeError::InvalidConfiguration(_))),
                "{verb:?}: {refused:?}"
            );
        }
        assert_eq!(obs.sent_text(), ["session_update"], "nothing went out");
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
    async fn handshake_then_immediate_close_trips_shared_breaker_fatal() {
        use crate::core::resilience::{
            CircuitBreaker, CircuitBreakerConfig, ReconnectGovernor, ResilienceHandles,
        };
        use std::sync::Arc;
        // Each connection: the DIAL succeeds, then the server immediately closes
        // (recv → None) inside MIN_STABLE_CONNECTION — the bad-credential
        // "handshake-OK-then-drop" signature. This is DISTINCT from `Step::Error`
        // (a dial failure → `record_failure`, the failure-rate window): it
        // exercises the connection-ended `record_connection_closed` feed that
        // drives the SHARED per-provider breaker's FATAL fast-trip (review
        // wf_fb932e8d #2). Without that feed the LOCAL cutoff still bounds THIS
        // session's storm, but other sessions of the same provider would get NO
        // cross-session protection (the breaker's `quick_failures` would stay 0).
        let scripts = vec![
            vec![Step::Close],
            vec![Step::Close],
            vec![Step::Close],
            vec![Step::Close],
        ];
        let mut c = cfg();
        c.reconnection = Some(crate::core::realtime::base::ReconnectionConfig {
            initial_delay_ms: 1,
            max_delay_ms: 2,
            ..Default::default()
        });
        let (proto, _obs) = MockProtocol::new(ProtocolCaps::default(), scripts);
        let mut s = RealtimeSession::from_parts(proto, c).unwrap();

        // Inject a SHARED breaker (default config = 3 quick failures, 5s stable)
        // and keep a handle so we can assert its cross-session FATAL state — the
        // exact state a SECOND session of this provider would observe.
        let breaker = Arc::new(CircuitBreaker::with_label(
            CircuitBreakerConfig::default(),
            "mock-realtime",
        ));
        s.set_resilience(ResilienceHandles {
            governor: Arc::new(ReconnectGovernor::new(4)),
            breaker: breaker.clone(),
        });

        assert!(
            !breaker.is_permanently_failed(),
            "breaker must start healthy"
        );
        s.connect().await.unwrap();
        until(|| s.get_connection_state() == ConnectionState::Failed).await;
        // The realtime path now feeds the breaker its sub-stable server-side
        // closes, so the per-provider breaker trips FATAL — a second session of
        // this provider would now defer its reconnect dials (the cross-session
        // protection that was missing before the fix).
        assert!(
            breaker.is_permanently_failed(),
            "shared breaker must trip FATAL after the handshake-then-drop storm"
        );
        s.disconnect().await.unwrap();
    }

    /// A configuration the provider refused (its error event, or its close reason) ends the
    /// session at once with the provider's reason: no second dial, and nothing counted against
    /// the shared breaker, so the vendor's other sessions are not blocked.
    #[tokio::test]
    async fn a_refused_configuration_ends_the_session_with_its_reason_and_spares_the_breaker() {
        use crate::core::resilience::{
            CircuitBreaker, CircuitBreakerConfig, ReconnectGovernor, ResilienceHandles,
        };
        use std::sync::Arc;
        for script in [
            vec![
                Step::Text("refused:Cannot specify agent.language".into()),
                Step::Close,
            ],
            vec![Step::Refused("Agent agent_x not found: 404".into())],
        ] {
            let scripts = vec![script.clone(), script.clone(), script.clone(), script];
            let mut c = cfg();
            c.reconnection = Some(crate::core::realtime::base::ReconnectionConfig {
                initial_delay_ms: 1,
                max_delay_ms: 2,
                ..Default::default()
            });
            let (proto, obs) = MockProtocol::new(ProtocolCaps::default(), scripts);
            let mut s = RealtimeSession::from_parts(proto, c).unwrap();
            let breaker = Arc::new(CircuitBreaker::with_label(
                CircuitBreakerConfig::default(),
                "mock-realtime",
            ));
            s.set_resilience(ResilienceHandles {
                governor: Arc::new(ReconnectGovernor::new(4)),
                breaker: breaker.clone(),
            });
            let errors = Arc::new(Mutex::new(Vec::<String>::new()));
            let seen = errors.clone();
            s.on_error(Arc::new(move |e: RealtimeError| {
                seen.lock().unwrap().push(e.to_string());
                Box::pin(async {})
            }))
            .unwrap();
            let gave_up = Arc::new(Mutex::new(None::<String>));
            let why = gave_up.clone();
            s.on_reconnection(Arc::new(
                move |e: crate::core::realtime::ReconnectionEvent| {
                    *why.lock().unwrap() = e.error;
                    Box::pin(async {})
                },
            ))
            .unwrap();
            s.connect().await.unwrap();
            until(|| s.get_connection_state() == ConnectionState::Failed).await;
            until(|| gave_up.lock().unwrap().is_some()).await;
            assert_eq!(
                obs.connect_count(),
                1,
                "the refused configuration is not dialled again"
            );
            let reason = gave_up.lock().unwrap().clone().unwrap();
            assert!(reason.starts_with("Refused by the provider: "), "{reason}");
            assert!(
                errors.lock().unwrap().iter().any(|e| e == &reason),
                "{:?}",
                errors.lock().unwrap()
            );
            assert!(!breaker.is_permanently_failed());
            assert!(breaker.allow_request());
            s.disconnect().await.unwrap();
        }
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
