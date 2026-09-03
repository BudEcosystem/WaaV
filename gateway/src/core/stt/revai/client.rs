//! Rev AI STT Client
//!
//! WebSocket-based streaming speech-to-text client for Rev AI API.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

use crate::core::resilience::connect::{WS_CONNECT_TIMEOUT, with_timeout};
use crate::core::stt::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback, STTStats,
};
use crate::core::websocket::ReconnectionConfig;
use crate::core::websocket::reconnectable_stream::{
    ReconnectOutcome, ReconnectableStream, ReconnectableStreamConfig, RestoreError, StreamError,
    WsTransport,
};

use super::EOS_MESSAGE;
use super::config::RevAISTTConfig;
use super::messages::{RevAICloseCode, ServerMessage};

// =============================================================================
// Type Aliases
// =============================================================================

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WebSocketSink = futures_util::stream::SplitSink<WsStream, Message>;
type WebSocketReadStream = futures_util::stream::SplitStream<WsStream>;

/// Per-message idle timeout for WebSocket message reception.
/// Resets after each successful message. Catches stuck/dead connections.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

// =============================================================================
// Supervised transport (W-D1 production adoption)
// =============================================================================

/// A [`WsTransport`] that adapts Rev AI's streaming event loop to the generic
/// [`ReconnectableStream`] supervisor (W-D1 fleet adoption). One is built per (re)connect by the
/// supervisor's `connect` closure; its [`run`](WsTransport::run) IS the original receiver loop,
/// now returning a [`ReconnectOutcome`] so a mid-stream transport drop reconnects instead of
/// bare-breaking the session.
///
/// Like Cartesia/ElevenLabs, Rev AI carries every feature (language, transcriber, diarization,
/// profanity/disfluency filtering, custom vocabulary id, auth `access_token`) in the connect URL,
/// so [`restore_session`](WsTransport::restore_session) is a no-op — a fresh dial already restored
/// the featured session.
struct RevAITransport {
    ws_sink: WebSocketSink,
    ws_stream: WebSocketReadStream,
    /// Shared inbound audio receiver (single-consumer; locked for the duration of `run`).
    audio_rx: Arc<Mutex<mpsc::Receiver<Bytes>>>,
    /// Shared shutdown token (fires once; an intentional close must not reconnect).
    shutdown_token: CancellationToken,
    /// Result callback storage (invoked directly, preserving the original architecture).
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback storage.
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Session ID from the `connected` message.
    session_id: Arc<RwLock<Option<String>>>,
    /// Statistics.
    stats: Arc<RwLock<STTStats>>,
    /// Fires once on the first successful connect, unblocking `connect`.
    connected_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl RevAITransport {
    async fn send_eos_and_close(ws_sink: &mut WebSocketSink) {
        let _ = ws_sink
            .send(Message::Text(EOS_MESSAGE.to_string().into()))
            .await;
        let _ = ws_sink.close().await;
    }
}

#[async_trait::async_trait]
impl WsTransport for RevAITransport {
    async fn restore_session(&mut self) -> Result<(), RestoreError> {
        // Rev AI puts every feature in the connect URL, so a fresh dial already restored the
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
                info!("Rev AI: Received shutdown signal");
                Self::send_eos_and_close(&mut self.ws_sink).await;
                return ReconnectOutcome::Completed;
            }

            tokio::select! {
                // Handle outgoing audio data (raw binary frames).
                Some(audio_data) = audio_rx.recv() => {
                    if let Err(e) = self
                        .ws_sink
                        .send(Message::Binary(bytes::Bytes::from(audio_data.to_vec())))
                        .await
                    {
                        let stt_error = STTError::NetworkError(format!("Failed to send audio: {e}"));
                        error!("{}", stt_error);
                        if let Some(ref cb) = *self.error_callback.read().await {
                            cb(stt_error).await;
                        }
                        // Transport-level send failure: reconnect to preserve the session.
                        return ReconnectOutcome::Reconnectable(StreamError::new("audio send failed"));
                    }
                    self.stats.write().await.total_audio_bytes += audio_data.len() as u64;
                    trace!(bytes = audio_data.len(), "Rev AI: Sent audio chunk");
                }

                // Handle incoming messages with idle timeout.
                message = timeout(WS_MESSAGE_TIMEOUT, self.ws_stream.next()) => {
                    match message {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            RevAISTT::handle_message(
                                &text,
                                &self.result_callback,
                                &self.error_callback,
                                &self.session_id,
                                &self.stats,
                            )
                            .await;
                        }
                        Ok(Some(Ok(Message::Close(frame)))) => {
                            let (code, reason) = frame
                                .map(|f| (f.code.into(), f.reason.to_string()))
                                .unwrap_or((1000, String::new()));
                            RevAISTT::handle_close(code, &reason, &self.error_callback).await;
                            let close_code = RevAICloseCode::from_code(code);
                            if close_code == RevAICloseCode::Normal {
                                // Server closed the session on purpose — intentional completion.
                                return ReconnectOutcome::Completed;
                            }
                            if close_code.is_retryable() {
                                // Transport-level abnormal close — reconnect to preserve the session.
                                return ReconnectOutcome::Reconnectable(StreamError::new("abnormal close"));
                            }
                            // Non-retryable close (auth/bad-request) — do not hammer with reconnects.
                            return ReconnectOutcome::Fatal(StreamError::new("non-retryable close"));
                        }
                        Ok(Some(Ok(Message::Ping(data)))) => {
                            trace!("Rev AI: Received ping");
                            let _ = data; // Pong handled automatically by tungstenite.
                        }
                        Ok(Some(Ok(Message::Pong(_)))) => {
                            trace!("Rev AI: Received pong");
                        }
                        Ok(Some(Ok(Message::Binary(data)))) => {
                            warn!(len = data.len(), "Rev AI: Received unexpected binary message");
                        }
                        Ok(Some(Ok(Message::Frame(_)))) => {
                            // Raw frame, ignore.
                        }
                        Ok(Some(Err(e))) => {
                            error!(error = %e, "Rev AI: WebSocket error");
                            if let Some(ref cb) = *self.error_callback.read().await {
                                cb(STTError::NetworkError(format!("WebSocket error: {e}"))).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("websocket error"));
                        }
                        Ok(None) => {
                            info!("Rev AI: WebSocket stream ended");
                            return ReconnectOutcome::Reconnectable(StreamError::new("stream ended"));
                        }
                        Err(_elapsed) => {
                            let stt_error = STTError::NetworkError(
                                "Rev AI WebSocket idle timeout - no message for 60 seconds".into(),
                            );
                            error!("Rev AI STT idle timeout: {}", stt_error);
                            if let Some(ref cb) = *self.error_callback.read().await {
                                cb(stt_error).await;
                            }
                            return ReconnectOutcome::Reconnectable(StreamError::new("idle timeout"));
                        }
                    }
                }

                // Handle shutdown signal (intentional close — must NOT reconnect).
                _ = shutdown_token.cancelled() => {
                    info!("Rev AI: Received shutdown signal");
                    Self::send_eos_and_close(&mut self.ws_sink).await;
                    return ReconnectOutcome::Completed;
                }
            }
        }
    }
}

// =============================================================================
// Rev AI STT Client
// =============================================================================

/// Rev AI STT client implementing BaseSTT trait
pub struct RevAISTT {
    /// Base STT configuration
    config: STTConfig,

    /// Rev AI specific configuration
    revai_config: RevAISTTConfig,

    /// Audio sender (bounded channel for backpressure); the supervised transport drains it.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal token.
    shutdown_token: Option<CancellationToken>,

    /// Connection task handle (the supervisor's outer reconnect loop).
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Connection state flag
    connected: AtomicBool,

    /// Intentional-disconnect flag shared with the reconnect supervisor (W-D1). Cleared on
    /// `connect()`, set in `disconnect()` before cancelling `shutdown_token`, so a client close racing a
    /// server-side close can never trigger a spurious reconnect.
    intentional_disconnect: Arc<AtomicBool>,

    /// Session ID from connected message
    session_id: Arc<RwLock<Option<String>>>,

    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,

    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,

    /// Statistics
    stats: Arc<RwLock<STTStats>>,

    /// Shared, process-global resilience handles (W-D2): the single reconnect governor + this
    /// provider's shared circuit breaker, injected by the VoiceManager from CoreState and driven
    /// by the generic [`ReconnectableStream`](crate::core::websocket::ReconnectableStream)
    /// supervisor. `None` before `set_resilience` (a direct unit-test construction) → the
    /// supervisor uses its own per-session governor/breaker default.
    resilience: Option<crate::core::resilience::ResilienceHandles>,
}

impl RevAISTT {
    /// Create a new Rev AI STT client from Rev AI config
    pub fn from_revai_config(config: RevAISTTConfig) -> Result<Self, STTError> {
        config.validate()?;

        let base_config: STTConfig = config.clone().into();

        Ok(Self {
            config: base_config,
            revai_config: config,
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            connected: AtomicBool::new(false),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            resilience: None,
        })
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Rev AI can express (diarization, profanity filtering, filler words) are honored
    /// END-TO-END. Mirrors `DeepgramSTT::new_standard`: validate the api_key, then build the
    /// provider from the standardized->provider config mapping (`RevAISTTConfig::from_standard`).
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        Self::from_revai_config(
            crate::core::stt::revai::config::RevAISTTConfig::from_standard(std)?,
        )
    }

    /// Get the session ID
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Get statistics
    pub async fn get_stats(&self) -> STTStats {
        self.stats.read().await.clone()
    }

    /// The shared circuit breaker this session feeds into the generic supervisor, if the
    /// process-global resilience handles have been injected (W-D1/W-D2). Two `RevAISTT` built from
    /// the same [`crate::core::resilience::ResilienceRegistry`] return the *same* `Arc`.
    pub fn resilience_breaker(&self) -> Option<&Arc<crate::core::resilience::CircuitBreaker>> {
        self.resilience.as_ref().map(|r| &r.breaker)
    }

    /// Handle incoming WebSocket message
    async fn handle_message(
        message: &str,
        result_callback: &Arc<RwLock<Option<STTResultCallback>>>,
        error_callback: &Arc<RwLock<Option<STTErrorCallback>>>,
        session_id: &Arc<RwLock<Option<String>>>,
        stats: &Arc<RwLock<STTStats>>,
    ) {
        match ServerMessage::from_json(message) {
            Ok(server_msg) => match server_msg {
                ServerMessage::Connected(connected) => {
                    info!(session_id = %connected.id, "Rev AI: Connected to streaming session");
                    *session_id.write().await = Some(connected.id);
                }
                ServerMessage::Partial(partial) => {
                    let text = partial.text();
                    trace!(text = %text, ts = %partial.ts, "Rev AI: Partial transcript");

                    if let Some(ref callback) = *result_callback.read().await {
                        let result =
                            STTResult::new(text, false, false, partial.average_confidence() as f32);
                        callback(result).await;
                    }
                }
                ServerMessage::Final(final_transcript) => {
                    let text = final_transcript.text();
                    let confidence = final_transcript.average_confidence() as f32;

                    debug!(
                        text = %text,
                        confidence = %confidence,
                        word_count = %final_transcript.word_count(),
                        "Rev AI: Final transcript"
                    );

                    let result = STTResult::new(text, true, true, confidence);

                    // Update stats
                    stats.write().await.update_with_result(&result);

                    if let Some(ref callback) = *result_callback.read().await {
                        callback(result).await;
                    }
                }
            },
            Err(e) => {
                warn!(error = %e, message = %message, "Rev AI: Failed to parse server message");
                if let Some(ref callback) = *error_callback.read().await {
                    callback(STTError::ProviderError(format!(
                        "Failed to parse message: {}",
                        e
                    )))
                    .await;
                }
            }
        }
    }

    /// Handle WebSocket close
    async fn handle_close(
        code: u16,
        reason: &str,
        error_callback: &Arc<RwLock<Option<STTErrorCallback>>>,
    ) {
        let close_code = RevAICloseCode::from_code(code);
        let description = close_code.description();

        if close_code == RevAICloseCode::Normal {
            info!("Rev AI: WebSocket closed normally");
        } else {
            warn!(
                code = %code,
                reason = %reason,
                description = %description,
                retryable = %close_code.is_retryable(),
                "Rev AI: WebSocket closed with error"
            );

            if let Some(ref callback) = *error_callback.read().await {
                callback(STTError::ConnectionFailed(format!(
                    "Connection closed: {} ({})",
                    description, code
                )))
                .await;
            }
        }
    }
}

// =============================================================================
// BaseSTT Implementation
// =============================================================================

#[async_trait::async_trait]
impl BaseSTT for RevAISTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Rev AI API key is required. Get your API key from https://www.rev.ai/access_token"
                    .to_string(),
            ));
        }

        let revai_config = RevAISTTConfig::from_base(&config)?;

        Ok(Self {
            config,
            revai_config,
            ws_sender: None,
            shutdown_token: None,
            connection_handle: None,
            connected: AtomicBool::new(false),
            intentional_disconnect: Arc::new(AtomicBool::new(false)),
            session_id: Arc::new(RwLock::new(None)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            resilience: None,
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::Acquire) {
            warn!("Rev AI: Already connected");
            return Ok(());
        }
        // Fresh session: clear any intent left over from a prior disconnect so the supervisor
        // does not immediately complete.
        self.intentional_disconnect.store(false, Ordering::SeqCst);

        info!("Rev AI: Connecting to streaming endpoint");

        // Build WebSocket URL with all parameters (every feature rides the URL).
        let ws_url = self.revai_config.build_websocket_url();
        debug!(url = %ws_url.replace(&self.revai_config.api_key, "[REDACTED]"), "Rev AI: WebSocket URL");

        // Create channels for communication (bounded for backpressure on audio).
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

        let result_callback = Arc::clone(&self.result_callback);
        let error_callback = Arc::clone(&self.error_callback);
        let session_id = Arc::clone(&self.session_id);
        let stats = Arc::clone(&self.stats);

        // Storm control + provider breaker: drive the GENERIC ReconnectableStream supervisor (the
        // same one the chaos tests exercise) with the shared process-global handles from CoreState
        // (W-D1/W-D2 fleet adoption). When no handles were injected (a direct unit-test
        // construction), the supervisor uses its own per-session governor/breaker default.
        let reconnection = ReconnectionConfig::aggressive();
        let disconnect_flag = Arc::clone(&self.intentional_disconnect);
        let supervisor = match self.resilience.clone() {
            Some(r) => ReconnectableStream::with_breaker_and_governor(
                ReconnectableStreamConfig::new("revai", reconnection),
                r.breaker,
                (*r.governor).clone(),
            ),
            None => ReconnectableStream::new(ReconnectableStreamConfig::new("revai", reconnection)),
        }
        .with_disconnect_flag(disconnect_flag);

        // Start the connection task: the supervisor owns the outer reconnect loop; the `connect`
        // closure dials the *featured* URL (every feature is in the URL) and hands back a transport
        // whose `run()` is the original Rev AI receiver loop.
        let connection_handle = tokio::spawn(async move {
            let exit = supervisor
                .run(|| {
                    let ws_url = ws_url.clone();
                    let audio_rx = Arc::clone(&audio_rx);
                    let shutdown_token = shutdown_token.clone();
                    let connected_tx = Arc::clone(&connected_tx);
                    let result_callback = Arc::clone(&result_callback);
                    let error_callback = Arc::clone(&error_callback);
                    let session_id = Arc::clone(&session_id);
                    let stats = Arc::clone(&stats);
                    async move {
                        let request = ws_url.into_client_request().map_err(|e| {
                            StreamError::new(format!("Failed to create request: {e}"))
                        })?;
                        let (ws_stream, response) =
                            with_timeout(WS_CONNECT_TIMEOUT, connect_async(request))
                                .await
                                .map_err(|_| {
                                    StreamError::new(format!(
                                        "connect to Rev AI timed out after {}s",
                                        WS_CONNECT_TIMEOUT.as_secs()
                                    ))
                                })?
                                .map_err(|e| {
                                    StreamError::new(format!("WebSocket connection failed: {e}"))
                                })?;
                        debug!(status = ?response.status(), "Rev AI: WebSocket connection established");
                        let (ws_sink, ws_stream) = ws_stream.split();
                        Ok(RevAITransport {
                            ws_sink,
                            ws_stream,
                            audio_rx,
                            shutdown_token,
                            result_callback,
                            error_callback,
                            session_id,
                            stats,
                            connected_tx,
                        })
                    }
                })
                .await;
            info!("Rev AI: WebSocket connection closed (supervisor exit: {exit:?})");
        });

        self.connection_handle = Some(connection_handle);

        // Wait for the first successful connect (restore_session fires the connected signal).
        match timeout(Duration::from_secs(10), connected_rx).await {
            Ok(Ok(())) => {
                self.connected.store(true, Ordering::Release);
                info!("Rev AI: Successfully connected");
                Ok(())
            }
            Ok(Err(_)) => Err(STTError::ConnectionFailed(
                "Connection channel closed before Rev AI session started".to_string(),
            )),
            Err(_) => Err(STTError::ConnectionFailed(
                "Connection timeout waiting for Rev AI session".to_string(),
            )),
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Record the intent BEFORE the connected-guard so the supervisor sees it even if the
        // transport's run() just reported a reconnectable drop (the disconnect-vs-close race).
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        if !self.connected.load(Ordering::Acquire) && self.connection_handle.is_none() {
            return Ok(());
        }

        info!("Rev AI: Disconnecting");

        // Signal the supervised transport to send EOS + close intentionally (no reconnect).
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }

        // Wait for the connection task to finish with a timeout (EOS + final results drain).
        if let Some(handle) = self.connection_handle.take() {
            crate::core::observability::await_task_shutdown(
                "revai-stt-connection",
                handle,
                Duration::from_secs(5),
            )
            .await;
        }

        self.connected.store(false, Ordering::Release);
        self.ws_sender = None;
        *self.session_id.write().await = None;

        info!("Rev AI: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Rev AI".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio: {}", e)))?;
            trace!(bytes = data_len, "Rev AI: Queued audio chunk");
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.result_callback.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.error_callback.write().await = Some(callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Validate new config
        let new_revai_config = RevAISTTConfig::from_base(&config)?;

        // If connected, need to reconnect with new config
        if self.is_ready() {
            self.disconnect().await?;
            self.config = config;
            self.revai_config = new_revai_config;
            self.connect().await?;
        } else {
            self.config = config;
            self.revai_config = new_revai_config;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Rev AI Streaming STT v1 - WebSocket API"
    }

    fn set_resilience(&mut self, resilience: crate::core::resilience::ResilienceHandles) {
        // Store the shared, process-global handles so `connect` drives the generic
        // ReconnectableStream supervisor with them — every Rev AI session trips the same breaker
        // and shares the one process-wide reconnect cap (W-D2).
        self.resilience = Some(resilience);
    }
}

impl Drop for RevAISTT {
    fn drop(&mut self) {
        self.intentional_disconnect.store(true, Ordering::SeqCst);
        // Send shutdown signal if still connected so the supervisor task stops cleanly.
        if let Some(shutdown_token) = self.shutdown_token.take() {
            shutdown_token.cancel();
        }
        if let Some(handle) = self.connection_handle.take() {
            handle.abort();
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "revai".to_string(),
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "S16LE".to_string(),
            model: "machine".to_string(),
        }
    }

    #[test]
    fn test_revai_stt_new_success() {
        let config = create_test_config();
        let result = RevAISTT::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_revai_stt_new_empty_api_key() {
        let mut config = create_test_config();
        config.api_key = String::new();

        let result = RevAISTT::new(config);
        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("API key"));
        }
    }

    #[test]
    fn test_revai_stt_from_revai_config() {
        let config = RevAISTTConfig::new("test-key")
            .with_language("es")
            .with_filter_profanity(true);

        let result = RevAISTT::from_revai_config(config);
        assert!(result.is_ok());
    }

    // W1 keystone: advanced features Rev AI can express (diarization -> enable_speaker_switch +
    // machine_v2 transcriber, profanity_filter -> filter_profanity) survive through the
    // provider-struct `new_standard` method to the provider-specific config.
    #[test]
    fn test_revai_new_standard_unlocks_advanced_features() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "revai".into(),
                api_key: "test-api-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                profanity_filter: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = RevAISTT::new_standard(&std).expect("new_standard must succeed");
        assert!(stt.revai_config.enable_speaker_switch); // diarization
        assert_eq!(
            stt.revai_config.transcriber,
            super::super::config::RevAITranscriber::MachineV2
        ); // required by switch
        assert!(stt.revai_config.filter_profanity); // profanity_filter

        // Missing api_key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            provider: "revai".into(),
            api_key: String::new(),
            ..Default::default()
        });
        assert!(RevAISTT::new_standard(&bad).is_err());
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
                    provider: "revai".into(),
                    api_key: "test-key".into(),
                    language: "en".into(),
                    sample_rate: 16000,
                    encoding: "S16LE".into(),
                    model: "machine".into(),
                    ..Default::default()
                },
                features: SttFeatures::default(),
                extras: ProviderExtras::default(),
                translation: None,
            }
            .with_endpoint_override(endpoint)
        };

        assert!(RevAISTT::new_standard(&mk("wss://revai-proxy.example.com")).is_ok());
        assert!(RevAISTT::new_standard(&mk("ws://127.0.0.1:9000")).is_err());
        assert!(RevAISTT::new_standard(&mk("file:///tmp/socket")).is_err());
        assert!(RevAISTT::new_standard(&mk("https://revai-proxy.example.com")).is_err());

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
    fn test_revai_stt_not_connected_initially() {
        let config = create_test_config();
        let stt = RevAISTT::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_revai_stt_get_config() {
        let config = create_test_config();
        let stt = RevAISTT::new(config.clone()).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, config.api_key);
        assert_eq!(stored_config.language, config.language);
        assert_eq!(stored_config.sample_rate, config.sample_rate);
    }

    #[test]
    fn test_revai_stt_get_provider_info() {
        let config = create_test_config();
        let stt = RevAISTT::new(config).unwrap();

        let info = stt.get_provider_info();
        assert!(info.contains("Rev AI"));
        assert!(info.contains("WebSocket"));
    }

    #[tokio::test]
    async fn test_revai_stt_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = RevAISTT::new(config).unwrap();

        let audio_data = Bytes::from(vec![0u8; 1024]);
        let result = stt.send_audio(audio_data).await;

        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("Not connected"));
        }
    }

    #[tokio::test]
    async fn test_revai_stt_on_result() {
        let config = create_test_config();
        let mut stt = RevAISTT::new(config).unwrap();

        let callback: STTResultCallback = Arc::new(|result: STTResult| {
            Box::pin(async move {
                println!("Received: {:?}", result);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_revai_stt_on_error() {
        let config = create_test_config();
        let mut stt = RevAISTT::new(config).unwrap();

        let callback: STTErrorCallback = Arc::new(|error: STTError| {
            Box::pin(async move {
                println!("Error: {:?}", error);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result = stt.on_error(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_revai_stt_get_session_id_none() {
        let config = create_test_config();
        let stt = RevAISTT::new(config).unwrap();

        let session_id = stt.get_session_id().await;
        assert!(session_id.is_none());
    }

    #[tokio::test]
    async fn test_revai_stt_get_stats_default() {
        let config = create_test_config();
        let stt = RevAISTT::new(config).unwrap();

        let stats = stt.get_stats().await;
        assert_eq!(stats.total_audio_bytes, 0);
        assert_eq!(stats.results_count, 0);
        assert_eq!(stats.final_results_count, 0);
    }

    #[tokio::test]
    async fn test_revai_stt_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = RevAISTT::new(config).unwrap();

        // Should not error when disconnecting a non-connected client
        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    // W-D1: disconnect() must record intent on the supervisor-shared flag so a client close racing
    // a server-side close can never trigger a spurious reconnect (the supervisor's loop-top guard
    // observes this same `Arc<AtomicBool>`). Before this wiring the flag was the supervisor's own
    // and disconnect() never set it.
    #[tokio::test]
    async fn disconnect_sets_intentional_flag_for_supervisor() {
        let config = create_test_config();
        let mut stt = RevAISTT::new(config).unwrap();
        assert!(!stt.intentional_disconnect.load(Ordering::SeqCst));
        stt.disconnect().await.unwrap();
        assert!(
            stt.intentional_disconnect.load(Ordering::SeqCst),
            "disconnect() must set the supervisor-shared intentional-disconnect flag",
        );
    }

    #[tokio::test]
    async fn test_revai_stt_update_config() {
        let config = create_test_config();
        let mut stt = RevAISTT::new(config).unwrap();

        let mut new_config = create_test_config();
        new_config.language = "es".to_string();
        new_config.sample_rate = 44100;

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.language, "es");
        assert_eq!(stored_config.sample_rate, 44100);
    }

    #[tokio::test]
    async fn test_handle_message_connected() {
        let result_callback: Arc<RwLock<Option<STTResultCallback>>> = Arc::new(RwLock::new(None));
        let error_callback: Arc<RwLock<Option<STTErrorCallback>>> = Arc::new(RwLock::new(None));
        let session_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let stats: Arc<RwLock<STTStats>> = Arc::new(RwLock::new(STTStats::default()));

        let message = r#"{"type": "connected", "id": "test-session-123"}"#;

        RevAISTT::handle_message(
            message,
            &result_callback,
            &error_callback,
            &session_id,
            &stats,
        )
        .await;

        let session = session_id.read().await;
        assert_eq!(session.as_deref(), Some("test-session-123"));
    }

    #[tokio::test]
    async fn test_handle_message_final_updates_stats() {
        let result_callback: Arc<RwLock<Option<STTResultCallback>>> = Arc::new(RwLock::new(None));
        let error_callback: Arc<RwLock<Option<STTErrorCallback>>> = Arc::new(RwLock::new(None));
        let session_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let stats: Arc<RwLock<STTStats>> = Arc::new(RwLock::new(STTStats::default()));

        let message = r#"{"type": "final", "ts": 0.0, "end_ts": 1.0, "elements": [{"type": "text", "value": "hello", "confidence": 0.95}]}"#;

        RevAISTT::handle_message(
            message,
            &result_callback,
            &error_callback,
            &session_id,
            &stats,
        )
        .await;

        let stats_val = stats.read().await;
        assert_eq!(stats_val.results_count, 1);
        assert_eq!(stats_val.final_results_count, 1);
    }

    #[tokio::test]
    async fn test_handle_message_with_callback() {
        use std::sync::atomic::AtomicUsize;

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let callback: STTResultCallback = Arc::new(move |result: STTResult| {
            let count = Arc::clone(&call_count_clone);
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(result.transcript, "hello");
                assert!(result.is_final);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result_callback: Arc<RwLock<Option<STTResultCallback>>> =
            Arc::new(RwLock::new(Some(callback)));
        let error_callback: Arc<RwLock<Option<STTErrorCallback>>> = Arc::new(RwLock::new(None));
        let session_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let stats: Arc<RwLock<STTStats>> = Arc::new(RwLock::new(STTStats::default()));

        let message = r#"{"type": "final", "ts": 0.0, "end_ts": 1.0, "elements": [{"type": "text", "value": "hello", "confidence": 0.95}]}"#;

        RevAISTT::handle_message(
            message,
            &result_callback,
            &error_callback,
            &session_id,
            &stats,
        )
        .await;

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
