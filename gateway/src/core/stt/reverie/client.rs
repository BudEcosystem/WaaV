//! Reverie STT Client Implementation
//!
//! Implements the BaseSTT trait for Reverie real-time speech-to-text.

use bytes::Bytes;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::core::resilience::connect::{WS_CONNECT_TIMEOUT, with_timeout};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTResult,
    STTResultCallback, STTStats,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

use super::EOF_MARKER;
use super::config::ReverieSTTConfig;
use super::messages::ReverieServerMessage;

// =============================================================================
// Constants
// =============================================================================

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

// =============================================================================
// Type Aliases
// =============================================================================

/// The concrete WebSocket stream type Reverie dials.
type ReverieWs = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

// =============================================================================
// ReverieTransport (W-D1 reconnect supervisor adoption)
// =============================================================================

/// A [`WsTransport`] that adapts Reverie's streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). Like Cartesia, Reverie carries every
/// feature (api_key, app_id, language, domain, continuous, timeout, audio format) in the connect
/// URL, so [`restore_session`](WsTransport::restore_session) is a no-op — a fresh dial already
/// restored the featured session. [`run`](WsTransport::run) IS the original receiver loop (now
/// also owning the audio-send half), returning a [`ReconnectOutcome`] so a mid-stream transport
/// drop reconnects instead of bare-breaking and ending the session.
struct ReverieTransport {
    ws_sink: SplitSink<ReverieWs, Message>,
    ws_stream: SplitStream<ReverieWs>,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown token (fires once; an intentional close must not reconnect).
    shutdown_token: CancellationToken,
    /// Fires once on the first successful connect, unblocking `connect`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    on_result: Arc<RwLock<Option<STTResultCallback>>>,
    on_error: Arc<RwLock<Option<STTErrorCallback>>>,
    session_id: Arc<RwLock<Option<String>>>,
    stats: Arc<RwLock<STTStats>>,
    /// Whether Reverie is in continuous mode (a final result does NOT end the session).
    continuous: bool,
}

impl ReverieTransport {
    async fn send_eof_and_close(ws_sink: &mut SplitSink<ReverieWs, Message>) {
        if let Err(e) = ws_sink
            .send(Message::Binary(EOF_MARKER.to_vec().into()))
            .await
        {
            warn!("Failed to send EOF marker: {}", e);
        }
        let _ = ws_sink.close().await;
    }
}

#[async_trait::async_trait]
impl WsTransport for ReverieTransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Reverie puts every feature in the connect URL, so a fresh dial already restored the
        // featured session — nothing to re-send. Signal the waiting connect() exactly once.
        if let Some(tx) = self.connected_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }

    async fn run(&mut self) -> ReconnectOutcome {
        let mut audio_rx = self.audio_rx.lock().await;
        let shutdown_token = self.shutdown_token.clone();
        loop {
            if shutdown_token.is_cancelled() {
                info!("Received shutdown signal for Reverie STT");
                Self::send_eof_and_close(&mut self.ws_sink).await;
                return ReconnectOutcome::Completed;
            }

            tokio::select! {
                // Handle outgoing audio data (raw binary, as Reverie expects)
                Some(audio_data) = audio_rx.recv() => {
                    if let Err(e) = self
                        .ws_sink
                        .send(Message::Binary(audio_data.to_vec().into()))
                        .await
                    {
                        let stt_error = STTError::NetworkError(e.to_string());
                        error!("Failed to send audio to Reverie: {}", stt_error);
                        if let Some(callback) = self.on_error.read().await.as_ref() {
                            callback(stt_error).await;
                        }
                        // Transport-level send failure: reconnect to preserve the session.
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                }

                // Handle incoming messages with idle timeout
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            trace!("Received text message: {}", text);
                            let server_msg = ReverieServerMessage::from_json(&text);
                            match server_msg {
                                ReverieServerMessage::Partial(result)
                                | ReverieServerMessage::Final(result) => {
                                    if let Some(id) = &result.id {
                                        *self.session_id.write().await = Some(id.clone());
                                    }
                                    let stt_result = STTResult::new(
                                        result.best_text().unwrap_or("").to_string(),
                                        result.r#final,
                                        result.r#final,
                                        result.confidence_f64().unwrap_or(0.0) as f32,
                                    );
                                    {
                                        let mut s = self.stats.write().await;
                                        s.update_with_result(&stt_result);
                                    }
                                    if let Some(callback) = self.on_result.read().await.as_ref() {
                                        callback(stt_result).await;
                                    }
                                    // A final result in non-continuous mode is the provider
                                    // signalling end-of-session — intentional completion, NOT a drop.
                                    if result.should_close(self.continuous) {
                                        debug!("Final result received, closing connection");
                                        return ReconnectOutcome::Completed;
                                    }
                                }
                                ReverieServerMessage::Error(err_msg) => {
                                    // A provider error frame is typically fatal (bad config) — don't
                                    // hammer it with reconnects.
                                    error!("Reverie error: {}", err_msg);
                                    if let Some(callback) = self.on_error.read().await.as_ref() {
                                        callback(STTError::ProviderError(err_msg)).await;
                                    }
                                    return ReconnectOutcome::Fatal(StreamError::new("provider error frame"));
                                }
                                ReverieServerMessage::Unknown(msg) => {
                                    warn!("Unknown message: {}", msg);
                                }
                            }
                        }
                        Ok(Some(Ok(Message::Binary(data)))) => {
                            trace!("Received binary message: {} bytes", data.len());
                        }
                        Ok(Some(Ok(Message::Close(frame)))) => {
                            // Server closed the connection mid-stream — reconnect to preserve the
                            // session (a client-initiated close arrives via the shutdown branch).
                            info!("Reverie WebSocket closed by server: {:?}", frame);
                            return ReconnectOutcome::Reconnectable(StreamError::new("server close"));
                        }
                        Ok(Some(Ok(Message::Ping(_)))) | Ok(Some(Ok(Message::Pong(_)))) => {
                            // Handled by tungstenite
                        }
                        Ok(Some(Ok(Message::Frame(_)))) => {
                            // Raw frame, ignore
                        }
                        Ok(Some(Err(e))) => {
                            let stt_error = STTError::NetworkError(e.to_string());
                            error!("Reverie WebSocket error: {}", e);
                            if let Some(callback) = self.on_error.read().await.as_ref() {
                                callback(stt_error).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Reverie WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "Reverie WebSocket idle timeout - no message for 60 seconds".into(),
                            );
                            error!("Reverie STT idle timeout: {}", stt_error);
                            if let Some(callback) = self.on_error.read().await.as_ref() {
                                callback(stt_error).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect)
                _ = shutdown_token.cancelled() => {
                    info!("Received shutdown signal for Reverie STT");
                    Self::send_eof_and_close(&mut self.ws_sink).await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

// =============================================================================
// ReverieSTT Implementation
// =============================================================================

/// Reverie STT client implementing BaseSTT trait
pub struct ReverieSTT {
    /// Provider-specific configuration
    reverie_config: ReverieSTTConfig,
    /// Base STT configuration (stored for get_config)
    base_config: Option<STTConfig>,
    /// Current connection state
    state: Arc<RwLock<STTConnectionState>>,
    /// WebSocket sender for audio data (bounded channel for backpressure)
    ws_sender: Option<mpsc::Sender<Bytes>>,
    /// Shutdown signal token
    shutdown_token: Option<CancellationToken>,
    /// Connection task handle (the reconnect supervisor)
    connection_handle: Option<tokio::task::JoinHandle<()>>,
    /// Session ID from server
    session_id: Arc<RwLock<Option<String>>>,
    /// Result callback
    on_result: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback
    on_error: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Ready flag
    is_ready: Arc<AtomicBool>,
    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling `shutdown_token`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,
    /// Statistics
    stats: Arc<RwLock<STTStats>>,
    /// Bytes sent counter
    bytes_sent: Arc<AtomicU64>,
    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl ReverieSTT {
    /// Create a new Reverie STT client from provider-specific config
    pub fn with_config(config: ReverieSTTConfig) -> Result<Self, STTError> {
        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        Ok(Self {
            reverie_config: config,
            base_config: None,
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            resilience: None,
        })
    }

    /// W1 keystone — construct directly from the standardized config. Reverie exposes no
    /// advanced-feature query knobs, so this is a uniform standardized entry point that delegates
    /// to `ReverieSTTConfig::from_standard` (a `from_base` passthrough) and then `with_config`.
    /// Mirrors `DeepgramSTT::new_standard`: validate the api_key, then build from the
    /// standardized->provider config mapping.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        let cfg = crate::core::stt::reverie::config::ReverieSTTConfig::from_standard(std)
            .map_err(STTError::ConfigurationError)?;
        Self::with_config(cfg)
    }

    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `ReverieSTT` built
    /// from the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }
}

#[async_trait::async_trait]
impl BaseSTT for ReverieSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        // Check for API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Reverie API key is required. Set api_key or REVERIE_API_KEY env var".to_string(),
            ));
        }

        let reverie_config =
            ReverieSTTConfig::from_base(&config).map_err(STTError::ConfigurationError)?;

        Ok(Self {
            reverie_config,
            base_config: Some(config),
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
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

        // Update state to connecting
        *self.state.write().await = STTConnectionState::Connecting;
        info!("Connecting to Reverie STT...");

        // Build WebSocket URL with all params (every feature is in the URL).
        let url = self.reverie_config.build_websocket_url();
        debug!(
            "WebSocket URL: {}",
            url.replace(&self.reverie_config.api_key, "[REDACTED]")
        );

        // Channels for communication (bounded for backpressure on audio).
        let (ws_tx, ws_rx) = mpsc::channel::<Bytes>(32);
        let shutdown_token = CancellationToken::new();
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(ws_tx);
        self.shutdown_token = Some(shutdown_token.clone());

        // Shared state the supervised transport re-uses across reconnect attempts: a single-
        // consumer audio receiver + shutdown token and the one-shot connected
        // signal that fires on the first successful connect.
        let audio_rx = Arc::new(Mutex::new(ws_rx));
        let connected_tx = Arc::new(Mutex::new(Some(connected_tx)));

        let on_result = self.on_result.clone();
        let on_error = self.on_error.clone();
        let session_id = self.session_id.clone();
        let stats = self.stats.clone();
        let continuous = self.reverie_config.continuous;

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor (the
        // same one the chaos tests exercise) with the shared process-global handles from CoreState
        // (W-D1/W-D2 fleet adoption). When no handles were injected (a direct unit-test
        // construction), the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("reverie", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => {
                ReconnectableStream::new(ReconnectableStreamConfig::new("reverie", reconnection))
            }
        }
        .with_disconnect_flag(disconnect_flag);

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure dials the *featured* URL (every feature is in the URL) and hands back a transport
        // whose `run()` is the original Reverie event loop.
        let state_handle = self.state.clone();
        let is_ready_handle = self.is_ready.clone();
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let url = url.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let on_result = on_result.clone();
                    let on_error = on_error.clone();
                    let session_id = session_id.clone();
                    let stats = stats.clone();
                    async move {
                        let (ws_stream, response) =
                            with_timeout(WS_CONNECT_TIMEOUT, connect_async(&url))
                                .await
                                .map_err(|_| {
                                    StreamError::new(format!(
                                        "connect to Reverie timed out after {}s",
                                        WS_CONNECT_TIMEOUT.as_secs()
                                    ))
                                })?
                                .map_err(|e| {
                                    StreamError::new(format!("Failed to connect to Reverie: {e}"))
                                })?;
                        debug!(
                            "Reverie WebSocket connected, response status: {:?}",
                            response.status()
                        );
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(ReverieTransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_token,
                            connected_tx,
                            on_result,
                            on_error,
                            session_id,
                            stats,
                            continuous,
                        })
                    }
                })
                .await;
            info!("Reverie STT WebSocket connection closed (supervisor exit: {exit:?})");
            is_ready_handle.store(false, Ordering::SeqCst);
            *state_handle.write().await = STTConnectionState::Disconnected;
        });

        self.connection_handle = Some(connection_handle);

        // Wait for connection to be established with timeout.
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                *self.state.write().await = STTConnectionState::Connected;
                self.is_ready.store(true, Ordering::SeqCst);
                info!(
                    "Connected to Reverie STT (lang: {}, domain: {})",
                    self.reverie_config.language, self.reverie_config.domain
                );
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Reverie connection channel closed".to_string();
                *self.state.write().await = STTConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Reverie connection timeout".to_string();
                *self.state.write().await = STTConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE cancelling the shutdown token so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        info!("Disconnecting from Reverie STT...");

        // Send shutdown signal (the supervised run loop sends the EOF marker and closes the
        // socket; an intentional close must NOT reconnect).
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }

        // Wait for the supervisor task to finish with a timeout.
        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "reverie-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        // Clear state
        self.ws_sender = None;
        *self.session_id.write().await = None;
        *self.state.write().await = STTConnectionState::Disconnected;
        self.is_ready.store(false, Ordering::SeqCst);

        info!("Disconnected from Reverie STT");
        Ok(())
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        let ws_sender = self
            .ws_sender
            .as_ref()
            .ok_or_else(|| STTError::ConnectionFailed("Not connected".to_string()))?;

        let data_len = audio_data.len();
        // Queue audio for the supervised send loop (raw binary on the wire).
        ws_sender
            .send(audio_data)
            .await
            .map_err(|e| STTError::NetworkError(e.to_string()))?;

        // Update stats
        self.bytes_sent
            .fetch_add(data_len as u64, Ordering::Relaxed);
        {
            let mut stats = self.stats.write().await;
            stats.total_audio_bytes += data_len as u64;
        }

        trace!("Sent {} bytes of audio", data_len);
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
        // Update the reverie config
        let new_reverie_config =
            ReverieSTTConfig::from_base(&config).map_err(STTError::ConfigurationError)?;
        self.reverie_config = new_reverie_config;
        self.base_config = Some(config);

        // If connected, we need to reconnect with the new config
        if self.is_ready() {
            self.disconnect().await?;
            self.connect().await?;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Reverie STT (Indian Languages)"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect` drives the generic
        // ReconnectableStream supervisor with them — every Reverie session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl Drop for ReverieSTT {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        // Send shutdown signal if still connected.
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
        // Abort the supervisor task.
        if let Some(handle) = self.connection_handle.take() {
            handle.abort();
        }
    }
}

// =============================================================================
// Factory Function
// =============================================================================

/// Create a new Reverie STT provider from base configuration
pub fn create_reverie_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    // Check for API key
    if config.api_key.is_empty() {
        return Err(STTError::AuthenticationFailed(
            "Reverie API key is required. Set api_key or REVERIE_API_KEY env var".to_string(),
        ));
    }

    let stt = ReverieSTT::new(config)?;
    Ok(Box::new(stt))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_reverie_config() -> ReverieSTTConfig {
        ReverieSTTConfig::new("test-api-key", "test-app-id")
    }

    fn create_test_base_config() -> STTConfig {
        STTConfig {
            provider: "reverie".to_string(),
            api_key: "test-api-key".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(), // app_id goes in model field
        }
    }

    #[test]
    fn test_reverie_stt_with_config() {
        let config = create_test_reverie_config();
        let stt = ReverieSTT::with_config(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Reverie STT (Indian Languages)");
    }

    // W1 keystone: Reverie maps zero standardized feature knobs, so `new_standard` is a pure
    // passthrough of the base config — assert the base (api_key + app_id from the model field +
    // language) survives through the provider-struct method to the provider-specific config.
    #[test]
    fn test_reverie_new_standard_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: create_test_base_config(),
            features: SttFeatures {
                // None of these can map to a real Reverie field; they must be ignored.
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = ReverieSTT::new_standard(&std).expect("new_standard must succeed");
        assert_eq!(stt.reverie_config.api_key, "test-api-key");
        assert_eq!(stt.reverie_config.app_id, "test-app-id"); // parsed from model field
        assert_eq!(
            stt.reverie_config.language,
            super::super::config::ReverieLanguage::Hindi
        );

        // Missing api_key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            provider: "reverie".into(),
            api_key: String::new(),
            model: "test-app-id".into(),
            ..Default::default()
        });
        assert!(ReverieSTT::new_standard(&bad).is_err());
    }

    #[test]
    fn test_new_standard_rejects_ssrf_endpoint_override() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};

        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS") };

        let mk = |endpoint: &str| {
            StandardSTTConfig {
                base: STTConfig {
                    provider: "reverie".into(),
                    api_key: "test-api-key".into(),
                    language: "hi".into(),
                    model: "test-app-id".into(),
                    ..Default::default()
                },
                features: SttFeatures::default(),
                extras: ProviderExtras::default(),
                translation: None,
            }
            .with_endpoint_override(endpoint)
        };

        assert!(ReverieSTT::new_standard(&mk("wss://reverie-proxy.example.com")).is_ok());
        assert!(ReverieSTT::new_standard(&mk("ws://127.0.0.1:9000")).is_err());
        assert!(ReverieSTT::new_standard(&mk("file:///tmp/socket")).is_err());
        assert!(ReverieSTT::new_standard(&mk("https://reverie-proxy.example.com")).is_err());

        // SAFETY: restore the process env before releasing the test env lock.
        unsafe {
            if let Some(previous) = previous {
                std::env::set_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS", previous);
            } else {
                std::env::remove_var("WAAV_ALLOW_LOOPBACK_ENDPOINTS");
            }
        }
    }

    #[test]
    fn test_reverie_stt_with_empty_api_key() {
        let config = ReverieSTTConfig::default();
        let stt = ReverieSTT::with_config(config);
        assert!(stt.is_err());
    }

    #[test]
    fn test_reverie_stt_new() {
        let config = create_test_base_config();
        let stt = ReverieSTT::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[test]
    fn test_reverie_stt_new_empty_api_key() {
        let config = STTConfig {
            provider: "reverie".to_string(),
            api_key: String::new(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(),
        };
        let stt = ReverieSTT::new(config);
        assert!(stt.is_err());
        if let Err(err) = stt {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    #[test]
    fn test_reverie_stt_new_missing_app_id() {
        let config = STTConfig {
            provider: "reverie".to_string(),
            api_key: "test-key".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: String::new(), // No app_id
        };
        // This will try to read from REVERIE_APP_ID env var, which won't be set in tests
        let stt = ReverieSTT::new(config);
        assert!(stt.is_err());
        if let Err(err) = stt {
            assert!(matches!(err, STTError::ConfigurationError(_)));
        }
    }

    #[test]
    fn test_create_reverie_stt_factory() {
        let config = create_test_base_config();
        let result = create_reverie_stt(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Reverie STT (Indian Languages)");
    }

    #[test]
    fn test_create_reverie_stt_factory_empty_api_key() {
        let config = STTConfig {
            provider: "reverie".to_string(),
            api_key: String::new(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(),
        };

        let result = create_reverie_stt(config);
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close can never trigger a spurious reconnect (the supervisor's loop-top guard
    // observes this same `Arc<AtomicBool>`). Before this wiring the flag was the supervisor's own
    // and disconnect() never set it.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = create_test_base_config();
        let mut stt = ReverieSTT::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    #[tokio::test]
    async fn test_reverie_stt_initial_state() {
        let config = create_test_base_config();
        let stt = ReverieSTT::new(config).unwrap();

        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[tokio::test]
    async fn test_reverie_stt_send_audio_not_connected() {
        let config = create_test_base_config();
        let mut stt = ReverieSTT::new(config).unwrap();

        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::ConnectionFailed(_)));
        }
    }

    #[tokio::test]
    async fn test_reverie_stt_callbacks() {
        use std::sync::atomic::AtomicUsize;

        let config = create_test_base_config();
        let mut stt = ReverieSTT::new(config).unwrap();

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
    fn test_reverie_stt_get_config() {
        let config = create_test_base_config();
        let stt = ReverieSTT::new(config).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "test-api-key");
        assert_eq!(stored_config.language, "hi");
        assert_eq!(stored_config.sample_rate, 16000);
    }

    #[test]
    fn test_reverie_websocket_url_construction() {
        let config = ReverieSTTConfig::new("test-key", "test-app")
            .with_language(super::super::config::ReverieLanguage::Hindi)
            .with_timeout(60)
            .with_continuous(true);

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://revapi.reverieinc.com/stream?"));
        assert!(url.contains("apikey=test-key"));
        assert!(url.contains("appid=test-app"));
        assert!(url.contains("src_lang=hi"));
        assert!(url.contains("timeout=60"));
        assert!(url.contains("continuous=1"));
    }

    #[test]
    fn test_reverie_websocket_url_trims_endpoint_override() {
        let config = ReverieSTTConfig {
            endpoint_override: Some(" wss://reverie-proxy.example.com/ ".to_string()),
            ..ReverieSTTConfig::new("test-key", "test-app")
        };

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://reverie-proxy.example.com/stream?"));
    }
}
