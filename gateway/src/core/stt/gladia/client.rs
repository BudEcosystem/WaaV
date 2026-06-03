//! Gladia STT Client Implementation
//!
//! Implements the BaseSTT trait for Gladia real-time speech-to-text.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTResult,
    STTResultCallback, STTStats,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

use super::config::GladiaSTTConfig;
use super::messages::{
    AudioChunkMessage, InitSessionRequest, InitSessionResponse, ServerMessage, StopRecordingMessage,
};

// =============================================================================
// Type Aliases
// =============================================================================

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;
type WsReadStream = futures_util::stream::SplitStream<WsStream>;

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

// =============================================================================
// Featured session-init helper (shared by connect closure + unit tests)
// =============================================================================

/// Build the `POST /v2/live` session-init request body from the provider config.
///
/// Free function so both `connect`'s reconnect closure and the `build_init_request` test hook
/// produce byte-identical JSON. (Guards the recurring "feature set on the config but never
/// serialized" bug class.)
pub(super) fn build_init_request_from(config: &GladiaSTTConfig) -> InitSessionRequest {
    // Omit post_processing entirely when no post-processing feature is requested, so the default
    // body stays byte-for-byte identical to before this feature was added.
    let post_processing = if config.post_processing.is_empty() {
        None
    } else {
        Some(config.post_processing.clone())
    };
    InitSessionRequest {
        encoding: config.encoding.as_str().to_string(),
        bit_depth: config.bit_depth.value(),
        sample_rate: config.sample_rate,
        channels: config.channels,
        model: Some(config.model.clone()),
        endpointing: Some(config.endpointing),
        maximum_duration_without_endpointing: Some(config.maximum_duration_without_endpointing),
        language_config: Some(config.language_config.clone()),
        pre_processing: Some(config.pre_processing.clone()),
        realtime_processing: Some(config.realtime_processing.clone()),
        post_processing,
        messages_config: Some(config.messages_config.clone()),
        custom_metadata: config.custom_metadata.clone(),
    }
}

/// Initialize a Gladia session via the REST API and return the per-session WebSocket URL.
///
/// This is the **featured handshake**: every Gladia feature rides the `POST /v2/live` init body,
/// which mints a fresh session id + WebSocket URL. A reconnect therefore re-runs this so the
/// restored session is identical to the original.
async fn init_session(
    http_client: &reqwest::Client,
    config: &GladiaSTTConfig,
) -> Result<InitSessionResponse, STTError> {
    let url = config.api_url();
    debug!("Initializing Gladia session at {}", url);

    let request_body = build_init_request_from(config);
    trace!("Session init request: {:?}", request_body);

    let response = http_client
        .post(&url)
        .header("X-Gladia-Key", &config.api_key)
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| STTError::ConnectionFailed(format!("HTTP request failed: {}", e)))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(match status.as_u16() {
            401 => STTError::AuthenticationFailed(format!("Invalid API key: {}", error_text)),
            400 | 422 => STTError::ConfigurationError(format!("Invalid request: {}", error_text)),
            _ => STTError::ConnectionFailed(format!(
                "Session init failed ({}): {}",
                status, error_text
            )),
        });
    }

    let init_response: InitSessionResponse = response
        .json()
        .await
        .map_err(|e| STTError::ConnectionFailed(format!("Failed to parse response: {}", e)))?;

    debug!(
        "Session initialized: id={}, ws_url={}",
        init_response.id, init_response.url
    );

    Ok(init_response)
}

// =============================================================================
// Supervised transport (W-D1 production adoption)
// =============================================================================

/// A [`WsTransport`] that adapts Gladia's streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure.
///
/// Gladia is a **REST-init-then-WebSocket** provider: every feature rides the `POST /v2/live` init
/// body, which mints a per-session WS URL. The reconnect closure re-runs that init (so the restored
/// session is fully featured) and dials the fresh URL; the WebSocket itself takes no post-handshake
/// config message, so [`restore_session`](WsTransport::restore_session) is a no-op. [`run`] IS the
/// original receiver loop, now returning a [`ReconnectOutcome`] so a transport drop reconnects
/// instead of bare-breaking the session.
struct GladiaTransport {
    ws_sink: WsSink,
    ws_stream: WsReadStream,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown signal (fires once; an intentional close must not reconnect).
    shutdown_rx: Arc<Mutex<oneshot::Receiver<()>>>,
    on_result: Arc<RwLock<Option<STTResultCallback>>>,
    on_error: Arc<RwLock<Option<STTErrorCallback>>>,
    stats: Arc<RwLock<STTStats>>,
    bytes_sent: Arc<AtomicU64>,
    /// Fires once on the first successful connect, unblocking `connect`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[async_trait::async_trait]
impl WsTransport for GladiaTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // The featured session was (re)established by the REST init in the connect closure (which
        // minted this connection's URL). The WebSocket itself needs no post-handshake config, so
        // there is nothing to re-send here — just signal the waiting connect() exactly once.
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let mut audio_rx = self.audio_rx.lock().await;
        let mut shutdown_rx = self.shutdown_rx.lock().await;
        loop {
            tokio::select! {
                // Handle outgoing audio data (JSON with base64-encoded chunk, per Gladia docs).
                Some(audio_data) = audio_rx.recv() => {
                    let chunk_msg = AudioChunkMessage::new(&audio_data);
                    let json = match chunk_msg.to_json() {
                        Ok(json) => json,
                        Err(e) => {
                            let stt_error = STTError::InvalidAudioFormat(e.to_string());
                            error!("{}", stt_error);
                            if let Some(cb) = self.on_error.read().await.as_ref() {
                                cb(stt_error).await;
                            }
                            continue;
                        }
                    };
                    if let Err(e) = self.ws_sink.send(Message::Text(json.into())).await {
                        let stt_error = STTError::NetworkError(e.to_string());
                        error!("{}", stt_error);
                        if let Some(cb) = self.on_error.read().await.as_ref() {
                            cb(stt_error).await;
                        }
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                    self.bytes_sent.fetch_add(audio_data.len() as u64, Ordering::Relaxed);
                    self.stats.write().await.total_audio_bytes += audio_data.len() as u64;
                    trace!("Sent {} bytes of audio", audio_data.len());
                }

                // Handle incoming messages with idle timeout.
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            trace!("Received text message: {}", text);
                            match ServerMessage::from_json(&text) {
                                Ok(ServerMessage::Transcript(transcript)) => {
                                    let result = STTResult::new(
                                        transcript.data.utterance.text.clone(),
                                        transcript.data.is_final,
                                        transcript.data.is_final,
                                        transcript.data.utterance.confidence as f32,
                                    );
                                    self.stats.write().await.update_with_result(&result);
                                    if let Some(callback) = self.on_result.read().await.as_ref() {
                                        callback(result).await;
                                    }
                                }
                                Ok(ServerMessage::Error(err)) => {
                                    error!("Gladia error: {}", err);
                                    if let Some(callback) = self.on_error.read().await.as_ref() {
                                        callback(STTError::ProviderError(err.message)).await;
                                    }
                                }
                                Ok(ServerMessage::Unknown(msg_type)) => {
                                    debug!("Unknown message type: {}", msg_type);
                                }
                                Err(e) => {
                                    warn!("Failed to parse message: {}", e);
                                }
                            }
                        }
                        Ok(Some(Ok(Message::Binary(data)))) => {
                            trace!("Received binary message: {} bytes", data.len());
                        }
                        Ok(Some(Ok(Message::Close(frame)))) => {
                            info!("WebSocket closed: {:?}", frame);
                            // Server closed mid-stream — reconnect to preserve the session.
                            return ReconnectOutcome::Reconnectable(StreamError::new("server close"));
                        }
                        Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {
                            // Handled by tungstenite.
                        }
                        Ok(Some(Ok(Message::Frame(_)))) => {
                            // Raw frame, ignore.
                        }
                        Ok(Some(Err(e))) => {
                            error!("WebSocket error: {}", e);
                            if let Some(callback) = self.on_error.read().await.as_ref() {
                                callback(STTError::NetworkError(e.to_string())).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Gladia WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "Gladia WebSocket idle timeout - no message for 60 seconds".into(),
                            );
                            error!("Gladia STT idle timeout: {}", stt_error);
                            if let Some(callback) = self.on_error.read().await.as_ref() {
                                callback(stt_error).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect).
                _ = &mut *shutdown_rx => {
                    info!("Gladia: Received shutdown signal");
                    let stop_msg = StopRecordingMessage::new();
                    if let Ok(json) = stop_msg.to_json() {
                        let _ = self.ws_sink.send(Message::Text(json.into())).await;
                    }
                    let _ = self.ws_sink.close().await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

// =============================================================================
// GladiaSTT Implementation
// =============================================================================

/// Gladia STT client implementing BaseSTT trait
pub struct GladiaSTT {
    /// Provider-specific configuration
    gladia_config: GladiaSTTConfig,
    /// Base STT configuration (stored for get_config)
    base_config: Option<STTConfig>,
    /// Current connection state
    state: Arc<RwLock<STTConnectionState>>,
    /// Audio sender (bounded channel for backpressure); the supervised transport drains it.
    ws_sender: Option<mpsc::Sender<Bytes>>,
    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Connection task handle (the supervisor's outer reconnect loop).
    connection_handle: Option<tokio::task::JoinHandle<()>>,
    /// Session ID from the most recent initialization.
    session_id: Arc<RwLock<Option<String>>>,
    /// Result callback
    on_result: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback
    on_error: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Ready flag
    is_ready: Arc<AtomicBool>,
    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before firing `shutdown_tx`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,
    /// Statistics
    stats: Arc<RwLock<STTStats>>,
    /// Bytes sent counter
    bytes_sent: Arc<AtomicU64>,
    /// HTTP client for session initialization
    http_client: reqwest::Client,
    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl GladiaSTT {
    /// W1 keystone — construct directly from the standardized config so Gladia's nested feature
    /// surface (interim results, word timestamps, custom vocabulary/keyterms, named-entity
    /// recognition, automatic language detection) is honored END-TO-END. The flat `BaseSTT::new`
    /// path cannot reach those knobs; this is the reachable standardized path mirroring
    /// `DeepgramSTT::new_standard`. The api_key is resolved by `from_standard`/`from_base` (config
    /// or `GLADIA_API_KEY` env var) and validated by `with_config`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let gladia_config = GladiaSTTConfig::from_standard(std)?;
        Self::with_config(gladia_config)
    }

    /// Create a new Gladia STT client from provider-specific config
    pub fn with_config(config: GladiaSTTConfig) -> Result<Self, STTError> {
        // Validate configuration
        config.validate()?;

        Ok(Self {
            gladia_config: config,
            base_config: None,
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            http_client: reqwest::Client::new(),
            resilience: None,
        })
    }

    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `GladiaSTT` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }

    /// Build the `POST /v2/live` session-init request body from the provider config.
    ///
    /// Test hook kept for the wire-level feature tests; delegates to the free
    /// [`build_init_request_from`] the connect closure uses, so they stay byte-identical.
    pub(super) fn build_init_request(&self) -> InitSessionRequest {
        build_init_request_from(&self.gladia_config)
    }
}

#[async_trait::async_trait]
impl BaseSTT for GladiaSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        // Check for API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Gladia API key is required. Set api_key or GLADIA_API_KEY env var".to_string(),
            ));
        }

        let gladia_config = GladiaSTTConfig::from_base(&config)?;

        Ok(Self {
            gladia_config,
            base_config: Some(config),
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            http_client: reqwest::Client::new(),
            resilience: None,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Check if already connected
        {
            let current_state = self.state.read().await;
            if matches!(*current_state, STTConnectionState::Connected) {
                return Ok(());
            }
        }
        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        *self.state.write().await = STTConnectionState::Connecting;
        info!("Connecting to Gladia STT...");

        // Create channels for communication (bounded for backpressure on audio).
        let (ws_tx, ws_rx) = mpsc::channel::<Bytes>(32);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(ws_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Shared state the supervised transport re-uses across reconnect attempts.
        let audio_rx = Arc::new(Mutex::new(ws_rx));
        let shutdown_rx = Arc::new(Mutex::new(shutdown_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        let on_result = Arc::clone(&self.on_result);
        let on_error = Arc::clone(&self.on_error);
        let stats = Arc::clone(&self.stats);
        let bytes_sent = Arc::clone(&self.bytes_sent);
        let session_id = Arc::clone(&self.session_id);
        let http_client = self.http_client.clone();
        let gladia_config = self.gladia_config.clone();

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor with
        // the shared process-global handles from CoreState (W-D1/W-D2 fleet adoption). When no
        // handles were injected, the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("gladia", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("gladia", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure re-runs the REST session-init (every feature rides that body) to mint a fresh
        // featured WS URL, dials it, and hands back a transport whose `run()` is the original
        // Gladia receiver loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let http_client = http_client.clone();
                    let gladia_config = gladia_config.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_rx = Arc::clone(&shutdown_rx);
                    let connected_tx = Arc::clone(&connected_tx);
                    let on_result = Arc::clone(&on_result);
                    let on_error = Arc::clone(&on_error);
                    let stats = Arc::clone(&stats);
                    let bytes_sent = Arc::clone(&bytes_sent);
                    let session_id = Arc::clone(&session_id);
                    async move {
                        // Featured handshake: REST init mints a fresh session id + WS URL.
                        let init_response = init_session(&http_client, &gladia_config)
                            .await
                            .map_err(|e| StreamError::new(e.to_string()))?;
                        *session_id.write().await = Some(init_response.id.clone());

                        debug!("Connecting to WebSocket: {}", init_response.url);
                        let (ws_stream, _) =
                            connect_async(&init_response.url).await.map_err(|e| {
                                StreamError::new(format!("WebSocket connection failed: {e}"))
                            })?;
                        info!("Connected to Gladia STT (session: {})", init_response.id);
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(GladiaTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_rx,
                            on_result,
                            on_error,
                            stats,
                            bytes_sent,
                            connected_tx,
                        })
                    }
                })
                .await;
            info!("Gladia STT WebSocket connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Wait for the first successful connect (restore_session fires the connected signal).
        match timeout(Duration::from_secs(15), connected_rx).await {
            Ok(Ok(())) => {
                *self.state.write().await = STTConnectionState::Connected;
                self.is_ready.store(true, Ordering::SeqCst);
                info!("Connected to Gladia STT");
                Ok(())
            }
            Ok(Err(_)) => {
                *self.state.write().await =
                    STTConnectionState::Error("connection channel closed".to_string());
                Err(STTError::ConnectionFailed(
                    "Connection channel closed before Gladia session started".to_string(),
                ))
            }
            Err(_) => {
                *self.state.write().await =
                    STTConnectionState::Error("connection timeout".to_string());
                Err(STTError::ConnectionFailed(
                    "Connection timeout waiting for Gladia session".to_string(),
                ))
            }
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE any further work so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        info!("Disconnecting from Gladia STT...");

        // Signal the supervised transport to send StopRecording + close intentionally (no
        // reconnect).
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // Clear state
        self.ws_sender = None;
        *self.session_id.write().await = None;
        *self.state.write().await = STTConnectionState::Disconnected;
        self.is_ready.store(false, Ordering::SeqCst);

        info!("Disconnected from Gladia STT");
        Ok(())
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        if let Some(ws_sender) = &self.ws_sender {
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(e.to_string()))?;
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.on_result.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.on_error.write().await = Some(callback);
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.base_config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Update the gladia config
        let new_gladia_config = GladiaSTTConfig::from_base(&config)?;
        self.gladia_config = new_gladia_config;
        self.base_config = Some(config);

        // If connected, we need to reconnect with the new config
        if self.is_ready() {
            self.disconnect().await?;
            self.connect().await?;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Gladia STT (solaria-1)"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect` drives the generic
        // ReconnectableStream supervisor with them — every Gladia session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl Drop for GladiaSTT {
    fn drop(&mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        if let Some(handle) = self.connection_handle.take() {
            handle.abort();
        }
    }
}

// =============================================================================
// Factory Function
// =============================================================================

/// Create a new Gladia STT provider from base configuration
pub fn create_gladia_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    // Check for API key
    if config.api_key.is_empty() {
        return Err(STTError::AuthenticationFailed(
            "Gladia API key is required. Set api_key or GLADIA_API_KEY env var".to_string(),
        ));
    }

    let stt = GladiaSTT::new(config)?;
    Ok(Box::new(stt))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_gladia_config() -> GladiaSTTConfig {
        GladiaSTTConfig::new("test-api-key")
    }

    fn create_test_base_config() -> STTConfig {
        STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            provider: "gladia".to_string(),
            ..Default::default()
        }
    }

    // W1 keystone: advanced features set on the standardized config must survive through
    // `new_standard` into the nested Gladia provider config (previously dropped by the flat
    // factory).
    #[test]
    fn new_standard_propagates_advanced_features() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "gladia".into(),
                api_key: "test-api-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Gladia".into()]),
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let stt = GladiaSTT::new_standard(&std).expect("new_standard should succeed");
        assert!(stt.gladia_config.realtime_processing.words_accurate_timestamps);
        assert_eq!(
            stt.gladia_config.realtime_processing.custom_vocabulary,
            vec!["WaaV", "Gladia"]
        );
    }

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close can never trigger a spurious reconnect (the supervisor's loop-top guard
    // observes this same `Arc<AtomicBool>`). Before this wiring the flag was the supervisor's own
    // and disconnect() never set it.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = create_test_base_config();
        let mut stt = GladiaSTT::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    // ===================== WIRE-LEVEL feature tests (session-init body) =====================
    //
    // These build the EXACT `POST /v2/live` JSON body via `build_init_request` (the same
    // builder `init_session` posts) and assert each newly-wired feature reaches the wire —
    // not merely the config struct. Construction goes through `new_standard`, the reachable
    // standardized path, so the assertion covers from_standard -> config -> body end-to-end.

    use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};

    fn std_with_extras(extras: serde_json::Value) -> StandardSTTConfig {
        StandardSTTConfig {
            base: STTConfig {
                provider: "gladia".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: Default::default(),
            extras: ProviderExtras(extras.as_object().unwrap().clone()),
        }
    }

    /// Build the session-init body JSON for a Gladia provider built from the given extras.
    fn init_body_json(extras: serde_json::Value) -> serde_json::Value {
        let stt = GladiaSTT::new_standard(&std_with_extras(extras)).expect("new_standard");
        serde_json::to_value(stt.build_init_request()).expect("serialize body")
    }

    #[test]
    fn wire_custom_spelling_dictionary_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "custom_spelling_dictionary": { "WaaV": ["wave", "wav"], "Gladia": ["gladiya"] }
        }));
        let rp = &body["realtime_processing"];
        assert_eq!(rp["custom_spelling"], true, "custom_spelling flag missing: {body}");
        assert_eq!(
            rp["custom_spelling_config"]["spelling_dictionary"]["WaaV"],
            serde_json::json!(["wave", "wav"]),
            "spelling_dictionary entry missing on wire: {body}"
        );
    }

    #[test]
    fn wire_custom_vocabulary_config_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "custom_vocabulary_config": {
                "vocabulary": [
                    { "value": "WaaV", "intensity": 0.8, "pronunciations": ["wave"], "language": "en" }
                ],
                "default_intensity": 0.5
            }
        }));
        let cvc = &body["realtime_processing"]["custom_vocabulary_config"];
        // f32 round-trips with widening, so compare numerics with a tolerance.
        let default_intensity = cvc["default_intensity"].as_f64().expect("default_intensity");
        assert!(
            (default_intensity - 0.5).abs() < 1e-6,
            "default_intensity missing on wire: {body}"
        );
        assert_eq!(cvc["vocabulary"][0]["value"], "WaaV");
        let intensity = cvc["vocabulary"][0]["intensity"].as_f64().expect("intensity");
        assert!((intensity - 0.8).abs() < 1e-6, "intensity missing on wire: {body}");
        assert_eq!(cvc["vocabulary"][0]["pronunciations"], serde_json::json!(["wave"]));
        assert_eq!(cvc["vocabulary"][0]["language"], "en");
    }

    #[test]
    fn wire_translation_config_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "translation_config": {
                "model": "enhanced",
                "match_original_utterances": true,
                "lipsync": true,
                "context_adaptation": true,
                "context": "medical",
                "informal": false
            }
        }));
        let tc = &body["realtime_processing"]["translation_config"];
        assert_eq!(tc["model"], "enhanced", "translation model missing: {body}");
        assert_eq!(tc["match_original_utterances"], true);
        assert_eq!(tc["lipsync"], true);
        assert_eq!(tc["context_adaptation"], true);
        assert_eq!(tc["context"], "medical");
        assert_eq!(tc["informal"], false);
    }

    #[test]
    fn wire_summarization_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "summarization": true,
            "summarization_config": { "type": "bullet_points" }
        }));
        let pp = &body["post_processing"];
        assert_eq!(pp["summarization"], true, "summarization flag missing: {body}");
        assert_eq!(
            pp["summarization_config"]["type"], "bullet_points",
            "summarization type missing on wire: {body}"
        );
    }

    #[test]
    fn wire_chapterization_reaches_body() {
        let body = init_body_json(serde_json::json!({ "chapterization": true }));
        assert_eq!(
            body["post_processing"]["chapterization"], true,
            "chapterization missing on wire: {body}"
        );
    }

    #[test]
    fn wire_message_toggles_reach_body() {
        let body = init_body_json(serde_json::json!({
            "receive_acknowledgments": true,
            "receive_errors": true,
            "receive_lifecycle_events": true
        }));
        let mc = &body["messages_config"];
        assert_eq!(mc["receive_acknowledgments"], true, "acks missing: {body}");
        assert_eq!(mc["receive_errors"], true, "errors toggle missing: {body}");
        assert_eq!(
            mc["receive_lifecycle_events"], true,
            "lifecycle toggle missing: {body}"
        );
    }

    #[test]
    fn wire_defaults_omit_post_processing_and_leave_toggles_off() {
        // No extras: post_processing must be omitted entirely and message toggles default off.
        let body = init_body_json(serde_json::json!({}));
        assert!(
            body.get("post_processing").is_none() || body["post_processing"].is_null(),
            "post_processing must be omitted by default: {body}"
        );
        let mc = &body["messages_config"];
        assert_eq!(mc["receive_acknowledgments"], false);
        assert_eq!(mc["receive_errors"], false);
        assert_eq!(mc["receive_lifecycle_events"], false);
        let rp = &body["realtime_processing"];
        assert_eq!(rp["custom_spelling"], false);
        assert!(rp.get("custom_spelling_config").is_none() || rp["custom_spelling_config"].is_null());
        assert!(rp.get("translation_config").is_none() || rp["translation_config"].is_null());
        assert!(
            rp.get("custom_vocabulary_config").is_none() || rp["custom_vocabulary_config"].is_null()
        );
    }

    #[test]
    fn test_gladia_stt_with_config() {
        let config = create_test_gladia_config();
        let stt = GladiaSTT::with_config(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Gladia STT (solaria-1)");
    }

    #[test]
    fn test_gladia_stt_with_empty_api_key() {
        let config = GladiaSTTConfig::default();
        let stt = GladiaSTT::with_config(config);
        assert!(stt.is_err());
    }

    #[test]
    fn test_gladia_stt_new() {
        let config = create_test_base_config();
        let stt = GladiaSTT::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[test]
    fn test_gladia_stt_new_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };
        let stt = GladiaSTT::new(config);
        assert!(stt.is_err());
        if let Err(err) = stt {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    #[test]
    fn test_create_gladia_stt_factory() {
        let config = create_test_base_config();
        let result = create_gladia_stt(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Gladia STT (solaria-1)");
    }

    #[test]
    fn test_create_gladia_stt_factory_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = create_gladia_stt(config);
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    #[tokio::test]
    async fn test_gladia_stt_initial_state() {
        let config = create_test_base_config();
        let stt = GladiaSTT::new(config).unwrap();

        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[tokio::test]
    async fn test_gladia_stt_send_audio_not_connected() {
        let config = create_test_base_config();
        let mut stt = GladiaSTT::new(config).unwrap();

        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::ConnectionFailed(_)));
        }
    }

    #[tokio::test]
    async fn test_gladia_stt_callbacks() {
        use std::sync::atomic::AtomicUsize;

        let config = create_test_base_config();
        let mut stt = GladiaSTT::new(config).unwrap();

        let result_count = Arc::new(AtomicUsize::new(0));
        let result_count_clone = result_count.clone();

        let callback = Arc::new(move |_result: STTResult| {
            let count = result_count_clone.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_gladia_stt_get_config() {
        let config = create_test_base_config();
        let stt = GladiaSTT::new(config.clone()).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "test-api-key");
        assert_eq!(stored_config.language, "en");
        assert_eq!(stored_config.sample_rate, 16000);
    }
}
