//! AssemblyAI STT WebSocket client implementation.
//!
//! This module contains the main `AssemblyAISTT` struct that implements the
//! `BaseSTT` trait for real-time speech-to-text streaming using AssemblyAI's
//! Streaming API v3.
//!
//! # Key Features
//!
//! - **Immutable Transcripts**: Unlike other providers, AssemblyAI transcripts
//!   are never modified after delivery (when format_turns=true)
//! - **End-of-Turn Detection**: Automatic detection of speech boundaries
//! - **Binary Audio**: Audio is sent as raw binary data (no base64 encoding)
//! - **Word-Level Timing**: Every word includes precise timestamps

use bytes::Bytes;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tracing::{debug, error, info, warn};

// =============================================================================
// Constants
// =============================================================================

/// Maximum audio chunk size in bytes (sanity check).
///
/// AssemblyAI recommends ~50ms of audio per message but doesn't specify
/// explicit byte limits. This limit prevents memory issues from buggy clients
/// sending excessively large chunks. At 48kHz mono 16-bit PCM, 1 second of
/// audio is ~96KB, so 256KB allows for ~2.5 seconds which is generous.
const MAX_AUDIO_CHUNK_SIZE: usize = 256 * 1024;

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Minimum supported sample rate (8kHz for telephony)
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum supported sample rate (48kHz for high-quality audio)
pub const MAX_SAMPLE_RATE: u32 = 48000;

use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

/// Extract the `host[:port]` authority from a `ws://`/`wss://` base URL, for the `Host`
/// header when an `endpoint_override` is in effect.
fn ws_authority(base: &str) -> Option<String> {
    let rest = base
        .strip_prefix("wss://")
        .or_else(|| base.strip_prefix("ws://"))
        .or_else(|| base.strip_prefix("https://"))
        .or_else(|| base.strip_prefix("http://"))
        .unwrap_or(base);
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    if authority.is_empty() {
        None
    } else {
        Some(authority.to_string())
    }
}

use super::config::{
    AssemblyAIEncoding, AssemblyAIRegion, AssemblyAISTTConfig, AssemblyAISpeechModel,
};
use super::messages::{AssemblyAIMessage, ForceEndpointMessage, TerminateMessage};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Type Aliases
// =============================================================================

/// Type alias for the async result callback function.
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Type alias for the async error callback function.
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

// =============================================================================
// Supervised Transport (W-D1 fleet adoption)
// =============================================================================

/// The concrete WebSocket stream type AssemblyAI dials.
type AssemblyAiWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A [`WsTransport`] that adapts AssemblyAI's v3 streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// AssemblyAI carries the entire featured session in the connect URL (the `/v3/ws` query:
/// `sample_rate`/`encoding`/`speech_model`/`format_turns`/`keyterms_prompt`/`language_detection`/
/// `speaker_labels`/...), so [`restore_session`](WsTransport::restore_session) is a no-op — the
/// fresh dial already restored the featured session, exactly like ElevenLabs. The server then
/// opens the session with a `Begin` frame, which [`run`](WsTransport::run) (the original
/// `select!` loop) observes: it flips `is_connected` on EVERY Begin (initial and restored
/// sessions) and fires the one-shot connected signal on the first. `run` yields a
/// [`ReconnectOutcome`] so a transport drop reconnects (turns after the drop are recovered on
/// the new connection) while a `Termination`/shutdown stays final.
struct AssemblyAiTransport {
    ws_sink: SplitSink<AssemblyAiWs, Message>,
    ws_stream: SplitStream<AssemblyAiWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared control-message receiver (ForceEndpoint etc.; locked for the duration of `run`).
    control_rx: Arc<Mutex<mpsc::Receiver<String>>>,
    /// Shared shutdown signal (fires once; an intentional close must not reconnect).
    shutdown_rx: Arc<Mutex<oneshot::Receiver<()>>>,
    result_tx: mpsc::Sender<STTResult>,
    error_tx: mpsc::Sender<STTError>,
    /// Fires once when the first `Begin` arrives, unblocking `start_connection`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// Session ID assigned by the `Begin` frame (shared with the client).
    session_id: Arc<RwLock<Option<String>>>,
    /// Readiness flag shared with the client's `is_ready()`: true from `Begin` until the
    /// transport drops; restored by the next `Begin` after a supervised reconnect.
    is_connected: Arc<AtomicBool>,
    /// D-G1: shared replay ring — pushed by the send arm, cleared on
    /// end-of-turn, replayed by `restore_session`.
    replay: Arc<crate::core::websocket::AudioReplayBuffer>,
}

#[async_trait::async_trait]
impl WsTransport for AssemblyAiTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // AssemblyAI puts every feature (sample rate, encoding, speech model, format_turns,
        // keyterms, language detection, diarization) in the connect URL, so a fresh dial already
        // restored the featured session. What a fresh dial CANNOT restore is the un-finalized
        // audio tail — the provider is stateless across sockets, so any audio sent after the
        // last end-of-turn but lost to the drop must be replayed (D-G1). Empty on the first
        // connect.
        let tail = self.replay.snapshot();
        if !tail.is_empty() {
            let tail_bytes: usize = tail.iter().map(|c| c.len()).sum();
            info!(
                chunks = tail.len(),
                bytes = tail_bytes,
                "AssemblyAI: replaying un-finalized audio tail after reconnect"
            );
            for chunk in tail {
                self.ws_sink.send(Message::Binary(chunk)).await.map_err(|e| {
                    RestoreError::new(format!("audio replay send failed: {e}"))
                })?;
            }
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let mut audio_rx = self.audio_rx.lock().await;
        let mut control_rx = self.control_rx.lock().await;
        let mut shutdown_rx = self.shutdown_rx.lock().await;
        loop {
            tokio::select! {
                // Handle outgoing audio data
                Some(audio_data) = audio_rx.recv() => {
                    // D-G1: record BEFORE the write — a chunk whose write
                    // fails is precisely the audio the next connection must
                    // replay (Bytes clone = refcount).
                    self.replay.push(audio_data.clone());
                    // AssemblyAI accepts raw binary audio data (no base64 encoding)
                    // Zero-copy: Bytes is passed directly to WebSocket
                    let data_len = audio_data.len();
                    let message = Message::Binary(audio_data);
                    if let Err(e) = self.ws_sink.send(message).await {
                        let stt_error = STTError::NetworkError(format!(
                            "Failed to send audio to AssemblyAI: {e}"
                        ));
                        error!("{}", stt_error);
                        let _ = self.error_tx.try_send(stt_error);
                        self.is_connected.store(false, Ordering::Release);
                        // Transport-level send failure: reconnect to preserve the session.
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }

                    debug!("Sent {} bytes of audio to AssemblyAI", data_len);
                }

                // Handle control messages (ForceEndpoint, UpdateConfiguration, etc.)
                Some(control_msg) = control_rx.recv() => {
                    if let Err(e) = self.ws_sink.send(Message::Text(control_msg.into())).await {
                        warn!("Failed to send control message: {}", e);
                    }
                }

                // Handle incoming messages with idle timeout
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(msg))) => {
                            match AssemblyAISTT::handle_websocket_message(
                                msg,
                                &self.result_tx,
                                &self.session_id,
                                &self.replay,
                            ).await {
                                Ok(should_continue) => {
                                    if !should_continue {
                                        // Termination frame (or server Close after our
                                        // Terminate): the session ended on purpose — do NOT
                                        // reconnect.
                                        info!("AssemblyAI session terminated normally");
                                        self.is_connected.store(false, Ordering::Release);
                                        return ReconnectOutcome::Completed;
                                    }

                                    // Mark ready on every Begin (initial AND restored
                                    // sessions); the one-shot connected signal fires only for
                                    // the first.
                                    if !self.is_connected.load(Ordering::Acquire)
                                        && self.session_id.read().await.is_some()
                                    {
                                        self.is_connected.store(true, Ordering::Release);
                                        if let Some(tx) = self.connected_tx.lock().await.take() {
                                            let _ = tx.send(());
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("AssemblyAI streaming error: {}", e);
                                    let _ = self.error_tx.try_send(e);
                                    self.is_connected.store(false, Ordering::Release);
                                    // A provider error frame (bad config/auth/rate limit) is
                                    // fatal — reconnecting would fail identically.
                                    return ReconnectOutcome::Fatal(StreamError::new("provider error frame"));
                                }
                            }
                        }
                        Ok(Some(Err(e))) => {
                            let stt_error = STTError::NetworkError(format!(
                                "WebSocket error: {e}"
                            ));
                            error!("{}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            self.is_connected.store(false, Ordering::Release);
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("AssemblyAI WebSocket stream ended");
                            self.is_connected.store(false, Ordering::Release);
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            // Idle timeout - no message received for 60s
                            let stt_error = STTError::NetworkError(
                                "WebSocket idle timeout - no message for 60 seconds".into()
                            );
                            error!("AssemblyAI STT idle timeout: {}", stt_error);
                            let _ = self.error_tx.try_send(stt_error);
                            self.is_connected.store(false, Ordering::Release);
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect)
                _ = &mut *shutdown_rx => {
                    info!("Received shutdown signal for AssemblyAI STT");

                    // Send terminate message for graceful shutdown
                    let terminate_msg = TerminateMessage::default();
                    if let Ok(json) = serde_json::to_string(&terminate_msg) {
                        let _ = self.ws_sink.send(Message::Text(json.into())).await;
                    }

                    let _ = self.ws_sink.send(Message::Close(None)).await;
                    self.is_connected.store(false, Ordering::Release);
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

// =============================================================================
// Connection State
// =============================================================================

/// Connection state for the WebSocket client.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

// =============================================================================
// AssemblyAISTT Client
// =============================================================================

/// AssemblyAI STT WebSocket client.
///
/// This struct implements real-time speech-to-text using the AssemblyAI
/// Streaming API v3. It manages:
/// - WebSocket connection lifecycle
/// - Audio data streaming to the API
/// - Transcription result callbacks
/// - Error handling and recovery
///
/// # Architecture
///
/// The implementation uses a multi-channel architecture for low-latency processing:
///
/// ```text
/// ┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
/// │   send_audio()  │────▶│  ws_sender (mpsc)│────▶│  WebSocket Task │
/// └─────────────────┘     └──────────────────┘     └────────┬────────┘
///                                                           │
///                         ┌──────────────────┐              │
///                         │  result_tx (mpsc)│◀─────────────┘
///                         └────────┬─────────┘
///                                  │
///                         ┌────────▼─────────┐
///                         │ Result Forward   │────▶ User Callback
///                         │      Task        │
///                         └──────────────────┘
/// ```
///
/// # Key Differences from Other Providers
///
/// 1. **Binary Audio**: Audio is sent as raw binary WebSocket frames, not base64
/// 2. **Immutable Transcripts**: When `format_turns=true`, transcripts are never modified
/// 3. **End-of-Turn**: Transcripts include `end_of_turn` flag for speech boundaries
///
/// # Thread Safety
///
/// All shared state is protected by either:
/// - `tokio::sync::Mutex` for async-safe access to callbacks
/// - `Arc<Notify>` for state change notifications
/// - Bounded `mpsc` channels for backpressure control
pub struct AssemblyAISTT {
    /// Configuration for the STT client
    pub(crate) config: Option<AssemblyAISTTConfig>,

    /// Current connection state
    pub(crate) state: ConnectionState,

    /// State change notification
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data
    /// Uses bounded channel (32 items) to provide backpressure
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Control message sender for ForceEndpoint, Terminate, etc.
    control_tx: Option<mpsc::Sender<String>>,

    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Result channel sender
    result_tx: Option<mpsc::Sender<STTResult>>,

    /// Error channel sender
    error_tx: Option<mpsc::Sender<STTError>>,

    /// Connection task handle
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result forwarding task handle
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Error forwarding task handle
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Shared callback storage for async access
    pub(crate) result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,

    /// Error callback storage
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,

    /// Session ID from the AssemblyAI connection (shared with connection task)
    session_id: Arc<RwLock<Option<String>>>,

    /// Connection state flag (shared with connection task)
    is_connected: Arc<AtomicBool>,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState. `None`
    /// when constructed directly (unit tests) — then the connect path falls back to per-session
    /// handles so storm control degrades gracefully rather than panicking.
    resilience: Option<crate::core::resilience::ResilienceHandles>,

    /// Intentional-disconnect flag shared with the [`ReconnectableStream`] supervisor (W-D1).
    /// Cleared on `start_connection`, set in `disconnect()` before firing `shutdown_tx`. The
    /// supervisor checks it at its loop top and after a racy `Reconnectable` outcome, so a client
    /// close racing a server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// D-G1 reconnect audio-replay: the un-finalized audio tail (cleared on
    /// every end-of-turn), replayed into the fresh socket by
    /// `restore_session` after a supervised reconnect.
    replay: Arc<crate::core::websocket::AudioReplayBuffer>,
}

impl Default for AssemblyAISTT {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            control_tx: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(AtomicBool::new(false)),
            resilience: None,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            replay: Arc::new(crate::core::websocket::AudioReplayBuffer::default()),
        }
    }
}

impl AssemblyAISTT {
    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// AssemblyAI v3 can express (word-level timestamps) are honored END-TO-END. The flat
    /// `BaseSTT::new` path maps only the base config; this is the reachable standardized path.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required for AssemblyAI STT".to_string(),
            ));
        }
        let sample_rate = std.base.sample_rate;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate {} Hz is outside supported range ({}-{} Hz)",
                sample_rate, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE
            )));
        }
        // `AssemblyAISTT` implements `Drop`, so build the default then set the config field
        // (struct-update with `..Default::default()` would move out of a Drop type).
        let mut stt = Self::default();
        stt.config = Some(AssemblyAISTTConfig::from_standard(std));
        Ok(stt)
    }

    /// Get the host name from the region for HTTP headers.
    pub(crate) fn get_host_from_region(region: &AssemblyAIRegion) -> &'static str {
        region.host()
    }

    /// Handle incoming WebSocket messages from AssemblyAI.
    ///
    /// This method is optimized for the hot path of message processing:
    /// - Parses JSON once
    /// - Branches on message type
    /// - Converts to internal STTResult format
    /// - Non-blocking result transmission
    ///
    /// # Arguments
    /// * `message` - The WebSocket message to handle
    /// * `result_tx` - Channel to send transcription results
    /// * `session_id` - Shared session ID storage
    ///
    /// # Returns
    /// * `Ok(true)` - Continue processing messages
    /// * `Ok(false)` - Session terminated, close connection
    /// * `Err(STTError)` - Error occurred, close connection
    pub(crate) async fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
        session_id: &Arc<RwLock<Option<String>>>,
        replay: &crate::core::websocket::AudioReplayBuffer,
    ) -> Result<bool, STTError> {
        // Returns true if connection should continue, false if terminated
        match message {
            Message::Text(text) => {
                debug!("Received AssemblyAI message: {}", text);

                match AssemblyAIMessage::parse(&text) {
                    Ok(parsed_msg) => match parsed_msg {
                        AssemblyAIMessage::Begin(begin) => {
                            info!(
                                "AssemblyAI STT session started: {} (expires at: {})",
                                begin.id, begin.expires_at
                            );
                            *session_id.write().await = Some(begin.id);
                        }

                        AssemblyAIMessage::Turn(turn) => {
                            // Calculate average confidence from words
                            let confidence = if turn.words.is_empty() {
                                1.0
                            } else {
                                let sum: f64 = turn.words.iter().map(|w| w.confidence).sum();
                                (sum / turn.words.len() as f64) as f32
                            };

                            // D-G1: an end-of-turn means everything sent so
                            // far is durably transcribed — drop the tail.
                            if turn.end_of_turn {
                                replay.clear();
                            }
                            let stt_result = STTResult::new(
                                turn.transcript,
                                turn.end_of_turn, // is_final
                                turn.end_of_turn, // is_speech_final
                                confidence.clamp(0.0, 1.0),
                            );

                            if result_tx.try_send(stt_result).is_err() {
                                warn!("Failed to send turn result - channel closed");
                            }

                            // Log language detection if present
                            if let (Some(lang), Some(conf)) =
                                (&turn.language, turn.language_confidence)
                            {
                                debug!("Detected language: {} (confidence: {:.2})", lang, conf);
                            }
                        }

                        AssemblyAIMessage::Termination(term) => {
                            info!(
                                "AssemblyAI session terminated (duration: {}ms, normal: {})",
                                term.audio_duration_ms, term.terminated_normally
                            );
                            return Ok(false); // Signal to close connection
                        }

                        AssemblyAIMessage::Error(err) => {
                            let error_msg = format!(
                                "AssemblyAI STT error{}: {}",
                                err.error_code
                                    .as_ref()
                                    .map(|c| format!(" ({})", c))
                                    .unwrap_or_default(),
                                err.error
                            );
                            error!("{}", error_msg);

                            return match err.error_code.as_deref() {
                                Some("invalid_api_key") | Some("authentication_failed") => {
                                    Err(STTError::AuthenticationFailed(err.error))
                                }
                                Some("rate_limit_exceeded") | Some("rate_limit") => {
                                    Err(STTError::ProviderError(format!(
                                        "Rate limit exceeded: {}",
                                        err.error
                                    )))
                                }
                                Some("invalid_audio") | Some("audio_error") => {
                                    Err(STTError::InvalidAudioFormat(err.error))
                                }
                                _ => Err(STTError::ProviderError(err.error)),
                            };
                        }

                        AssemblyAIMessage::Unknown(raw) => {
                            debug!("Received unknown AssemblyAI message type: {}", raw);
                        }
                    },
                    Err(e) => {
                        warn!("Failed to parse AssemblyAI message: {}", e);
                    }
                }
            }

            Message::Close(close_frame) => {
                info!("AssemblyAI WebSocket closed: {:?}", close_frame);
                return Ok(false);
            }

            Message::Ping(_) => {
                debug!("Received ping from AssemblyAI");
            }

            Message::Pong(_) => {
                debug!("Received pong from AssemblyAI");
            }

            Message::Binary(_) => {
                debug!("Received unexpected binary message from AssemblyAI");
            }

            _ => {
                debug!("Received unexpected message type");
            }
        }

        Ok(true) // Continue connection
    }

    /// Start the WebSocket connection to AssemblyAI STT API.
    async fn start_connection(&mut self, config: AssemblyAISTTConfig) -> Result<(), STTError> {
        // Validate sample rate
        let sample_rate = config.base.sample_rate;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate {} Hz is outside supported range ({}-{} Hz)",
                sample_rate, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE
            )));
        }

        let ws_url = config.build_websocket_url();

        // Create channels for communication
        let (ws_tx, ws_rx) = mpsc::channel::<Bytes>(32);
        let (control_tx, control_rx) = mpsc::channel::<String>(8);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        // Bounded channels for backpressure - 256 should handle bursts while preventing memory exhaustion
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(256);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(64);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        // Store channels
        self.ws_sender = Some(ws_tx);
        self.control_tx = Some(control_tx);
        self.shutdown_tx = Some(shutdown_tx);
        self.result_tx = Some(result_tx.clone());
        self.error_tx = Some(error_tx.clone());

        // Clone necessary data for the connection task
        let api_key = config.base.api_key.clone();
        // The Host header must match the endpoint actually dialed: for an override
        // (mock/proxy) derive it from the override authority; otherwise the regional host.
        let host: String = match &config.endpoint_override {
            Some(o) => ws_authority(o)
                .unwrap_or_else(|| Self::get_host_from_region(&config.region).to_string()),
            None => Self::get_host_from_region(&config.region).to_string(),
        };

        // Clone shared state for the supervised transport
        let session_id = self.session_id.clone();
        let is_connected = self.is_connected.clone();

        // Shared state the supervised transport re-uses across reconnect attempts: single-
        // consumer audio/control receivers + shutdown oneshot (locked per `run`) and the
        // one-shot connected signal that fires on the first Begin.
        let audio_rx = Arc::new(Mutex::new(ws_rx));
        let control_rx = Arc::new(Mutex::new(control_rx));
        let shutdown_rx = Arc::new(Mutex::new(shutdown_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor
        // (the same one the chaos tests exercise) with the shared process-global handles from
        // CoreState (W-D1/W-D2 fleet adoption) so every AssemblyAI session trips the same
        // breaker and shares the one process-wide reconnect cap. When no handles were injected
        // (a unit test constructing the provider directly), the supervisor uses its own
        // per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        // Fresh session: clear any intent left over from a prior disconnect, and share the flag
        // into the supervisor so disconnect() can stop a reconnect that races a server-side
        // close (W-D1 disconnect-vs-close race fix).
        self.intentional_disconnect.store(false, Ordering::SeqCst);
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let replay = Arc::clone(&self.replay);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("assemblyai", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("assemblyai", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure dials the *featured* URL (the /v3/ws query carries format_turns / keyterms /
        // language_detection / ... — re-dialing it IS the session restore) and hands back a
        // transport whose `run()` is the original AssemblyAI event loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let ws_url = ws_url.clone();
                    let host = host.clone();
                    let api_key = api_key.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let control_rx = Arc::clone(&control_rx);
                    let shutdown_rx = Arc::clone(&shutdown_rx);
                    let connected_tx = Arc::clone(&connected_tx);
                    let session_id = Arc::clone(&session_id);
                    let is_connected = Arc::clone(&is_connected);
                    let result_tx = result_tx.clone();
                    let error_tx = error_tx.clone();
                    let replay = Arc::clone(&replay);
                    async move {
                        // Build WebSocket request with AssemblyAI authentication
                        // Note: AssemblyAI uses "Authorization: <API_KEY>" (no Bearer prefix
                        // for WebSocket)
                        let request = tokio_tungstenite::tungstenite::http::Request::builder()
                            .method("GET")
                            .uri(&ws_url)
                            .header("Host", host)
                            .header("Upgrade", "websocket")
                            .header("Connection", "upgrade")
                            .header("Sec-WebSocket-Key", generate_key())
                            .header("Sec-WebSocket-Version", "13")
                            .header("Authorization", &api_key) // AssemblyAI uses raw API key
                            .body(())
                            .map_err(|e| {
                                let stt_error = STTError::ConnectionFailed(format!(
                                    "Failed to create WebSocket request: {e}"
                                ));
                                error!("{}", stt_error);
                                if error_tx.try_send(stt_error).is_err() {
                                    error!(
                                        "AssemblyAI error channel full/closed — fatal connection \
                                         error NOT delivered to caller (session will appear silent)"
                                    );
                                }
                                StreamError::new(format!("Failed to create WebSocket request: {e}"))
                            })?;

                        // Connect to AssemblyAI
                        let (ws_stream, _response) = connect_async(request).await.map_err(|e| {
                            let stt_error = STTError::ConnectionFailed(format!(
                                "Failed to connect to AssemblyAI: {e}"
                            ));
                            error!("{}", stt_error);
                            if error_tx.try_send(stt_error).is_err() {
                                error!(
                                    "AssemblyAI error channel full/closed — connection error \
                                         NOT delivered to caller (session will appear silent)"
                                );
                            }
                            StreamError::new(format!("Failed to connect to AssemblyAI: {e}"))
                        })?;

                        info!("Connected to AssemblyAI STT WebSocket");
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(AssemblyAiTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            control_rx,
                            shutdown_rx,
                            result_tx,
                            error_tx,
                            connected_tx,
                            session_id,
                            is_connected,
                            replay,
                        })
                    }
                })
                .await;
            info!("AssemblyAI STT WebSocket connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Start result forwarding task
        let callback_ref = self.result_callback.clone();
        let result_forwarding_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                if let Some(callback) = callback_ref.lock().await.as_ref() {
                    callback(result).await;
                } else {
                    debug!(
                        "AssemblyAI STT result (no callback): {} (final: {}, confidence: {})",
                        result.transcript, result.is_final, result.confidence
                    );
                }
            }
        });

        self.result_forward_handle = Some(result_forwarding_handle);

        // Start error forwarding task
        let error_callback_ref = self.error_callback.clone();
        let error_forwarding_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                if let Some(callback) = error_callback_ref.lock().await.as_ref() {
                    callback(error).await;
                } else {
                    error!("AssemblyAI STT error (no callback registered): {}", error);
                }
            }
        });

        self.error_forward_handle = Some(error_forwarding_handle);

        // Update state and wait for connection
        self.state = ConnectionState::Connecting;

        // Wait for Begin message with timeout
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to AssemblyAI STT");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed before session started".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Connection timeout waiting for Begin message".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }
}

impl Drop for AssemblyAISTT {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

// =============================================================================
// BaseSTT Trait Implementation
// =============================================================================

#[async_trait::async_trait]
impl BaseSTT for AssemblyAISTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required for AssemblyAI STT".to_string(),
            ));
        }

        // Validate sample rate early
        let sample_rate = config.sample_rate;
        if !(MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&sample_rate) {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate {} Hz is outside supported range ({}-{} Hz)",
                sample_rate, MIN_SAMPLE_RATE, MAX_SAMPLE_RATE
            )));
        }

        let assemblyai_config = AssemblyAISTTConfig::from_base(config);

        Ok(Self {
            config: Some(assemblyai_config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            control_tx: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(AtomicBool::new(false)),
            resilience: None,
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            replay: Arc::new(crate::core::websocket::AudioReplayBuffer::default()),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        self.start_connection(config.clone()).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // W-D1: record the intent BEFORE firing the shutdown signal so the reconnect loop never
        // re-dials on a disconnect that races a server-side close.
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        self.ws_sender = None;
        self.control_tx = None;
        self.result_tx = None;
        self.error_tx = None;
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;
        *self.session_id.write().await = None;
        self.is_connected.store(false, Ordering::Release);

        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from AssemblyAI STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::Acquire) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to AssemblyAI STT".to_string(),
            ));
        }

        // Validate audio chunk size to prevent memory issues
        let data_len = audio_data.len();
        if data_len > MAX_AUDIO_CHUNK_SIZE {
            return Err(STTError::InvalidAudioFormat(format!(
                "Audio chunk size {} bytes exceeds maximum {} bytes",
                data_len, MAX_AUDIO_CHUNK_SIZE
            )));
        }

        if let Some(ws_sender) = &self.ws_sender {
            // Zero-copy - Bytes passed directly to WebSocket
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {e}")))?;

            debug!("Queued {} bytes of audio for AssemblyAI", data_len);
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.lock().await = Some(Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        }));
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.lock().await = Some(Box::new(move |error| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(error).await;
            })
        }));
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.config.as_ref().map(|c| &c.base)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        let existing = self.config.take();

        let assemblyai_config = AssemblyAISTTConfig {
            base: config,
            speech_model: existing
                .as_ref()
                .map(|c| c.speech_model)
                .unwrap_or_default(),
            encoding: existing.as_ref().map(|c| c.encoding).unwrap_or_default(),
            format_turns: existing.as_ref().map(|c| c.format_turns).unwrap_or(true),
            end_of_turn_confidence_threshold: existing
                .as_ref()
                .and_then(|c| c.end_of_turn_confidence_threshold),
            region: existing.as_ref().map(|c| c.region).unwrap_or_default(),
            include_word_timestamps: existing
                .as_ref()
                .map(|c| c.include_word_timestamps)
                .unwrap_or(true),
            keyterms_prompt: existing
                .as_ref()
                .map(|c| c.keyterms_prompt.clone())
                .unwrap_or_default(),
            language_detection: existing
                .as_ref()
                .map(|c| c.language_detection)
                .unwrap_or(false),
            // Preserve advanced streaming knobs across a config update (they were set from the
            // standardized features at session start and must survive a mid-session base swap).
            speaker_labels: existing.as_ref().map(|c| c.speaker_labels).unwrap_or(false),
            max_speakers: existing.as_ref().and_then(|c| c.max_speakers),
            max_turn_silence: existing.as_ref().and_then(|c| c.max_turn_silence),
            min_turn_silence: existing.as_ref().and_then(|c| c.min_turn_silence),
            vad_threshold: existing.as_ref().and_then(|c| c.vad_threshold),
            inactivity_timeout: existing.as_ref().and_then(|c| c.inactivity_timeout),
            domain: existing.as_ref().and_then(|c| c.domain.clone()),
            endpoint_override: existing.as_ref().and_then(|c| c.endpoint_override.clone()),
        };

        self.config = Some(assemblyai_config);

        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "AssemblyAI Streaming STT v3"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared process-global handles; the next `start_connection` uses them so every
        // AssemblyAI session trips the same breaker and shares the one reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

// =============================================================================
// AssemblyAI-Specific Helper Methods
// =============================================================================

impl AssemblyAISTT {
    /// Get the current session ID.
    ///
    /// The session ID is assigned by AssemblyAI when the connection is established.
    /// Returns a cloned String since the session_id is stored in a shared RwLock.
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Force the current utterance to end.
    ///
    /// Sends a `ForceEndpoint` message to AssemblyAI to manually finalize
    /// the current speech segment and return it as a completed turn.
    ///
    /// This is useful when you know the speaker has finished talking
    /// but end-of-turn hasn't been automatically detected.
    ///
    /// # Errors
    ///
    /// Returns an error if not connected or the message fails to send.
    pub async fn force_endpoint(&self) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to AssemblyAI STT".to_string(),
            ));
        }

        let control_tx = self.control_tx.as_ref().ok_or_else(|| {
            STTError::ConnectionFailed("Control channel not available".to_string())
        })?;

        let msg = ForceEndpointMessage::default();
        let json = serde_json::to_string(&msg)
            .map_err(|e| STTError::ProviderError(format!("Failed to serialize message: {e}")))?;

        control_tx
            .send(json)
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to send ForceEndpoint: {e}")))?;

        debug!("Sent ForceEndpoint message to AssemblyAI");
        Ok(())
    }

    /// Update AssemblyAI-specific settings.
    ///
    /// This allows updating AssemblyAI-specific parameters without
    /// affecting the base STT configuration. Requires reconnection.
    pub async fn update_assemblyai_settings(
        &mut self,
        speech_model: Option<AssemblyAISpeechModel>,
        format_turns: Option<bool>,
        region: Option<AssemblyAIRegion>,
    ) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            if let Some(model) = speech_model {
                config.speech_model = model;
            }
            if let Some(turns) = format_turns {
                config.format_turns = turns;
            }
            if let Some(reg) = region {
                config.region = reg;
            }
        }

        self.connect().await
    }

    /// Set end-of-turn detection threshold.
    ///
    /// Controls how aggressively AssemblyAI detects speech boundaries.
    /// Lower values = more aggressive detection.
    pub async fn set_end_of_turn_threshold(&mut self, threshold: f32) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            config.end_of_turn_confidence_threshold = Some(threshold.clamp(0.0, 1.0));
        }

        self.connect().await
    }

    /// Set audio encoding.
    ///
    /// Changes the expected audio encoding format.
    pub async fn set_encoding(&mut self, encoding: AssemblyAIEncoding) -> Result<(), STTError> {
        if self.is_ready() {
            self.disconnect().await?;
        }

        if let Some(config) = &mut self.config {
            config.encoding = encoding;
        }

        self.connect().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_with_valid_config() {
        let config = STTConfig {
            api_key: "test_api_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
            provider: "assemblyai".to_string(),
        };

        let stt = AssemblyAISTT::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
        assert_eq!(stt.get_provider_info(), "AssemblyAI Streaming STT v3");
    }

    #[test]
    fn test_new_with_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "".to_string(),
            provider: "assemblyai".to_string(),
        };

        let stt = AssemblyAISTT::new(config);
        assert!(stt.is_err());

        if let Err(STTError::AuthenticationFailed(msg)) = stt {
            assert!(msg.contains("API key is required"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_default_config_uses_english_model() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let stt = AssemblyAISTT::new(config).unwrap();
        let assemblyai_config = stt.config.as_ref().unwrap();

        assert_eq!(
            assemblyai_config.speech_model,
            AssemblyAISpeechModel::UniversalStreamingEnglish
        );
    }

    #[test]
    fn test_non_english_uses_multilingual_model() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "fr-FR".to_string(),
            sample_rate: 16000,
            // No explicit model → model selected by language (clear the Deepgram default "nova-3").
            model: String::new(),
            ..Default::default()
        };

        let stt = AssemblyAISTT::new(config).unwrap();
        let assemblyai_config = stt.config.as_ref().unwrap();

        assert_eq!(
            assemblyai_config.speech_model,
            AssemblyAISpeechModel::UniversalStreamingMultilingual
        );
    }

    #[test]
    fn test_websocket_url_generation() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let stt = AssemblyAISTT::new(config).unwrap();
        let assemblyai_config = stt.config.as_ref().unwrap();
        let url = assemblyai_config.build_websocket_url();

        assert!(url.starts_with("wss://streaming.assemblyai.com/v3/ws?"));
        assert!(url.contains("sample_rate=16000"));
        assert!(url.contains("encoding=pcm_s16le"));
        assert!(url.contains("speech_model=universal-streaming-english"));
    }

    // W-D1: disconnect() must record intent on the flag shared with the hand-rolled reconnect
    // loop, so a client close racing a server-side close can never trigger a spurious reconnect.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };
        let mut stt = AssemblyAISTT::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the reconnect-loop intentional-disconnect flag",
        );
    }

    #[tokio::test]
    async fn test_send_audio_when_not_connected() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let mut stt = AssemblyAISTT::new(config).unwrap();
        let audio_data = Bytes::from(vec![0u8; 1024]);

        let result = stt.send_audio(audio_data).await;
        assert!(result.is_err());

        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("Not connected"));
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }

    #[tokio::test]
    async fn test_handle_begin_message() {
        let (tx, _rx) = mpsc::channel::<STTResult>(256);
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Begin","id":"test-session-123","expires_at":1704067200}"#.into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id, &crate::core::websocket::AudioReplayBuffer::default()).await;

        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue
        assert_eq!(
            *session_id.read().await,
            Some("test-session-123".to_string())
        );
    }

    #[tokio::test]
    async fn test_handle_turn_message() {
        let (tx, mut rx) = mpsc::channel::<STTResult>(256);
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Turn","turn_order":0,"transcript":"Hello world","end_of_turn":true,"words":[{"start":0,"end":500,"confidence":0.95,"text":"Hello"},{"start":500,"end":1000,"confidence":0.98,"text":"world"}]}"#.into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id, &crate::core::websocket::AudioReplayBuffer::default()).await;

        assert!(result.is_ok());
        assert!(result.unwrap()); // Should continue

        let stt_result = rx.try_recv().unwrap();
        assert_eq!(stt_result.transcript, "Hello world");
        assert!(stt_result.is_final);
        assert!(stt_result.is_speech_final);
        assert!(stt_result.confidence > 0.9);
    }

    #[tokio::test]
    async fn test_handle_termination_message() {
        let (tx, _rx) = mpsc::channel::<STTResult>(256);
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Termination","audio_duration_ms":5000,"terminated_normally":true}"#.into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id, &crate::core::websocket::AudioReplayBuffer::default()).await;

        assert!(result.is_ok());
        assert!(!result.unwrap()); // Should NOT continue (termination)
    }

    #[tokio::test]
    async fn test_handle_error_message() {
        let (tx, _rx) = mpsc::channel::<STTResult>(256);
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Error","error_code":"invalid_api_key","error":"API key is invalid"}"#
                .into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id, &crate::core::websocket::AudioReplayBuffer::default()).await;

        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("invalid"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[tokio::test]
    async fn test_handle_rate_limit_error() {
        let (tx, _rx) = mpsc::channel::<STTResult>(256);
        let session_id = Arc::new(RwLock::new(None));

        let msg = Message::Text(
            r#"{"type":"Error","error_code":"rate_limit_exceeded","error":"Too many requests"}"#
                .into(),
        );

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id, &crate::core::websocket::AudioReplayBuffer::default()).await;

        assert!(result.is_err());
        if let Err(STTError::ProviderError(msg)) = result {
            assert!(msg.contains("Rate limit"));
        } else {
            panic!("Expected ProviderError");
        }
    }
}
