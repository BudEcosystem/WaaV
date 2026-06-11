//! # Generic WebSocket Streaming TTS Client (P1.1)
//!
//! [`WebSocketTtsClient`] is the shared transport for streaming TTS providers
//! (Deepgram Aura WS first; Cartesia/ElevenLabs WS variants reuse it). It owns a
//! persistent tokio-tungstenite connection and maps the provider's wire protocol —
//! abstracted behind [`WsTtsProtocol`] — onto the existing [`BaseTTS`]-shaped
//! contract used by the rest of the gateway:
//!
//! - `speak(text, flush=false)` accumulates into an internal text buffer;
//!   `flush=true` sends ONE provider `Speak` frame (buffer + text) followed by the
//!   provider's `Flush` frame and registers a [`PendingUtterance`].
//! - Binary frames are delivered straight to the registered
//!   [`AudioCallback::on_audio`]; the provider's `Flushed` event drives
//!   [`AudioCallback::on_complete`]; `clear()` cancels every in-flight utterance
//!   and sends the provider's `Clear` frame (audio is dropped until `Cleared`).
//! - TTFB is stamped on the FIRST binary frame of each utterance using
//!   [`crate::core::observability::now_monotonic_ns`].
//!
//! ## Lock-safety invariant (MASTER_PLAN §P1.1)
//!
//! `VoiceManager` holds the provider behind `Arc<RwLock<Box<dyn BaseTTS>>>` and
//! takes a WRITE lock per `speak()`. The recv task spawned at [`connect`] therefore
//! holds NO outer lock: it owns the socket read-half outright and communicates only
//! via tokio channels plus this module's small internal `parking_lot` mutexes
//! (never held across an `.await`). `speak_with_context()` only channel-sends.
//!
//! ## Failure handling
//!
//! On a socket error the client marks itself not-ready, transitions to
//! [`ConnectionState::Error`], cancels all in-flight utterances and surfaces the
//! failure via [`AudioCallback::on_error`]. Full `ReconnectableStream` supervision
//! is a follow-up; the next `connect()` (e.g. lazy reconnect from the provider's
//! `speak`) tears down the dead tasks and dials fresh.
//!
//! [`connect`]: WebSocketTtsClient::connect
//! [`BaseTTS`]: super::base::BaseTTS

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::base::{AudioCallback, AudioData, ConnectionState, TTSError, TTSResult};
use crate::core::observability::now_monotonic_ns;

/// Default WebSocket connect timeout when the config does not specify one.
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 30;
/// Outbound frame channel depth — speak() only channel-sends, so this bounds the
/// number of frames buffered while the send task drains to the socket.
const OUTBOUND_CHANNEL_BUFFER: usize = 64;
/// Grace period for the send/recv tasks to exit on disconnect before abort.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// Client-side classification of a provider text (JSON) frame.
#[derive(Debug, Clone, PartialEq)]
pub enum WsTtsEvent {
    /// Synthesis of all flushed text is complete (e.g. Deepgram `Flushed`).
    Flushed,
    /// The server confirmed a clear/cancel (e.g. Deepgram `Cleared`).
    Cleared,
    /// Informational metadata; logged, no client action.
    Metadata,
    /// Provider warning surfaced in logs only.
    Warning(String),
    /// Provider error surfaced via [`AudioCallback::on_error`].
    Error(String),
    /// Frame is not relevant to the client.
    Ignored,
}

/// Wire protocol of one streaming TTS provider, consumed by [`WebSocketTtsClient`].
///
/// Implementations are pure data-mapping: build the text frames the provider expects
/// and classify the text frames it sends back. All transport, buffering, utterance
/// tracking and callback delivery live in the generic client.
pub trait WsTtsProtocol: Send + Sync + 'static {
    /// Provider name for logs/metrics (e.g. `"deepgram"`).
    fn provider_name(&self) -> &'static str;
    /// JSON text frame that submits `text` for synthesis.
    fn speak_frame(&self, text: &str) -> String;
    /// JSON text frame that forces synthesis of submitted text, if the protocol has one.
    fn flush_frame(&self) -> Option<String>;
    /// JSON text frame that cancels server-side buffered/in-flight synthesis.
    fn clear_frame(&self) -> Option<String>;
    /// JSON text frame announcing an orderly shutdown, sent before the WS close frame.
    fn close_frame(&self) -> Option<String>;
    /// Classify an incoming provider text frame.
    fn classify_text_frame(&self, raw: &str) -> WsTtsEvent;
    /// Sample rate of the binary audio frames.
    fn sample_rate(&self) -> u32;
    /// Audio format label of the binary audio frames (e.g. `"linear16"`).
    fn audio_format(&self) -> String;
}

/// Everything the client needs to dial the provider socket.
#[derive(Debug, Clone)]
pub struct WsTtsConnectSpec {
    /// Full connection URL including query string.
    pub url: String,
    /// Extra HTTP headers for the upgrade request (e.g. `Authorization`). The five
    /// mandatory WS handshake headers come from `into_client_request` — never build
    /// the upgrade request by hand (Alibaba/Sarvam fleet bug class).
    pub headers: Vec<(String, String)>,
    /// True when the URL host originates from a client-supplied `endpoint_override`;
    /// such URLs MUST pass the DAG's SSRF rules before the socket dials out.
    pub url_is_override: bool,
    /// Connect timeout; defaults to [`DEFAULT_CONNECT_TIMEOUT_SECS`] when `None`.
    pub connect_timeout: Option<Duration>,
}

/// One in-flight utterance (a `Speak`+`Flush` pair awaiting its `Flushed`).
#[derive(Debug)]
pub struct PendingUtterance {
    /// Cancelled by `clear()`; audio for a cancelled utterance is dropped.
    pub cancel_token: CancellationToken,
    /// Monotonic ns at request (channel-send) time; TTFB = first binary frame − this.
    pub request_ts_ns: u64,
    /// Whether the first binary frame for this utterance was already seen (TTFB stamped).
    first_frame_seen: bool,
}

/// Utterance bookkeeping: map keyed by context id + FIFO order of submission.
/// Providers stream responses in submission order, so the queue front is the
/// utterance currently being synthesized.
#[derive(Default)]
struct PendingMap {
    by_context: HashMap<String, PendingUtterance>,
    order: VecDeque<String>,
}

impl PendingMap {
    fn cancel_all(&mut self) {
        for entry in self.by_context.values() {
            entry.cancel_token.cancel();
        }
    }

    fn purge(&mut self) {
        self.by_context.clear();
        self.order.clear();
    }
}

/// State shared between the client handle and the detached send/recv tasks.
///
/// Mutexes here are `parking_lot` and are only held for short, non-await scopes —
/// the recv task never holds an outer lock (MASTER_PLAN §P1.1 lock-safety invariant).
struct WsShared {
    pending: parking_lot::Mutex<PendingMap>,
    callback: parking_lot::Mutex<Option<Arc<dyn AudioCallback>>>,
    /// `flush=false` text accumulation.
    buffer: parking_lot::Mutex<String>,
    state: parking_lot::Mutex<ConnectionState>,
    ready: AtomicBool,
    /// Set after sending `Clear`; binary frames are dropped until `Cleared` arrives.
    clearing: AtomicBool,
    /// Set by `disconnect()` so the recv task exiting is not reported as a failure.
    intentional_disconnect: AtomicBool,
    /// TTFB of the most recent utterance in ns (0 = none stamped yet).
    last_ttfb_ns: AtomicU64,
}

impl WsShared {
    fn new() -> Self {
        Self {
            pending: parking_lot::Mutex::new(PendingMap::default()),
            callback: parking_lot::Mutex::new(None),
            buffer: parking_lot::Mutex::new(String::new()),
            state: parking_lot::Mutex::new(ConnectionState::Disconnected),
            ready: AtomicBool::new(false),
            clearing: AtomicBool::new(false),
            intentional_disconnect: AtomicBool::new(false),
            last_ttfb_ns: AtomicU64::new(0),
        }
    }

    fn set_state(&self, state: ConnectionState) {
        *self.state.lock() = state;
    }

    /// Socket-level failure: mark not-ready, cancel in-flight work and surface the
    /// error through the callback error path (reconnect supervision is a follow-up).
    async fn fail(&self, provider: &str, reason: String) {
        if self.intentional_disconnect.load(Ordering::Acquire) {
            return; // orderly shutdown, not a failure
        }
        self.ready.store(false, Ordering::Release);
        self.clearing.store(false, Ordering::Release);
        self.set_state(ConnectionState::Error(reason.clone()));
        {
            let mut pending = self.pending.lock();
            pending.cancel_all();
            pending.purge();
        }
        error!(provider, "streaming TTS socket failed: {reason}");
        let cb = self.callback.lock().clone();
        if let Some(cb) = cb {
            cb.on_error(TTSError::NetworkError(reason)).await;
        }
    }
}

/// Frames travelling from the client handle to the send task (which owns the write half).
enum Outbound {
    /// Raw provider text frame.
    Frame(String),
    /// Orderly shutdown: optional protocol close frame, then the WS close frame.
    Shutdown,
}

/// Generic streaming-TTS WebSocket client. See module docs for the contract.
pub struct WebSocketTtsClient {
    protocol: Arc<dyn WsTtsProtocol>,
    spec: WsTtsConnectSpec,
    shared: Arc<WsShared>,
    outbound_tx: Option<mpsc::Sender<Outbound>>,
    send_task: Option<JoinHandle<()>>,
    recv_task: Option<JoinHandle<()>>,
}

impl WebSocketTtsClient {
    /// Create a disconnected client for `protocol` dialing `spec`.
    pub fn new(protocol: Arc<dyn WsTtsProtocol>, spec: WsTtsConnectSpec) -> Self {
        Self {
            protocol,
            spec,
            shared: Arc::new(WsShared::new()),
            outbound_tx: None,
            send_task: None,
            recv_task: None,
        }
    }

    /// Establish the persistent socket and spawn the detached send/recv tasks.
    ///
    /// Idempotent-ish: a live connection is left alone; a dead one (after a socket
    /// failure) is torn down and re-dialed, preserving the registered callback and
    /// any buffered text (both live in the shared state, not the connection).
    pub async fn connect(&mut self) -> TTSResult<()> {
        if self.is_ready() {
            return Ok(());
        }
        self.teardown_tasks().await;

        // SSRF gate (MASTER_PLAN §P1.1): a client-supplied endpoint_override must not
        // be able to point the gateway's socket at internal services. Same rules as
        // the DAG endpoint nodes; loopback only under WAAV_ALLOW_LOOPBACK_ENDPOINTS=1.
        if self.spec.url_is_override {
            validate_ws_endpoint_for_ssrf(&self.spec.url)?;
        }

        self.shared
            .intentional_disconnect
            .store(false, Ordering::Release);
        self.shared.set_state(ConnectionState::Connecting);

        // Build the upgrade request via `into_client_request` (repo convention): it
        // derives the 5 mandatory WS handshake headers; we only add provider headers.
        let mut request = self.spec.url.as_str().into_client_request().map_err(|e| {
            self.shared.set_state(ConnectionState::Disconnected);
            TTSError::InvalidConfiguration(format!(
                "invalid WebSocket URL '{}': {e}",
                self.spec.url
            ))
        })?;
        for (name, value) in &self.spec.headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                TTSError::InvalidConfiguration(format!("invalid header name '{name}': {e}"))
            })?;
            let value = HeaderValue::from_str(value).map_err(|e| {
                TTSError::InvalidConfiguration(format!("invalid header value for {name}: {e}"))
            })?;
            request.headers_mut().insert(name, value);
        }

        let timeout = self
            .spec
            .connect_timeout
            .unwrap_or(Duration::from_secs(DEFAULT_CONNECT_TIMEOUT_SECS));
        let ws_stream = match tokio::time::timeout(timeout, connect_async(request)).await {
            Ok(Ok((stream, _response))) => stream,
            Ok(Err(e)) => {
                let reason = format!(
                    "{} streaming TTS connect failed: {e}",
                    self.protocol.provider_name()
                );
                self.shared.set_state(ConnectionState::Error(reason.clone()));
                return Err(TTSError::ConnectionFailed(reason));
            }
            Err(_) => {
                let reason = format!(
                    "{} streaming TTS connect timed out after {}s",
                    self.protocol.provider_name(),
                    timeout.as_secs()
                );
                self.shared.set_state(ConnectionState::Error(reason.clone()));
                return Err(TTSError::ConnectionFailed(reason));
            }
        };
        info!(
            provider = self.protocol.provider_name(),
            "streaming TTS WebSocket connected"
        );

        let (mut write, mut read) = ws_stream.split();
        let (outbound_tx, mut outbound_rx) = mpsc::channel::<Outbound>(OUTBOUND_CHANNEL_BUFFER);

        // --- Send task: owns the socket write half; drains the outbound channel. ---
        let send_shared = Arc::clone(&self.shared);
        let send_protocol = Arc::clone(&self.protocol);
        let send_task = tokio::spawn(async move {
            while let Some(outbound) = outbound_rx.recv().await {
                match outbound {
                    Outbound::Frame(frame) => {
                        if let Err(e) = write.send(Message::Text(frame.into())).await {
                            send_shared
                                .fail(
                                    send_protocol.provider_name(),
                                    format!("WebSocket send failed: {e}"),
                                )
                                .await;
                            break;
                        }
                    }
                    Outbound::Shutdown => {
                        if let Some(close) = send_protocol.close_frame() {
                            let _ = write.send(Message::Text(close.into())).await;
                        }
                        let _ = write.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
        });

        // --- Recv task: owns the socket read half; holds NO outer lock (§P1.1). ---
        let recv_shared = Arc::clone(&self.shared);
        let recv_protocol = Arc::clone(&self.protocol);
        let recv_task = tokio::spawn(async move {
            while let Some(message) = read.next().await {
                match message {
                    Ok(Message::Binary(data)) => {
                        Self::handle_binary_frame(&recv_shared, &recv_protocol, data.to_vec())
                            .await;
                    }
                    Ok(Message::Text(text)) => {
                        Self::handle_text_frame(&recv_shared, &recv_protocol, &text).await;
                    }
                    Ok(Message::Close(frame)) => {
                        debug!(
                            provider = recv_protocol.provider_name(),
                            "streaming TTS WebSocket closed by server: {frame:?}"
                        );
                        recv_shared
                            .fail(
                                recv_protocol.provider_name(),
                                "WebSocket closed by server".to_string(),
                            )
                            .await;
                        break;
                    }
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {
                        // tungstenite answers pings automatically on flush.
                    }
                    Err(e) => {
                        recv_shared
                            .fail(
                                recv_protocol.provider_name(),
                                format!("WebSocket receive failed: {e}"),
                            )
                            .await;
                        break;
                    }
                }
            }
        });

        self.outbound_tx = Some(outbound_tx);
        self.send_task = Some(send_task);
        self.recv_task = Some(recv_task);
        self.shared.ready.store(true, Ordering::Release);
        self.shared.set_state(ConnectionState::Connected);
        Ok(())
    }

    /// Binary frame → TTFB stamp (first frame per utterance) + `on_audio` delivery.
    async fn handle_binary_frame(
        shared: &Arc<WsShared>,
        protocol: &Arc<dyn WsTtsProtocol>,
        data: Vec<u8>,
    ) {
        // Dropping window: after `clear()` everything is discarded until `Cleared`.
        if shared.clearing.load(Ordering::Acquire) {
            return;
        }

        // Short, non-await lock scope: stamp TTFB on the FRONT utterance's first
        // frame and check its cancellation state.
        let cancelled = {
            let mut pending = shared.pending.lock();
            if let Some(front_id) = pending.order.front().cloned() {
                if let Some(entry) = pending.by_context.get_mut(&front_id) {
                    if !entry.first_frame_seen {
                        entry.first_frame_seen = true;
                        let ttfb_ns = now_monotonic_ns().saturating_sub(entry.request_ts_ns);
                        shared.last_ttfb_ns.store(ttfb_ns, Ordering::Release);
                        debug!(
                            provider = protocol.provider_name(),
                            context_id = %front_id,
                            ttfb_ms = ttfb_ns / 1_000_000,
                            "streaming TTS first audio frame (TTFB)"
                        );
                    }
                    entry.cancel_token.is_cancelled()
                } else {
                    false
                }
            } else {
                // Unsolicited audio (no registered utterance): deliver liberally.
                false
            }
        };
        if cancelled {
            return;
        }

        let callback = shared.callback.lock().clone();
        if let Some(callback) = callback {
            let sample_rate = protocol.sample_rate();
            let format = protocol.audio_format();
            // Mirror the HTTP dispatcher's duration math for raw PCM-family audio.
            let duration_ms = if crate::core::tts::sniff::is_pcm_family(&format) {
                let bytes_per_sample = if matches!(format.as_str(), "linear16" | "pcm" | "pcm16") {
                    2
                } else {
                    1
                };
                let samples = (data.len() / bytes_per_sample) as u32;
                Some((samples * 1000) / sample_rate.max(1))
            } else {
                None
            };
            callback
                .on_audio(AudioData {
                    data,
                    sample_rate,
                    format,
                    duration_ms,
                })
                .await;
        }
    }

    /// Text frame → protocol classification → completion/clear/error handling.
    async fn handle_text_frame(
        shared: &Arc<WsShared>,
        protocol: &Arc<dyn WsTtsProtocol>,
        raw: &str,
    ) {
        match protocol.classify_text_frame(raw) {
            WsTtsEvent::Flushed => {
                // Pop the finished utterance; fire on_complete only when it was not
                // cancelled and nothing else is in flight (mirrors the HTTP
                // dispatcher's "one completion after all queued audio" behavior).
                let (finished, queue_empty) = {
                    let mut pending = shared.pending.lock();
                    let finished = pending
                        .order
                        .pop_front()
                        .and_then(|id| pending.by_context.remove(&id));
                    (finished, pending.order.is_empty())
                };
                let was_cancelled = finished
                    .as_ref()
                    .map(|p| p.cancel_token.is_cancelled())
                    .unwrap_or(false);
                if shared.clearing.load(Ordering::Acquire) || was_cancelled {
                    debug!(
                        provider = protocol.provider_name(),
                        "Flushed for cancelled/cleared utterance — skipping on_complete"
                    );
                    return;
                }
                if queue_empty {
                    let callback = shared.callback.lock().clone();
                    if let Some(callback) = callback {
                        callback.on_complete().await;
                    }
                }
            }
            WsTtsEvent::Cleared => {
                debug!(
                    provider = protocol.provider_name(),
                    "streaming TTS clear acknowledged"
                );
                {
                    let mut pending = shared.pending.lock();
                    pending.purge();
                }
                shared.clearing.store(false, Ordering::Release);
            }
            WsTtsEvent::Metadata => {
                debug!(provider = protocol.provider_name(), "metadata frame: {raw}");
            }
            WsTtsEvent::Warning(message) => {
                warn!(
                    provider = protocol.provider_name(),
                    "streaming TTS warning: {message}"
                );
            }
            WsTtsEvent::Error(message) => {
                warn!(
                    provider = protocol.provider_name(),
                    "streaming TTS provider error: {message}"
                );
                let callback = shared.callback.lock().clone();
                if let Some(callback) = callback {
                    callback.on_error(TTSError::ProviderError(message)).await;
                }
            }
            WsTtsEvent::Ignored => {
                debug!(provider = protocol.provider_name(), "ignored frame: {raw}");
            }
        }
    }

    /// `speak(flush=false)` buffers; `flush=true` emits ONE `Speak` (buffer + text)
    /// followed by the provider `Flush` frame and registers the pending utterance.
    /// Only channel-sends — never awaits socket I/O (§P1.1 lock-safety invariant).
    pub async fn speak_with_context(
        &self,
        text: &str,
        flush: bool,
        context_id: Option<&str>,
    ) -> TTSResult<()> {
        if !self.is_ready() {
            return Err(TTSError::ProviderNotReady(format!(
                "{} streaming TTS is not connected",
                self.protocol.provider_name()
            )));
        }
        if !flush {
            self.shared.buffer.lock().push_str(text);
            return Ok(());
        }
        let combined = {
            let mut buffer = self.shared.buffer.lock();
            let mut combined = std::mem::take(&mut *buffer);
            combined.push_str(text);
            combined
        };
        if combined.is_empty() {
            return Ok(());
        }
        self.submit_utterance(&combined, context_id).await
    }

    /// Flush any buffered text (used by the trait-level `flush(&self)`).
    pub async fn flush_buffered(&self) -> TTSResult<()> {
        if !self.is_ready() {
            return Err(TTSError::ProviderNotReady(format!(
                "{} streaming TTS is not connected",
                self.protocol.provider_name()
            )));
        }
        let combined = std::mem::take(&mut *self.shared.buffer.lock());
        if combined.is_empty() {
            return Ok(());
        }
        self.submit_utterance(&combined, None).await
    }

    /// Register the pending utterance, then send `Speak` + `Flush`.
    /// Registration happens BEFORE the send so the recv task can never observe
    /// audio for an unregistered utterance.
    async fn submit_utterance(&self, text: &str, context_id: Option<&str>) -> TTSResult<()> {
        let context_id = context_id
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        {
            let mut pending = self.shared.pending.lock();
            pending.by_context.insert(
                context_id.clone(),
                PendingUtterance {
                    cancel_token: CancellationToken::new(),
                    request_ts_ns: now_monotonic_ns(),
                    first_frame_seen: false,
                },
            );
            pending.order.push_back(context_id.clone());
        }

        let mut frames = vec![self.protocol.speak_frame(text)];
        if let Some(flush_frame) = self.protocol.flush_frame() {
            frames.push(flush_frame);
        }
        for frame in frames {
            if let Err(e) = self.send_frame(frame).await {
                // Roll the registration back — no audio will ever arrive for it.
                let mut pending = self.shared.pending.lock();
                pending.by_context.remove(&context_id);
                pending.order.retain(|id| id != &context_id);
                return Err(e);
            }
        }
        debug!(
            provider = self.protocol.provider_name(),
            context_id = %context_id,
            chars = text.len(),
            "streaming TTS utterance submitted"
        );
        Ok(())
    }

    /// Cancel ALL in-flight utterances and send the provider's `Clear` frame.
    /// Binary frames are dropped from this instant until `Cleared` arrives.
    pub async fn clear(&self) -> TTSResult<()> {
        if !self.is_ready() {
            return Err(TTSError::ProviderNotReady(format!(
                "{} streaming TTS is not connected",
                self.protocol.provider_name()
            )));
        }
        self.shared.buffer.lock().clear();
        self.shared.pending.lock().cancel_all();
        if let Some(clear_frame) = self.protocol.clear_frame() {
            // Set the dropping window only when a Cleared ack will actually arrive.
            self.shared.clearing.store(true, Ordering::Release);
            if let Err(e) = self.send_frame(clear_frame).await {
                self.shared.clearing.store(false, Ordering::Release);
                return Err(e);
            }
        }
        Ok(())
    }

    async fn send_frame(&self, frame: String) -> TTSResult<()> {
        let Some(tx) = &self.outbound_tx else {
            return Err(TTSError::ProviderNotReady(
                "streaming TTS socket not connected".to_string(),
            ));
        };
        tx.send(Outbound::Frame(frame)).await.map_err(|_| {
            self.shared.ready.store(false, Ordering::Release);
            TTSError::NetworkError(format!(
                "{} streaming TTS send channel closed (socket dead)",
                self.protocol.provider_name()
            ))
        })
    }

    /// Orderly shutdown: protocol close frame + WS close, then task teardown.
    pub async fn disconnect(&mut self) -> TTSResult<()> {
        self.shared
            .intentional_disconnect
            .store(true, Ordering::Release);
        self.shared.ready.store(false, Ordering::Release);
        self.shared.clearing.store(false, Ordering::Release);
        {
            let mut pending = self.shared.pending.lock();
            pending.cancel_all();
            pending.purge();
        }
        if let Some(tx) = self.outbound_tx.take() {
            let _ = tx.send(Outbound::Shutdown).await;
        }
        self.teardown_tasks().await;
        self.shared.set_state(ConnectionState::Disconnected);
        Ok(())
    }

    /// Await task exit within a grace period, then abort.
    async fn teardown_tasks(&mut self) {
        self.outbound_tx = None; // closing the channel ends the send task
        if let Some(mut task) = self.send_task.take() {
            if tokio::time::timeout(SHUTDOWN_GRACE, &mut task).await.is_err() {
                warn!("streaming TTS send task did not exit in time; aborting");
                task.abort();
            }
        }
        if let Some(mut task) = self.recv_task.take() {
            // The recv task exits when the socket closes (the send task's WS close
            // frame triggers that for orderly shutdown). Abort as a backstop.
            if tokio::time::timeout(SHUTDOWN_GRACE, &mut task).await.is_err() {
                warn!("streaming TTS recv task did not exit in time; aborting");
                task.abort();
            }
        }
    }

    /// Register the audio callback (delivered per binary frame by the recv task).
    pub fn set_audio_callback(&self, callback: Arc<dyn AudioCallback>) {
        *self.shared.callback.lock() = Some(callback);
    }

    /// Remove the registered audio callback.
    pub fn remove_audio_callback(&self) {
        *self.shared.callback.lock() = None;
    }

    /// Connected and the socket tasks are healthy.
    pub fn is_ready(&self) -> bool {
        self.shared.ready.load(Ordering::Acquire) && self.outbound_tx.is_some()
    }

    /// Current connection state.
    pub fn connection_state(&self) -> ConnectionState {
        self.shared.state.lock().clone()
    }

    /// TTFB of the most recent utterance, if any frame was stamped yet.
    pub fn last_ttfb_ns(&self) -> Option<u64> {
        match self.shared.last_ttfb_ns.load(Ordering::Acquire) {
            0 => None,
            ns => Some(ns),
        }
    }

    /// Number of in-flight (submitted, not yet `Flushed`) utterances.
    pub fn in_flight(&self) -> usize {
        self.shared.pending.lock().order.len()
    }

    /// Characters currently accumulated by `speak(flush=false)`.
    pub fn buffered_chars(&self) -> usize {
        self.shared.buffer.lock().len()
    }
}

impl Drop for WebSocketTtsClient {
    fn drop(&mut self) {
        // Channel sender drops with self (ends the send task → WS close → recv task
        // end). Abort as a synchronous backstop for non-graceful drops.
        if let Some(task) = &self.send_task {
            task.abort();
        }
        if let Some(task) = &self.recv_task {
            task.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// SSRF validation for client-supplied WS endpoint overrides
// ---------------------------------------------------------------------------

/// Validate a client-supplied streaming-TTS endpoint with the SAME rules the DAG
/// uses for its endpoint nodes (`src/dag/nodes/endpoint.rs::validate_url_for_ssrf`):
/// http/https/ws/wss schemes only; loopback/private/metadata targets rejected unless
/// `WAAV_ALLOW_LOOPBACK_ENDPOINTS=1`; DNS names resolve-then-validate (rebind/TOCTOU).
///
/// The DAG function itself is module-private to `dag::nodes` (and the whole `dag`
/// tree is behind the `dag-routing` feature, while this client must also protect
/// non-DAG builds), so the rules are mirrored below rule-for-rule rather than
/// called through. Keep the two in lockstep; do not weaken either path.
pub fn validate_ws_endpoint_for_ssrf(url: &str) -> TTSResult<()> {
    validate_ws_url_for_ssrf_standalone(url)
}

/// Whether loopback/private endpoint targets are explicitly permitted.
/// Mirrors `crate::dag::nodes::endpoint::loopback_endpoints_allowed` exactly.
fn loopback_endpoints_allowed() -> bool {
    matches!(
        std::env::var("WAAV_ALLOW_LOOPBACK_ENDPOINTS").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

/// Rule-for-rule mirror of `src/dag/nodes/endpoint.rs::validate_url_for_ssrf`
/// (the canonical rule set; see [`validate_ws_endpoint_for_ssrf`] for why it is
/// mirrored instead of called). `ssrf_blocklist_matches_dag_rules` locks the matrix.
fn validate_ws_url_for_ssrf_standalone(url: &str) -> TTSResult<()> {
    let reject = |msg: String| {
        Err(TTSError::InvalidConfiguration(format!(
            "endpoint_override rejected (SSRF protection): {msg}"
        )))
    };

    let parsed = match url::Url::parse(url) {
        Ok(p) => p,
        Err(e) => return reject(format!("invalid URL '{url}': {e}")),
    };

    let scheme = parsed.scheme().to_lowercase();
    if !["http", "https", "ws", "wss"].contains(&scheme.as_str()) {
        return reject(format!(
            "URL scheme '{scheme}' not allowed. Use http, https, ws, or wss"
        ));
    }

    // Test/local-mock escape hatch (opt-in, OFF by default) — same as the DAG.
    if loopback_endpoints_allowed() {
        return Ok(());
    }

    let Some(host) = parsed.host_str() else {
        return reject(format!("URL '{url}' has no host"));
    };

    let host_lower = host.to_lowercase();
    let blocked_hostnames = [
        "localhost",
        "localhost.localdomain",
        "127.0.0.1",
        "::1",
        "0.0.0.0",
        "[::1]",
        "[::ffff:127.0.0.1]",
        "169.254.169.254",
        "metadata.google.internal",
        "metadata.gcp.internal",
        "internal",
        "intranet",
    ];
    if blocked_hostnames.contains(&host_lower.as_str()) {
        return reject(format!("URL host '{host}' is blocked"));
    }

    if let Ok(ip) = host.parse::<IpAddr>()
        && is_private_ip(&ip)
    {
        return reject(format!("URL points to private IP '{ip}'"));
    }

    if host.starts_with('[')
        && host.ends_with(']')
        && let Ok(ip) = host[1..host.len() - 1].parse::<Ipv6Addr>()
        && is_private_ipv6(&ip)
    {
        return reject(format!("URL points to private IPv6 '{ip}'"));
    }

    // Resolve-then-validate (DNS-rebind / TOCTOU): a public-looking hostname must
    // not currently resolve to a private/metadata address. Unresolvable hosts pass
    // (they may resolve later; the transport applies its own checks) — same policy
    // as the DAG validator.
    let is_ip_literal =
        host.parse::<IpAddr>().is_ok() || (host.starts_with('[') && host.ends_with(']'));
    if !is_ip_literal {
        use std::net::ToSocketAddrs;
        if let Ok(resolved) = (host, 0u16).to_socket_addrs() {
            for addr in resolved {
                if is_private_ip(&addr.ip()) {
                    return reject(format!(
                        "host '{host}' resolves to private/internal IP '{}'",
                        addr.ip()
                    ));
                }
            }
        }
    }

    Ok(())
}

/// Mirror of the DAG's `is_private_ip`.
fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

/// Mirror of the DAG's `is_private_ipv4`.
fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    if ip.is_loopback() || ip.is_link_local() || ip.is_broadcast() {
        return true;
    }
    let octets = ip.octets();
    octets[0] == 10
        || (octets[0] == 172 && (16..=31).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 168)
        || octets[0] == 0
}

/// Mirror of the DAG's `is_private_ipv6`.
fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_private_ipv4(&v4);
    }
    let segments = ip.segments();
    (0xfe80..=0xfebf).contains(&segments[0]) || (0xfc00..=0xfdff).contains(&segments[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NullProtocol;
    impl WsTtsProtocol for NullProtocol {
        fn provider_name(&self) -> &'static str {
            "null"
        }
        fn speak_frame(&self, text: &str) -> String {
            serde_json::json!({"type": "Speak", "text": text}).to_string()
        }
        fn flush_frame(&self) -> Option<String> {
            Some(r#"{"type":"Flush"}"#.to_string())
        }
        fn clear_frame(&self) -> Option<String> {
            Some(r#"{"type":"Clear"}"#.to_string())
        }
        fn close_frame(&self) -> Option<String> {
            None
        }
        fn classify_text_frame(&self, _raw: &str) -> WsTtsEvent {
            WsTtsEvent::Ignored
        }
        fn sample_rate(&self) -> u32 {
            24000
        }
        fn audio_format(&self) -> String {
            "linear16".to_string()
        }
    }

    fn client() -> WebSocketTtsClient {
        WebSocketTtsClient::new(
            Arc::new(NullProtocol),
            WsTtsConnectSpec {
                url: "wss://example.com/v1/speak".to_string(),
                headers: vec![],
                url_is_override: false,
                connect_timeout: None,
            },
        )
    }

    #[tokio::test]
    async fn speak_requires_connection() {
        let c = client();
        let err = c.speak_with_context("hi", true, None).await.unwrap_err();
        assert!(matches!(err, TTSError::ProviderNotReady(_)));
        assert!(!c.is_ready());
        assert_eq!(c.connection_state(), ConnectionState::Disconnected);
    }

    // Buffering itself is connection-independent state; exercise it directly so the
    // accumulate-then-single-Speak contract is unit-locked (the wire side is covered
    // by the deepgram_aura_ws_e2e mock test).
    #[tokio::test]
    async fn buffer_accumulates_until_flush() {
        let c = client();
        // Bypass readiness for the buffer-only path by writing through the shared
        // state exactly as speak(flush=false) does.
        c.shared.buffer.lock().push_str("Hello ");
        c.shared.buffer.lock().push_str("world");
        assert_eq!(c.buffered_chars(), 11);
        let combined = std::mem::take(&mut *c.shared.buffer.lock());
        assert_eq!(combined, "Hello world");
        assert_eq!(c.buffered_chars(), 0);
    }

    #[tokio::test]
    async fn pending_map_orders_and_cancels() {
        let mut map = PendingMap::default();
        for id in ["a", "b"] {
            map.by_context.insert(
                id.to_string(),
                PendingUtterance {
                    cancel_token: CancellationToken::new(),
                    request_ts_ns: now_monotonic_ns(),
                    first_frame_seen: false,
                },
            );
            map.order.push_back(id.to_string());
        }
        assert_eq!(map.order.front().map(String::as_str), Some("a"));
        map.cancel_all();
        assert!(map.by_context.values().all(|p| p.cancel_token.is_cancelled()));
        map.purge();
        assert!(map.order.is_empty() && map.by_context.is_empty());
    }

    // ---- SSRF: standalone mirror behaves like the DAG validator ----

    /// Env-var-dependent tests share one lock: `WAAV_ALLOW_LOOPBACK_ENDPOINTS` is
    /// process-global state.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn ssrf_rejects_loopback_and_private_without_flag() {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };
        for url in [
            "ws://127.0.0.1:9000/v1/speak",
            "wss://localhost/v1/speak",
            "ws://10.1.2.3/x",
            "ws://192.168.1.5/x",
            "ws://169.254.169.254/latest/meta-data",
            "ws://[::1]:9/x",
        ] {
            assert!(
                validate_ws_endpoint_for_ssrf(url).is_err(),
                "{url} must be rejected"
            );
        }
        // Non-WS/HTTP schemes rejected regardless of host.
        assert!(validate_ws_endpoint_for_ssrf("ftp://example.com/x").is_err());
    }

    // NOTE: the allow-path (loopback permitted under WAAV_ALLOW_LOOPBACK_ENDPOINTS=1)
    // is intentionally NOT unit-tested here: it would make this lib binary's only
    // env SETTER for that var, racing the unsynchronized removers in other modules'
    // tests (e.g. conversation::tests). It is wire-covered by the
    // `deepgram_aura_ws_e2e` suite, which connects through a loopback override with
    // the flag set under a shared env lock.

    /// Matrix tripwire mirroring the DAG validator's decisions (the canonical rule
    /// set in `src/dag/nodes/endpoint.rs`): blocked targets stay blocked, public
    /// IP-literal targets stay allowed. If the DAG rules ever change, change the
    /// mirror AND this matrix together.
    #[test]
    fn ssrf_blocklist_matches_dag_rules() {
        let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: test-only env mutation, serialized by env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };
        let blocked = [
            "ws://127.0.0.1:9000/v1/speak",
            "wss://localhost/v1/speak",
            "ws://10.0.0.1/x",
            "ws://172.16.0.1/x",
            "ws://192.168.0.1/x",
            "ws://169.254.169.254/x",
            "ws://metadata.google.internal/x",
            "ws://[::1]/x",
            "ws://0.0.0.0/x",
            "ftp://example.com/x",
        ];
        for url in blocked {
            assert!(
                validate_ws_url_for_ssrf_standalone(url).is_err(),
                "{url} must be blocked"
            );
        }
        // Public IP literals pass (no DNS resolution involved).
        for url in ["ws://8.8.8.8/x", "wss://8.8.4.4/v1/speak"] {
            assert!(
                validate_ws_url_for_ssrf_standalone(url).is_ok(),
                "{url} must be allowed"
            );
        }
    }
}
