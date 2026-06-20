//! WaaV Infer **native full-duplex S2S** realtime provider — Regime B (INFER_GATEWAY_INTEGRATION §6,
//! §13 **GW-9**). The Infer engine hosts a Moshi-class interaction model (intrinsic LLM); the gateway
//! is a thin passthrough that streams audio BIDIRECTIONALLY (§6.4). [`InferProtocol`] is the per-
//! provider wire mapper; [`InferRealtime`] is the thin `RealtimeSession<InferProtocol>` newtype that
//! inherits the scaffold's reconnect / barge-in / resilience / dispatch for free (§6.5).
//!
//! # Reference
//! - Endpoint: `ws://127.0.0.1:8088/v1/realtime` (loopback default — §6.2 D5 "loopback-ws first");
//!   a SERVER-CONFIG `realtime_endpoint_override` points it at a remote Infer router / sidecar / mock.
//! - Wire: the engine's native WS v1 (§5.2) — `session.config{task:s2s}` + raw-binary audio BOTH ways.
//! - Turn-taking: INSIDE the model (`emits_user_turn_frames()=true` ⇒ the cascade Smart-Turn is
//!   bypassed, §6.1).

mod protocol;

pub use protocol::{InferProtocol, InferWire};

use async_trait::async_trait;
use bytes::Bytes;

use crate::core::realtime::base::{
    AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback, RealtimeConfig,
    RealtimeErrorCallback, RealtimeResponseOverride, RealtimeResult, ReconnectionCallback,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
};
use crate::core::realtime::scaffold::RealtimeSession;

/// WaaV Infer native-S2S realtime client — a newtype over the generic [`RealtimeSession`] driver
/// parameterized by [`InferProtocol`]. Every [`BaseRealtime`] method delegates to the driver.
pub struct InferRealtime(RealtimeSession<InferProtocol>);

impl InferRealtime {
    /// The shared circuit breaker this session feeds, if injected (for metrics/tests).
    pub fn resilience_breaker(
        &self,
    ) -> Option<&std::sync::Arc<crate::core::resilience::CircuitBreaker>> {
        self.0.resilience_breaker()
    }

    /// Get the session ID if connected.
    pub async fn session_id(&self) -> Option<String> {
        self.0.session_id().await
    }
}

#[async_trait]
impl BaseRealtime for InferRealtime {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        Ok(Self(RealtimeSession::<InferProtocol>::new(config)?))
    }

    async fn connect(&mut self) -> RealtimeResult<()> {
        self.0.connect().await
    }

    async fn disconnect(&mut self) -> RealtimeResult<()> {
        self.0.disconnect().await
    }

    fn is_ready(&self) -> bool {
        self.0.is_ready()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.0.get_connection_state()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()> {
        self.0.send_audio(audio_data).await
    }

    async fn send_text(&mut self, text: &str) -> RealtimeResult<()> {
        self.0.send_text(text).await
    }

    async fn create_response(&mut self) -> RealtimeResult<()> {
        self.0.create_response().await
    }

    async fn create_response_with(
        &mut self,
        overrides: RealtimeResponseOverride,
    ) -> RealtimeResult<()> {
        self.0.create_response_with(overrides).await
    }

    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        self.0.cancel_response().await
    }

    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.0.commit_audio_buffer().await
    }

    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.0.clear_audio_buffer().await
    }

    fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()> {
        self.0.on_transcript(callback)
    }

    fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()> {
        self.0.on_audio(callback)
    }

    fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()> {
        self.0.on_error(callback)
    }

    fn on_function_call(&mut self, callback: FunctionCallCallback) -> RealtimeResult<()> {
        self.0.on_function_call(callback)
    }

    fn on_speech_event(&mut self, callback: SpeechEventCallback) -> RealtimeResult<()> {
        self.0.on_speech_event(callback)
    }

    fn on_response_done(&mut self, callback: ResponseDoneCallback) -> RealtimeResult<()> {
        self.0.on_response_done(callback)
    }

    fn on_reconnection(&mut self, callback: ReconnectionCallback) -> RealtimeResult<()> {
        self.0.on_reconnection(callback)
    }

    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()> {
        self.0.update_session(config).await
    }

    async fn submit_function_result(&mut self, call_id: &str, result: &str) -> RealtimeResult<()> {
        self.0.submit_function_result(call_id, result).await
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "waav-infer",
            "api_type": "WaaV Infer native WS v1 (S2S full-duplex)",
            "version": "1.0.0",
            "regime": "native-s2s",
            "transport": "loopback-ws (UDS/datagram = GW-13)",
            "turn_taking": "intrinsic (model owns it; cascade Smart-Turn bypassed)",
            "features": {
                "full_duplex_audio": true,
                "bidirectional_audio": true,
                "intrinsic_llm": true,
                "barge_in": "model-owned (§6.4)"
            },
            "documentation": "WaaV/INFER_GATEWAY_INTEGRATION.md §6"
        })
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        self.0.set_resilience(resilience)
    }

    fn emits_user_turn_frames(&self) -> bool {
        self.0.emits_user_turn_frames()
    }

    async fn truncate_response(&mut self, item_id: &str, audio_end_ms: u64) -> RealtimeResult<()> {
        self.0.truncate_response(item_id, audio_end_ms).await
    }

    async fn truncate_current_response(&mut self) -> RealtimeResult<Option<(String, u64)>> {
        self.0.truncate_current_response().await
    }

    async fn replay_user_audio_preroll(&mut self) -> RealtimeResult<()> {
        self.0.replay_user_audio_preroll().await
    }

    async fn replay_conversation(&mut self, items: &[ReplayConversationItem]) -> RealtimeResult<()> {
        self.0.replay_conversation(items).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::realtime::base::RealtimeError;
    use crate::core::realtime::scaffold::{
        ConnectSpec, OutFrame, RealtimeProtocol, RealtimeTransport, RealtimeTransportFactory,
    };
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::Mutex as TMutex;

    fn cfg() -> RealtimeConfig {
        RealtimeConfig {
            provider: "waav-infer".into(),
            model: "moshi".into(),
            voice: Some("af_sky".into()),
            ..Default::default()
        }
    }

    /// Poll until `pred` holds or a bounded timeout fires (so a hang FAILS, never blocks forever).
    async fn until<F: Fn() -> bool>(pred: F) {
        for _ in 0..400 {
            if pred() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!("condition not met within the bounded timeout (would be a deadlock)");
    }

    // ── A local full-duplex test-double transport (no network, no Infer engine) ──
    //
    // The scaffold's own mock is `#[cfg(test)]`-private to the scaffold, so this provider test wires a
    // tiny duplex transport of its own (the public `RealtimeTransport`/`RealtimeTransportFactory` seam).
    // It (a) records EVERY outbound frame the driver sends — so we can assert the user-audio OUT half
    // and the session.config handshake — and (b) emits scripted INBOUND frames — so we can assert the
    // assistant-audio IN half drives `on_audio`. Both halves run on the ONE live socket: full-duplex.

    /// One scripted inbound step.
    #[derive(Clone)]
    enum InStep {
        /// A JSON control frame (text).
        Text(String),
        /// A raw binary audio frame (assistant audio IN).
        Binary(Bytes),
    }

    #[derive(Clone, Default)]
    struct DuplexObs {
        /// Every outbound frame the driver wrote (binary = user audio OUT; text = control).
        sent: Arc<Mutex<Vec<OutFrame>>>,
        connects: Arc<AtomicUsize>,
    }

    impl DuplexObs {
        fn sent_binary(&self) -> Vec<Bytes> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .filter_map(|f| match f {
                    OutFrame::Binary(b) => Some(b.clone()),
                    OutFrame::Text(_) => None,
                })
                .collect()
        }
        fn sent_text(&self) -> Vec<String> {
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
    }

    struct DuplexTransport {
        script: VecDeque<InStep>,
        obs: DuplexObs,
    }

    #[async_trait]
    impl RealtimeTransport for DuplexTransport {
        async fn send(&mut self, frame: OutFrame) -> RealtimeResult<()> {
            self.obs.sent.lock().unwrap().push(frame);
            Ok(())
        }
        async fn recv(&mut self) -> Option<RealtimeResult<OutFrame>> {
            match self.script.pop_front() {
                Some(InStep::Text(s)) => Some(Ok(OutFrame::Text(s))),
                Some(InStep::Binary(b)) => Some(Ok(OutFrame::Binary(b))),
                // Drained → stay open (pending) so the session lives until disconnect — a BOUNDED wait
                // in the test is guaranteed because `disconnect()` aborts the supervisor task.
                None => std::future::pending().await,
            }
        }
    }

    struct DuplexFactory {
        script: Arc<Mutex<VecDeque<InStep>>>,
        obs: DuplexObs,
    }

    #[async_trait]
    impl RealtimeTransportFactory for DuplexFactory {
        async fn connect(
            &self,
            _spec: ConnectSpec,
        ) -> RealtimeResult<Box<dyn RealtimeTransport>> {
            self.obs.connects.fetch_add(1, Ordering::SeqCst);
            let script: VecDeque<InStep> =
                std::mem::take(&mut *self.script.lock().unwrap());
            Ok(Box::new(DuplexTransport { script, obs: self.obs.clone() }))
        }
    }

    /// An [`InferProtocol`] wrapper whose `transport_factory` returns the duplex test-double (so the
    /// REAL driver runs against a scripted, observable socket — end-to-end, no network).
    struct TestInfer {
        inner: InferProtocol,
        script: Arc<Mutex<VecDeque<InStep>>>,
        obs: DuplexObs,
    }

    impl RealtimeProtocol for TestInfer {
        type Wire = InferWire;
        fn from_config(cfg: &RealtimeConfig) -> RealtimeResult<Self> {
            Ok(Self {
                inner: InferProtocol::from_config(cfg)?,
                script: Arc::new(Mutex::new(VecDeque::new())),
                obs: DuplexObs::default(),
            })
        }
        fn transport_factory(&self) -> Arc<dyn RealtimeTransportFactory> {
            Arc::new(DuplexFactory { script: self.script.clone(), obs: self.obs.clone() })
        }
        fn provider_id(&self) -> &'static str {
            self.inner.provider_id()
        }
        fn caps(&self) -> crate::core::realtime::scaffold::ProtocolCaps {
            self.inner.caps()
        }
        fn connect_spec(&self, cfg: &RealtimeConfig) -> RealtimeResult<ConnectSpec> {
            self.inner.connect_spec(cfg)
        }
        fn build_session_config(
            &self,
            cfg: &RealtimeConfig,
            r: Option<&str>,
        ) -> Vec<Self::Wire> {
            self.inner.build_session_config(cfg, r)
        }
        fn map_server_event(
            &self,
            raw: crate::core::realtime::scaffold::Inbound<'_>,
        ) -> Vec<crate::core::realtime::scaffold::S2sEvent> {
            self.inner.map_server_event(raw)
        }
        fn encode_user_audio(&self, pcm: &[u8]) -> Self::Wire {
            self.inner.encode_user_audio(pcm)
        }
        fn send_text(&self, text: &str) -> Vec<Self::Wire> {
            self.inner.send_text(text)
        }
        fn create_response(
            &self,
            o: Option<&RealtimeResponseOverride>,
        ) -> Vec<Self::Wire> {
            self.inner.create_response(o)
        }
        fn commit_turn(&self) -> Vec<Self::Wire> {
            self.inner.commit_turn()
        }
        fn cancel_response(&self) -> Vec<Self::Wire> {
            self.inner.cancel_response()
        }
        fn format_tool_result(&self, c: &str, r: &str) -> Vec<Self::Wire> {
            self.inner.format_tool_result(c, r)
        }
        fn serialize(&self, msg: &Self::Wire) -> RealtimeResult<OutFrame> {
            self.inner.serialize(msg)
        }
    }

    /// **RED→GREEN `infer_protocol_bidir_audio`** (§6, §13 GW-9 accept): the native S2S InferProtocol,
    /// driven END-TO-END through the REAL [`RealtimeSession`] driver, carries **full-duplex
    /// bidirectional audio** on ONE live socket. We script the assistant-audio IN half (raw binary
    /// frames the model emits) and assert:
    /// 1. on connect, the `session.config{task:s2s}` handshake went OUT (the duplex is set up);
    /// 2. INBOUND assistant audio drives `on_audio` byte-exact (the IN half);
    /// 3. WHILE the inbound stream is live, `send_audio` puts user PCM OUT byte-exact (the OUT half) —
    ///    the two directions flow CONCURRENTLY (full-duplex, not a half-duplex turn machine);
    /// 4. `emits_user_turn_frames()` is true (the gateway gets out of the way — §6.1/§6.4).
    /// The whole body is bounded by a tokio timeout so a deadlock FAILS the test instead of hanging.
    #[tokio::test]
    async fn infer_protocol_bidir_audio() {
        tokio::time::timeout(Duration::from_secs(20), async {
            // The interaction model emits a ready ack, then streams assistant audio IN (two chunks),
            // interleaved with a transcript — the inbound half of the duplex.
            let asst_a = Bytes::from(vec![0x00, 0x40, 0x01, 0x80]); // assistant audio chunk #1
            let asst_b = Bytes::from(vec![0x10, 0x20, 0x30, 0x40]); // assistant audio chunk #2
            let script = vec![
                InStep::Text(r#"{"type":"ready","session_id":"s_1"}"#.to_string()),
                InStep::Binary(asst_a.clone()),
                InStep::Text(
                    r#"{"type":"transcript","role":"assistant","text":"hi there","is_final":true}"#
                        .to_string(),
                ),
                InStep::Binary(asst_b.clone()),
            ];

            let proto = TestInfer::from_config(&cfg()).unwrap();
            // capture the observer + script BEFORE moving the protocol into the session.
            let obs = proto.obs.clone();
            *proto.script.lock().unwrap() = script.into();

            let mut session = RealtimeSession::from_parts(proto, cfg()).unwrap();

            // collectors for the inbound (assistant-audio IN) half.
            let audio_in: Arc<TMutex<Vec<Bytes>>> = Arc::new(TMutex::new(Vec::new()));
            {
                let sink = audio_in.clone();
                session
                    .on_audio(Arc::new(move |a| {
                        let sink = sink.clone();
                        Box::pin(async move { sink.lock().await.push(a.data) })
                    }))
                    .unwrap();
            }
            let transcripts: Arc<TMutex<Vec<String>>> = Arc::new(TMutex::new(Vec::new()));
            {
                let sink = transcripts.clone();
                session
                    .on_transcript(Arc::new(move |t| {
                        let sink = sink.clone();
                        Box::pin(async move { sink.lock().await.push(t.text) })
                    }))
                    .unwrap();
            }

            // ── connect: the driver dials the duplex socket + sends the session.config handshake OUT ──
            session.connect().await.unwrap();
            assert!(session.is_ready(), "ready immediately after connect()");
            // (1) the handshake went OUT — the s2s duplex is set up.
            until(|| obs.sent_text().iter().any(|t| t.contains("\"task\":\"s2s\""))).await;

            // (2) INBOUND assistant audio drives on_audio byte-exact (the IN half) — both chunks arrive.
            until(|| {
                let got = audio_in.try_lock().map(|g| g.len()).unwrap_or(0);
                got >= 2
            })
            .await;
            {
                let got = audio_in.lock().await;
                assert_eq!(got.as_slice(), &[asst_a.clone(), asst_b.clone()], "assistant audio IN is byte-exact, in order");
            }
            // the interleaved assistant transcript also arrived (the model transcribes what it spoke).
            until(|| {
                transcripts.try_lock().map(|g| g.iter().any(|t| t == "hi there")).unwrap_or(false)
            })
            .await;

            // (3) THE DUPLEX KEYSTONE: WHILE the inbound stream is live (the socket is still open and
            // pending more inbound), put USER audio OUT — concurrently, no turn boundary. It rides the
            // wire byte-exact as a raw binary frame.
            let user_pcm = Bytes::from(vec![0xde, 0xad, 0xbe, 0xef]);
            session.send_audio(user_pcm.clone()).await.unwrap();
            let user2 = Bytes::from(vec![0xca, 0xfe]);
            session.send_audio(user2.clone()).await.unwrap();
            until(|| {
                let b = obs.sent_binary();
                b.contains(&user_pcm) && b.contains(&user2)
            })
            .await;
            // the OUT binary frames are EXACTLY the user PCM (byte-exact, no base64) — the OUT half.
            let out = obs.sent_binary();
            assert!(out.contains(&user_pcm), "user audio OUT #1 byte-exact");
            assert!(out.contains(&user2), "user audio OUT #2 byte-exact");

            // (4) the gateway got out of the way: the model owns turns.
            assert!(session.emits_user_turn_frames(), "S2S model owns turn-taking (§6.1/§6.4)");

            session.disconnect().await.unwrap();
        })
        .await
        .expect("the full-duplex bidir-audio test must complete within the bound (no deadlock)");
    }

    /// **RED→GREEN `uds_carries_s2s_frames`** (GW-13 UDS half, §6,§13): native S2S
    /// connects over a **unix domain socket** and carries the full-duplex frame
    /// vocabulary in BOTH directions over the in-box UDS — the literal accept
    /// criterion ("S2S connects over a unix domain socket"). We:
    ///   1. point `InferProtocol::connect_spec` at a live UDS via a SERVER-CONFIG
    ///      `unix://…` `realtime_endpoint_override` (so it yields `ConnectSpec::Unix`);
    ///   2. drive the REAL [`RealtimeSession<InferProtocol>`] through its DEFAULT
    ///      (now UDS-capable) transport factory over that socket;
    ///   3. assert the `session.config{task:s2s}` handshake went OUT over the UDS,
    ///      and a raw-binary assistant-audio frame came IN byte-exact (`on_audio`).
    /// Bounded by a tokio timeout so a deadlock FAILS instead of hanging.
    #[tokio::test]
    async fn uds_carries_s2s_frames() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tokio::time::timeout(Duration::from_secs(20), async {
            // A unique UDS path for this test process.
            let path = std::env::temp_dir()
                .join(format!("waav_infer_s2s_{}.sock", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let listener = tokio::net::UnixListener::bind(&path).expect("bind UDS");

            // The in-box "Infer S2S" server: accept ONE connection; read the first
            // OUTBOUND kind+len-prefixed frame (the session.config handshake); reply
            // with a `ready` control frame then a raw-binary assistant-audio frame.
            let asst = [0x00u8, 0x40, 0x01, 0x80];
            let captured: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
            let cap2 = captured.clone();
            let server = tokio::spawn(async move {
                let (mut sock, _) = listener.accept().await.expect("accept");
                // Read the first frame (kind + u32 len + body).
                let mut kind = [0u8; 1];
                sock.read_exact(&mut kind).await.expect("kind");
                let mut len_buf = [0u8; 4];
                sock.read_exact(&mut len_buf).await.expect("len");
                let n = u32::from_be_bytes(len_buf) as usize;
                let mut body = vec![0u8; n];
                sock.read_exact(&mut body).await.expect("body");
                *cap2.lock().unwrap() = Some(String::from_utf8_lossy(&body).to_string());

                // Reply: a `ready` text control frame (kind 0).
                let ready = br#"{"type":"ready","session_id":"s_uds"}"#;
                sock.write_all(&[0u8]).await.unwrap();
                sock.write_all(&(ready.len() as u32).to_be_bytes()).await.unwrap();
                sock.write_all(ready).await.unwrap();
                // Then a raw-binary assistant-audio frame (kind 1) byte-exact.
                sock.write_all(&[1u8]).await.unwrap();
                sock.write_all(&(asst.len() as u32).to_be_bytes()).await.unwrap();
                sock.write_all(&asst).await.unwrap();
                sock.flush().await.unwrap();
                // Keep the socket open so the client's recv loop stays live until
                // the test disconnects (a BOUNDED wait — the test aborts it).
                tokio::time::sleep(Duration::from_secs(10)).await;
            });

            // Point the infer protocol at the UDS via a SERVER-CONFIG override.
            let mut over = cfg();
            over.realtime_endpoint_override =
                Some(format!("unix://{}", path.to_string_lossy()));

            // Drive the REAL session over its DEFAULT (UDS-capable) transport.
            let mut session = InferRealtime::new(over.clone()).unwrap();
            let audio_in: Arc<TMutex<Vec<Bytes>>> = Arc::new(TMutex::new(Vec::new()));
            {
                let sink = audio_in.clone();
                session
                    .on_audio(Arc::new(move |a| {
                        let sink = sink.clone();
                        Box::pin(async move { sink.lock().await.push(a.data) })
                    }))
                    .unwrap();
            }

            session.connect().await.expect("S2S connects over the UDS");
            assert!(session.is_ready(), "ready after the UDS handshake ack");

            // The session.config{task:s2s} handshake went OUT over the UDS.
            until(|| captured.lock().unwrap().is_some()).await;
            let got = captured.lock().unwrap().clone().unwrap();
            assert!(
                got.contains("\"task\":\"s2s\""),
                "session.config{{task:s2s}} rode the UDS OUT: {got}"
            );

            // The assistant-audio binary frame arrived IN byte-exact (the IN half).
            until(|| audio_in.try_lock().map(|g| !g.is_empty()).unwrap_or(false)).await;
            assert_eq!(
                audio_in.lock().await.as_slice(),
                &[Bytes::copy_from_slice(&asst)],
                "assistant audio IN over UDS is byte-exact"
            );

            session.disconnect().await.unwrap();
            server.abort();
            let _ = std::fs::remove_file(&path);
        })
        .await
        .expect("the UDS S2S test must complete within the bound (no deadlock)");
    }

    /// **`uds_carries_s2s_frames_concurrent`** (multi-tenant invariant, ≥4 concurrent): FOUR independent
    /// native-S2S sessions, each over its OWN unix domain socket, all driven CONCURRENTLY. Each tenant's
    /// server replies with a DISTINCT audio payload; the test asserts every session receives ONLY its own
    /// audio (no cross-talk between the per-session UDS transports — masked ≠ absent / per-slot isolation).
    #[tokio::test]
    async fn uds_carries_s2s_frames_concurrent() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        tokio::time::timeout(Duration::from_secs(30), async {
            const N: usize = 4;
            let mut handles = Vec::new();

            for tenant in 0..N {
                handles.push(tokio::spawn(async move {
                    let path = std::env::temp_dir().join(format!(
                        "waav_infer_mt_{}_{}.sock",
                        std::process::id(),
                        tenant
                    ));
                    let _ = std::fs::remove_file(&path);
                    let listener = tokio::net::UnixListener::bind(&path).expect("bind UDS");

                    // This tenant's UNIQUE audio payload (so cross-talk would be detectable).
                    let asst = [tenant as u8, 0xA0, 0x0A, tenant as u8];
                    let srv_path = path.clone();
                    let server = tokio::spawn(async move {
                        let (mut sock, _) = listener.accept().await.expect("accept");
                        let mut kind = [0u8; 1];
                        sock.read_exact(&mut kind).await.unwrap();
                        let mut len_buf = [0u8; 4];
                        sock.read_exact(&mut len_buf).await.unwrap();
                        let n = u32::from_be_bytes(len_buf) as usize;
                        let mut body = vec![0u8; n];
                        sock.read_exact(&mut body).await.unwrap();
                        // ready ack + this tenant's distinct audio frame.
                        let ready = br#"{"type":"ready"}"#;
                        sock.write_all(&[0u8]).await.unwrap();
                        sock.write_all(&(ready.len() as u32).to_be_bytes()).await.unwrap();
                        sock.write_all(ready).await.unwrap();
                        sock.write_all(&[1u8]).await.unwrap();
                        sock.write_all(&(asst.len() as u32).to_be_bytes()).await.unwrap();
                        sock.write_all(&asst).await.unwrap();
                        sock.flush().await.unwrap();
                        tokio::time::sleep(Duration::from_secs(15)).await;
                        let _ = std::fs::remove_file(&srv_path);
                    });

                    let mut over = cfg();
                    over.realtime_endpoint_override = Some(format!("unix://{}", path.to_string_lossy()));
                    let mut session = InferRealtime::new(over).unwrap();
                    let got: Arc<TMutex<Vec<Bytes>>> = Arc::new(TMutex::new(Vec::new()));
                    {
                        let sink = got.clone();
                        session
                            .on_audio(Arc::new(move |a| {
                                let sink = sink.clone();
                                Box::pin(async move { sink.lock().await.push(a.data) })
                            }))
                            .unwrap();
                    }
                    session.connect().await.expect("tenant S2S connects over its UDS");

                    // bounded wait for this tenant's one audio frame.
                    for _ in 0..400 {
                        if got.try_lock().map(|g| !g.is_empty()).unwrap_or(false) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(2)).await;
                    }
                    let received = got.lock().await.clone();
                    session.disconnect().await.unwrap();
                    server.abort();
                    (tenant, received, Bytes::copy_from_slice(&asst))
                }));
            }

            // Every tenant got EXACTLY its own audio — no cross-talk between per-session UDS transports.
            for h in handles {
                let (tenant, received, expected) = h.await.expect("tenant task joined");
                assert_eq!(
                    received.as_slice(),
                    &[expected],
                    "tenant {tenant} received only its own audio over its own UDS (no cross-talk)"
                );
            }
        })
        .await
        .expect("the concurrent multi-tenant UDS test must complete within the bound (no deadlock)");
    }

    // ── the InferRealtime newtype surface ──

    #[tokio::test]
    async fn creates_and_reports_provider() {
        let rt = InferRealtime::new(cfg()).unwrap();
        assert!(!rt.is_ready());
        assert_eq!(rt.get_provider_info()["provider"], "waav-infer");
        assert_eq!(rt.get_provider_info()["features"]["full_duplex_audio"], true);
        // the model owns turns ⇒ the cascade Smart-Turn is bypassed (§6.1).
        assert!(rt.emits_user_turn_frames());
    }

    #[tokio::test]
    async fn requires_model_id() {
        let bad = RealtimeConfig { model: String::new(), ..cfg() };
        assert!(matches!(
            InferRealtime::new(bad),
            Err(RealtimeError::InvalidConfiguration(_))
        ));
    }
}
