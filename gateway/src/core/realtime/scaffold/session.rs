//! `RealtimeSession<P>` — the generic S2S driver. Implements
//! [`BaseRealtime`](super::super::base::BaseRealtime) ONCE for every provider:
//! the reconnect supervisor (backoff + quick-failure cutoff + breaker/governor),
//! the receive loop, `S2sEvent` → callback dispatch, barge-in/truncate/preroll,
//! conversation replay, and resilience — all the machinery that today lives
//! tangled (and 4×-duplicated) in `openai/client.rs`.
//!
//! A provider is then a thin newtype: `OpenAIRealtime(RealtimeSession<OpenAiProtocol>)`.

use async_trait::async_trait;
use bytes::Bytes;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::task::JoinHandle;

use super::super::base::{
    clamp_truncate_ms, AudioOutputCallback, BaseRealtime, ConnectionState, FunctionCallCallback,
    RealtimeAudioData, RealtimeConfig, RealtimeError, RealtimeErrorCallback, RealtimeResponseOverride,
    RealtimeResult, ReconnectionCallback, ReconnectionConfig, ReconnectionEvent,
    ReplayConversationItem, ResponseDoneCallback, SpeechEventCallback, TranscriptCallback,
    TranscriptResult, TranscriptRole,
};
use super::event::{OutFrame, ProtocolCaps, S2sEvent};
use super::protocol::RealtimeProtocol;
use super::transport::RealtimeTransportFactory;
use crate::core::resilience::ResilienceHandles;
use crate::core::websocket::audio_replay::AudioReplayBuffer;

/// Cap on the replayed-on-reconnect conversation log (B-G2): a long session must
/// not grow it unbounded.
const MAX_REPLAY_LOG_ITEMS: usize = 100;
/// Outbound wire channel capacity.
const OUT_CHANNEL_CAPACITY: usize = 256;
/// ~660ms of 24k PCM16 preroll — covers VAD onset latency.
const PREROLL_CAP_BYTES: usize = 32_000;
/// Quick-failure cutoff: a connection that drops within this window after a
/// successful open is "unstable" (bad key / policy close); too many in a row
/// stops the storm instead of hammering.
const MIN_STABLE_CONNECTION: Duration = Duration::from_secs(5);
const MAX_CONSECUTIVE_QUICK_FAILURES: u32 = 3;

/// Playback position of the currently-streaming assistant item (for truncate).
/// `first_delta` anchors WALL-CLOCK elapsed so a barge-in truncates to what the
/// user ACTUALLY heard — `min(elapsed, received)` — never beyond received audio
/// (the OpenAI API 400s on over-truncate). Matches `openai/client.rs`.
struct ItemPlayback {
    item_id: String,
    first_delta: Instant,
    duration_ms: u64,
}

/// The 6 shared callbacks + the bookkeeping the receive loop drives. Cloned (Arc)
/// into the supervisor task.
#[derive(Clone)]
struct DispatchCtx {
    transcript_cb: Arc<Mutex<Option<TranscriptCallback>>>,
    audio_cb: Arc<Mutex<Option<AudioOutputCallback>>>,
    error_cb: Arc<Mutex<Option<RealtimeErrorCallback>>>,
    function_call_cb: Arc<Mutex<Option<FunctionCallCallback>>>,
    speech_event_cb: Arc<Mutex<Option<SpeechEventCallback>>>,
    response_done_cb: Arc<Mutex<Option<ResponseDoneCallback>>>,
    session_id: Arc<RwLock<Option<String>>>,
    assistant_accum: Arc<RwLock<String>>,
    user_accum: Arc<RwLock<String>>,
    pending_calls: Arc<RwLock<HashMap<String, String>>>,
    playback: Arc<StdMutex<Option<ItemPlayback>>>,
    conversation_log: Arc<RwLock<Vec<ReplayConversationItem>>>,
    resumption: Arc<RwLock<Option<String>>>,
    caps: ProtocolCaps,
}

impl DispatchCtx {
    /// Turn one normalized event into callback + state effects. The ONE place all
    /// the load-bearing bookkeeping lives (deduplicated from the bespoke clients).
    async fn dispatch(&self, ev: S2sEvent) {
        match ev {
            S2sEvent::SessionReady { session_id } => {
                if session_id.is_some() {
                    *self.session_id.write().await = session_id;
                }
            }
            S2sEvent::Transcript {
                role,
                text,
                is_final,
                item_id,
            } => {
                let delivered = if is_final {
                    // Reset the running accumulator for this role; log the final.
                    match role {
                        TranscriptRole::Assistant => *self.assistant_accum.write().await = String::new(),
                        TranscriptRole::User => *self.user_accum.write().await = String::new(),
                    }
                    let mut log = self.conversation_log.write().await;
                    log.push(ReplayConversationItem {
                        role,
                        text: text.clone(),
                    });
                    let len = log.len();
                    if len > MAX_REPLAY_LOG_ITEMS {
                        log.drain(0..len - MAX_REPLAY_LOG_ITEMS);
                    }
                    text
                } else {
                    // Interim: accumulate, deliver the running text.
                    let accum = match role {
                        TranscriptRole::Assistant => &self.assistant_accum,
                        TranscriptRole::User => &self.user_accum,
                    };
                    let mut g = accum.write().await;
                    g.push_str(&text);
                    g.clone()
                };
                let cb = self.transcript_cb.lock().await.clone();
                if let Some(cb) = cb {
                    cb(TranscriptResult {
                        text: delivered,
                        role,
                        is_final,
                        item_id,
                    })
                    .await;
                }
            }
            S2sEvent::Audio {
                data,
                item_id,
                response_id,
            } => {
                // Playback bookkeeping for truncate (bytes→ms via caps).
                {
                    let chunk_ms = if self.caps.output_bytes_per_ms > 0 {
                        data.len() as u64 / self.caps.output_bytes_per_ms
                    } else {
                        0
                    };
                    let mut pb = self.playback.lock().unwrap();
                    match pb.as_mut() {
                        Some(p) if item_id.as_deref() == Some(p.item_id.as_str()) => {
                            p.duration_ms += chunk_ms;
                        }
                        _ => {
                            *pb = Some(ItemPlayback {
                                item_id: item_id.clone().unwrap_or_default(),
                                first_delta: Instant::now(),
                                duration_ms: chunk_ms,
                            });
                        }
                    }
                }
                let cb = self.audio_cb.lock().await.clone();
                if let Some(cb) = cb {
                    cb(RealtimeAudioData {
                        data,
                        sample_rate: self.caps.output_sample_rate,
                        item_id,
                        response_id,
                    })
                    .await;
                }
            }
            S2sEvent::Speech(ev) => {
                let cb = self.speech_event_cb.lock().await.clone();
                if let Some(cb) = cb {
                    cb(ev).await;
                }
            }
            S2sEvent::FunctionCall(mut req) => {
                if req.name.is_empty() {
                    if let Some(name) = self.pending_calls.read().await.get(&req.call_id) {
                        req.name = name.clone();
                    }
                }
                let cb = self.function_call_cb.lock().await.clone();
                if let Some(cb) = cb {
                    cb(req).await;
                }
            }
            S2sEvent::TrackPendingCall { call_id, name } => {
                self.pending_calls.write().await.insert(call_id, name);
            }
            S2sEvent::ResponseDone { response_id } => {
                let cb = self.response_done_cb.lock().await.clone();
                if let Some(cb) = cb {
                    cb(response_id).await;
                }
            }
            S2sEvent::InterruptedByServer => {
                // The server already stopped its own output; clear local playback
                // so a follow-up truncate doesn't double-count.
                *self.playback.lock().unwrap() = None;
            }
            S2sEvent::ResumptionHandle(h) => {
                *self.resumption.write().await = Some(h);
            }
            S2sEvent::Error(e) => {
                let cb = self.error_cb.lock().await.clone();
                if let Some(cb) = cb {
                    cb(e).await;
                }
            }
            S2sEvent::Ignore => {}
        }
    }
}

/// The generic realtime driver. `P` is the per-provider [`RealtimeProtocol`].
pub struct RealtimeSession<P: RealtimeProtocol> {
    protocol: Arc<P>,
    factory: Arc<dyn RealtimeTransportFactory>,
    config: RealtimeConfig,
    caps: ProtocolCaps,
    state: Arc<StdRwLock<ConnectionState>>,
    connected: Arc<AtomicBool>,
    intentional_disconnect: Arc<AtomicBool>,
    out_tx: Arc<Mutex<Option<mpsc::Sender<OutFrame>>>>,
    connection_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
    reconnect_cfg: ReconnectionConfig,
    resilience: Option<ResilienceHandles>,
    // shared bookkeeping (also captured by the supervisor task)
    cb: DispatchCtx,
    reconnection_cb: Arc<Mutex<Option<ReconnectionCallback>>>,
    preroll: Arc<AudioReplayBuffer>,
}

impl<P: RealtimeProtocol> RealtimeSession<P> {
    /// Build a session from a protocol + config (the provider newtype calls this
    /// from `BaseRealtime::new`).
    pub fn from_parts(protocol: P, config: RealtimeConfig) -> RealtimeResult<Self> {
        let caps = protocol.caps();
        let factory = protocol.transport_factory();
        let reconnect_cfg = config.reconnection.clone().unwrap_or_default();
        Ok(Self {
            protocol: Arc::new(protocol),
            factory,
            config,
            caps,
            state: Arc::new(StdRwLock::new(ConnectionState::Disconnected)),
            connected: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            out_tx: Arc::new(Mutex::new(None)),
            connection_handle: Arc::new(Mutex::new(None)),
            reconnect_cfg,
            resilience: None,
            cb: DispatchCtx {
                transcript_cb: Arc::new(Mutex::new(None)),
                audio_cb: Arc::new(Mutex::new(None)),
                error_cb: Arc::new(Mutex::new(None)),
                function_call_cb: Arc::new(Mutex::new(None)),
                speech_event_cb: Arc::new(Mutex::new(None)),
                response_done_cb: Arc::new(Mutex::new(None)),
                session_id: Arc::new(RwLock::new(None)),
                assistant_accum: Arc::new(RwLock::new(String::new())),
                user_accum: Arc::new(RwLock::new(String::new())),
                pending_calls: Arc::new(RwLock::new(HashMap::new())),
                playback: Arc::new(StdMutex::new(None)),
                conversation_log: Arc::new(RwLock::new(Vec::new())),
                resumption: Arc::new(RwLock::new(None)),
                caps,
            },
            reconnection_cb: Arc::new(Mutex::new(None)),
            preroll: Arc::new(AudioReplayBuffer::new(PREROLL_CAP_BYTES)),
        })
    }

    fn set_state(&self, s: ConnectionState) {
        *self.state.write().unwrap() = s;
    }

    /// Borrow the per-provider protocol (lets a provider newtype expose its own
    /// inherent accessors — e.g. `OpenAIRealtime::model()` — by delegating).
    pub(crate) fn protocol(&self) -> &P {
        &self.protocol
    }

    /// Borrow the session's config (lets a provider newtype delegate config-derived
    /// inherent methods — e.g. `build_session_config()` / reconnection accessors).
    pub(crate) fn config(&self) -> &RealtimeConfig {
        &self.config
    }

    /// The shared circuit breaker this session feeds, if injected (W-D2 metrics /
    /// tests). Mirrors the bespoke `resilience_breaker()` accessor.
    pub(crate) fn resilience_breaker(
        &self,
    ) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }

    /// Read the current session id (post-`session.created`).
    pub(crate) async fn session_id(&self) -> Option<String> {
        self.cb.session_id.read().await.clone()
    }

    /// Serialize one wire message and queue it for the supervisor to send.
    /// Fails fast when not connected (e.g. mid-reconnect) rather than silently
    /// buffering frames the live socket will never carry (review finding #6).
    async fn push_wire(&self, wire: P::Wire) -> RealtimeResult<()> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(RealtimeError::NotConnected);
        }
        let frame = self.protocol.serialize(&wire)?;
        let guard = self.out_tx.lock().await;
        let tx = guard.as_ref().ok_or(RealtimeError::NotConnected)?;
        tx.send(frame)
            .await
            .map_err(|_| RealtimeError::NotConnected)
    }

    async fn push_wires(&self, wires: Vec<P::Wire>) -> RealtimeResult<()> {
        for w in wires {
            self.push_wire(w).await?;
        }
        Ok(())
    }
}

impl<P: RealtimeProtocol> RealtimeSession<P> {
    /// The supervisor task: outer reconnect loop + inner select. Mirrors the
    /// proven `openai/client.rs` loop, generic over `P` + transport.
    #[allow(clippy::too_many_arguments)]
    async fn supervisor(
        protocol: Arc<P>,
        factory: Arc<dyn RealtimeTransportFactory>,
        config: RealtimeConfig,
        mut out_rx: mpsc::Receiver<OutFrame>,
        connected: Arc<AtomicBool>,
        state: Arc<StdRwLock<ConnectionState>>,
        intentional_disconnect: Arc<AtomicBool>,
        reconnect_cfg: ReconnectionConfig,
        resilience: Option<ResilienceHandles>,
        ctx: DispatchCtx,
        reconnection_cb: Arc<Mutex<Option<ReconnectionCallback>>>,
        ready_tx: Option<tokio::sync::oneshot::Sender<RealtimeResult<()>>>,
    ) {
        let provider_id = protocol.provider_id();
        let mut attempt: u32 = 0;
        let mut quick_failures: u32 = 0;
        let mut is_reconnect = false;
        // Resolves `connect()` on the FIRST successful connect (or a terminal
        // failure before ever connecting) — the sync-readiness contract the
        // consumer relies on (review finding #1).
        let mut ready_tx = ready_tx;

        'outer: loop {
            // ── (re)connect ──
            let spec = match protocol.connect_spec(&config) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(provider = provider_id, error = %e, "connect_spec failed");
                    Self::signal_ready(&mut ready_tx, Err(RealtimeError::ConnectionFailed(e.to_string())));
                    break 'outer;
                }
            };

            // Resilience participates ONLY on RECONNECT dials (matches the proven
            // OpenAI loop): a NEW session must not be deferred by an open breaker
            // nor consume a governor permit (review finding #3).
            if is_reconnect {
                if let Some(r) = &resilience {
                    if !r.breaker.allow_request() {
                        tracing::warn!(provider = provider_id, "circuit breaker open; deferring dial");
                        crate::core::metrics::bridge::record_reconnect(provider_id, "circuit_open");
                        if !Self::should_continue(&intentional_disconnect, &reconnect_cfg, attempt) {
                            break 'outer;
                        }
                        attempt += 1;
                        tokio::time::sleep(Duration::from_millis(
                            reconnect_cfg.calculate_delay(attempt),
                        ))
                        .await;
                        continue 'outer;
                    }
                }
            }

            let resumption = ctx.resumption.read().await.clone();
            // The governor permit is scoped to the DIAL ONLY (not the backoff
            // sleep), and only on reconnects (review findings #2, #3).
            let dial = {
                let _permit = if is_reconnect {
                    match &resilience {
                        Some(r) => Some(r.governor.acquire().await),
                        None => None,
                    }
                } else {
                    None
                };
                factory.connect(spec).await
            };
            let mut transport = match dial {
                Ok(t) => t,
                Err(e) => {
                    if is_reconnect {
                        if let Some(r) = &resilience {
                            r.breaker.record_failure();
                        }
                    }
                    tracing::warn!(provider = provider_id, error = %e, attempt, "dial failed");
                    if !Self::should_continue(&intentional_disconnect, &reconnect_cfg, attempt) {
                        Self::signal_ready(&mut ready_tx, Err(RealtimeError::ConnectionFailed(e.to_string())));
                        Self::notify_reconnect(&reconnection_cb, attempt, false, Some(e.to_string())).await;
                        break 'outer;
                    }
                    attempt += 1;
                    tokio::time::sleep(Duration::from_millis(reconnect_cfg.calculate_delay(attempt))).await;
                    continue 'outer;
                }
            };
            let connected_at = Instant::now();

            // Send session config; replay context on reconnect.
            for w in protocol.build_session_config(&config, resumption.as_deref()) {
                if let Ok(f) = protocol.serialize(&w) {
                    let _ = transport.send(f).await;
                }
            }
            if is_reconnect {
                let log = ctx.conversation_log.read().await.clone();
                for item in &log {
                    for w in protocol.replay_item(item) {
                        if let Ok(f) = protocol.serialize(&w) {
                            let _ = transport.send(f).await;
                        }
                    }
                }
                if let Some(r) = &resilience {
                    r.breaker.record_success();
                }
                Self::notify_reconnect(&reconnection_cb, attempt, true, None).await;
            }
            crate::core::metrics::bridge::record_reconnect(provider_id, "connected");
            connected.store(true, Ordering::SeqCst);
            *state.write().unwrap() = ConnectionState::Connected;
            // First connect succeeded → unblock connect().
            Self::signal_ready(&mut ready_tx, Ok(()));
            attempt = 0;

            // ── inner loop: pump outbound + dispatch inbound ──
            loop {
                tokio::select! {
                    out = out_rx.recv() => match out {
                        Some(frame) => {
                            if transport.send(frame).await.is_err() {
                                break;
                            }
                        }
                        None => {
                            // All senders dropped → session dropped → stop.
                            intentional_disconnect.store(true, Ordering::SeqCst);
                            break;
                        }
                    },
                    inbound = transport.recv() => match inbound {
                        Some(Ok(frame)) => {
                            for ev in protocol.map_server_event(frame.as_inbound()) {
                                // A SERVER-side barge-in must cancel + truncate
                                // server state to what the user actually heard
                                // (review finding #5). The supervisor — not the
                                // dispatcher — does this because only it holds the
                                // protocol + transport.
                                if matches!(ev, S2sEvent::InterruptedByServer) {
                                    let target = {
                                        let mut pb = ctx.playback.lock().unwrap();
                                        pb.take().map(|p| {
                                            let elapsed = p.first_delta.elapsed().as_millis() as u64;
                                            (p.item_id, clamp_truncate_ms(elapsed, p.duration_ms))
                                        })
                                    };
                                    for w in protocol.cancel_response() {
                                        if let Ok(f) = protocol.serialize(&w) {
                                            let _ = transport.send(f).await;
                                        }
                                    }
                                    if let Some((item, end)) = target {
                                        for w in protocol.truncate(&item, end) {
                                            if let Ok(f) = protocol.serialize(&w) {
                                                let _ = transport.send(f).await;
                                            }
                                        }
                                    }
                                } else {
                                    ctx.dispatch(ev).await;
                                }
                            }
                        }
                        Some(Err(e)) => {
                            tracing::warn!(provider = provider_id, error = %e, "transport error");
                            break;
                        }
                        None => break, // clean close
                    },
                }
            }

            // ── connection ended ──
            connected.store(false, Ordering::SeqCst);
            transport.close().await;

            if intentional_disconnect.load(Ordering::SeqCst) {
                *state.write().unwrap() = ConnectionState::Disconnected;
                break 'outer;
            }

            // Quick-failure cutoff (handshake ok then immediate close = bad).
            if connected_at.elapsed() < MIN_STABLE_CONNECTION {
                quick_failures += 1;
                if quick_failures >= MAX_CONSECUTIVE_QUICK_FAILURES {
                    tracing::error!(provider = provider_id, "too many quick failures; giving up");
                    *state.write().unwrap() = ConnectionState::Failed;
                    Self::notify_reconnect(&reconnection_cb, attempt, false, Some("quick-failure cutoff".into())).await;
                    break 'outer;
                }
            } else {
                quick_failures = 0;
            }

            if !Self::should_continue(&intentional_disconnect, &reconnect_cfg, attempt) {
                *state.write().unwrap() = ConnectionState::Failed;
                Self::notify_reconnect(&reconnection_cb, attempt, false, None).await;
                break 'outer;
            }
            *state.write().unwrap() = ConnectionState::Reconnecting;
            attempt += 1;
            is_reconnect = true;
            tokio::time::sleep(Duration::from_millis(reconnect_cfg.calculate_delay(attempt))).await;
        }
    }

    /// Resolve the one-shot readiness signal exactly once (first connect or
    /// terminal pre-connect failure).
    fn signal_ready(
        tx: &mut Option<tokio::sync::oneshot::Sender<RealtimeResult<()>>>,
        result: RealtimeResult<()>,
    ) {
        if let Some(s) = tx.take() {
            let _ = s.send(result);
        }
    }

    fn should_continue(
        intentional: &AtomicBool,
        cfg: &ReconnectionConfig,
        attempt: u32,
    ) -> bool {
        !intentional.load(Ordering::SeqCst) && cfg.should_retry(attempt)
    }

    async fn notify_reconnect(
        cb: &Arc<Mutex<Option<ReconnectionCallback>>>,
        attempt: u32,
        success: bool,
        error: Option<String>,
    ) {
        if let Some(c) = cb.lock().await.as_ref() {
            c(ReconnectionEvent {
                attempt,
                success,
                error,
            })
            .await;
        }
    }
}

#[async_trait]
impl<P: RealtimeProtocol> BaseRealtime for RealtimeSession<P> {
    fn new(config: RealtimeConfig) -> RealtimeResult<Self> {
        let protocol = P::from_config(&config)?;
        Self::from_parts(protocol, config)
    }

    async fn connect(&mut self) -> RealtimeResult<()> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.intentional_disconnect.store(false, Ordering::SeqCst);
        self.set_state(ConnectionState::Connecting);

        let (tx, rx) = mpsc::channel::<OutFrame>(OUT_CHANNEL_CAPACITY);
        *self.out_tx.lock().await = Some(tx);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

        let handle = tokio::spawn(Self::supervisor(
            self.protocol.clone(),
            self.factory.clone(),
            self.config.clone(),
            rx,
            self.connected.clone(),
            self.state.clone(),
            self.intentional_disconnect.clone(),
            self.reconnect_cfg.clone(),
            self.resilience.clone(),
            self.cb.clone(),
            self.reconnection_cb.clone(),
            Some(ready_tx),
        ));
        *self.connection_handle.lock().await = Some(handle);
        // Block until the first connect succeeds (or terminally fails) so a caller
        // that sends audio immediately after `connect().await` sees `is_ready()`
        // (the sync-readiness contract — review finding #1).
        match ready_rx.await {
            Ok(result) => result,
            Err(_) => Err(RealtimeError::ConnectionFailed(
                "supervisor exited before connecting".into(),
            )),
        }
    }

    async fn disconnect(&mut self) -> RealtimeResult<()> {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        *self.out_tx.lock().await = None; // drop sender → supervisor stops
        if let Some(h) = self.connection_handle.lock().await.take() {
            h.abort();
        }
        self.set_state(ConnectionState::Disconnected);
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn get_connection_state(&self) -> ConnectionState {
        *self.state.read().unwrap()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> RealtimeResult<()> {
        self.preroll.push(audio_data.clone());
        self.push_wire(self.protocol.encode_user_audio(&audio_data)).await
    }

    async fn send_text(&mut self, text: &str) -> RealtimeResult<()> {
        self.push_wires(self.protocol.send_text(text)).await
    }

    async fn create_response(&mut self) -> RealtimeResult<()> {
        self.push_wires(self.protocol.create_response(None)).await
    }

    async fn create_response_with(
        &mut self,
        overrides: RealtimeResponseOverride,
    ) -> RealtimeResult<()> {
        self.push_wires(self.protocol.create_response(Some(&overrides))).await
    }

    async fn cancel_response(&mut self) -> RealtimeResult<()> {
        self.push_wires(self.protocol.cancel_response()).await
    }

    async fn commit_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.push_wires(self.protocol.commit_turn()).await
    }

    async fn clear_audio_buffer(&mut self) -> RealtimeResult<()> {
        self.push_wires(self.protocol.clear_input_buffer()).await
    }

    fn on_transcript(&mut self, callback: TranscriptCallback) -> RealtimeResult<()> {
        if let Ok(mut g) = self.cb.transcript_cb.try_lock() {
            *g = Some(callback);
        }
        Ok(())
    }
    fn on_audio(&mut self, callback: AudioOutputCallback) -> RealtimeResult<()> {
        if let Ok(mut g) = self.cb.audio_cb.try_lock() {
            *g = Some(callback);
        }
        Ok(())
    }
    fn on_error(&mut self, callback: RealtimeErrorCallback) -> RealtimeResult<()> {
        if let Ok(mut g) = self.cb.error_cb.try_lock() {
            *g = Some(callback);
        }
        Ok(())
    }
    fn on_function_call(&mut self, callback: FunctionCallCallback) -> RealtimeResult<()> {
        if let Ok(mut g) = self.cb.function_call_cb.try_lock() {
            *g = Some(callback);
        }
        Ok(())
    }
    fn on_speech_event(&mut self, callback: SpeechEventCallback) -> RealtimeResult<()> {
        if let Ok(mut g) = self.cb.speech_event_cb.try_lock() {
            *g = Some(callback);
        }
        Ok(())
    }
    fn on_response_done(&mut self, callback: ResponseDoneCallback) -> RealtimeResult<()> {
        if let Ok(mut g) = self.cb.response_done_cb.try_lock() {
            *g = Some(callback);
        }
        Ok(())
    }
    fn on_reconnection(&mut self, callback: ReconnectionCallback) -> RealtimeResult<()> {
        if let Ok(mut g) = self.reconnection_cb.try_lock() {
            *g = Some(callback);
        }
        Ok(())
    }

    async fn update_session(&mut self, config: RealtimeConfig) -> RealtimeResult<()> {
        self.config = config;
        self.push_wires(self.protocol.build_session_config(&self.config, None)).await
    }

    async fn submit_function_result(&mut self, call_id: &str, result: &str) -> RealtimeResult<()> {
        self.push_wires(self.protocol.format_tool_result(call_id, result)).await
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": self.protocol.provider_id(),
            "model": self.config.model,
            "connected": self.connected.load(Ordering::SeqCst),
        })
    }

    fn set_resilience(&mut self, resilience: ResilienceHandles) {
        self.resilience = Some(resilience);
    }

    fn emits_user_turn_frames(&self) -> bool {
        self.caps.emits_user_turn_frames
    }

    async fn truncate_response(&mut self, item_id: &str, audio_end_ms: u64) -> RealtimeResult<()> {
        if !self.caps.supports_truncate {
            return Ok(());
        }
        self.push_wires(self.protocol.truncate(item_id, audio_end_ms)).await
    }

    async fn truncate_current_response(&mut self) -> RealtimeResult<Option<(String, u64)>> {
        if !self.caps.supports_truncate {
            return Ok(None);
        }
        let target = {
            let mut pb = self.cb.playback.lock().unwrap();
            pb.take().map(|p| {
                // Clamp to what the user ACTUALLY heard: min(wall-clock elapsed
                // since the first audio delta, received duration).
                let elapsed_ms = p.first_delta.elapsed().as_millis() as u64;
                let end = clamp_truncate_ms(elapsed_ms, p.duration_ms);
                (p.item_id, end)
            })
        };
        if let Some((item_id, end_ms)) = &target {
            self.push_wires(self.protocol.truncate(item_id, *end_ms)).await?;
        }
        Ok(target)
    }

    async fn replay_user_audio_preroll(&mut self) -> RealtimeResult<()> {
        if !self.caps.supports_input_buffer {
            return Ok(());
        }
        for chunk in self.preroll.snapshot() {
            self.push_wire(self.protocol.encode_user_audio(&chunk)).await?;
        }
        Ok(())
    }

    async fn replay_conversation(&mut self, items: &[ReplayConversationItem]) -> RealtimeResult<()> {
        for item in items {
            self.push_wires(self.protocol.replay_item(item)).await?;
        }
        Ok(())
    }
}
