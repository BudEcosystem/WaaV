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
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
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
        }
    }
}

impl AssemblyAISTT {
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
        let (ws_tx, mut ws_rx) = mpsc::channel::<Bytes>(32);
        let (control_tx, mut control_rx) = mpsc::channel::<String>(8);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
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
        let host = Self::get_host_from_region(&config.region);

        // Clone shared state for the connection task
        let session_id = self.session_id.clone();
        let is_connected = self.is_connected.clone();

        // Start the connection task
        let connection_handle = tokio::spawn(async move {
            // Build WebSocket request with AssemblyAI authentication
            // Note: AssemblyAI uses "Authorization: <API_KEY>" (no Bearer prefix for WebSocket)
            let request = match tokio_tungstenite::tungstenite::http::Request::builder()
                .method("GET")
                .uri(&ws_url)
                .header("Host", host)
                .header("Upgrade", "websocket")
                .header("Connection", "upgrade")
                .header("Sec-WebSocket-Key", generate_key())
                .header("Sec-WebSocket-Version", "13")
                .header("Authorization", &api_key) // AssemblyAI uses raw API key
                .body(())
            {
                Ok(request) => request,
                Err(e) => {
                    let stt_error = STTError::ConnectionFailed(format!(
                        "Failed to create WebSocket request: {e}"
                    ));
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            };

            // Connect to AssemblyAI
            let (ws_stream, _response) = match connect_async(request).await {
                Ok(result) => result,
                Err(e) => {
                    let stt_error =
                        STTError::ConnectionFailed(format!("Failed to connect to AssemblyAI: {e}"));
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            };

            info!("Connected to AssemblyAI STT WebSocket");

            let (mut ws_sink, mut ws_stream) = ws_stream.split();

            let mut connected_tx = Some(connected_tx);

            // Main event loop
            loop {
                tokio::select! {
                    // Handle outgoing audio data
                    Some(audio_data) = ws_rx.recv() => {
                        // AssemblyAI accepts raw binary audio data (no base64 encoding)
                        // Zero-copy: Bytes is passed directly to WebSocket
                        let data_len = audio_data.len();
                        let message = Message::Binary(audio_data);
                        if let Err(e) = ws_sink.send(message).await {
                            let stt_error = STTError::NetworkError(format!(
                                "Failed to send audio to AssemblyAI: {e}"
                            ));
                            error!("{}", stt_error);
                            let _ = error_tx.try_send(stt_error);
                            break;
                        }

                        debug!("Sent {} bytes of audio to AssemblyAI", data_len);
                    }

                    // Handle control messages (ForceEndpoint, UpdateConfiguration, etc.)
                    Some(control_msg) = control_rx.recv() => {
                        if let Err(e) = ws_sink.send(Message::Text(control_msg.into())).await {
                            warn!("Failed to send control message: {}", e);
                        }
                    }

                    // Handle incoming messages with idle timeout
                    message = timeout(WS_MESSAGE_TIMEOUT, ws_stream.next()) => {
                        match message {
                            Ok(Some(Ok(msg))) => {
                                match Self::handle_websocket_message(
                                    msg,
                                    &result_tx,
                                    &session_id,
                                ).await {
                                    Ok(should_continue) => {
                                        if !should_continue {
                                            info!("AssemblyAI session terminated normally");
                                            is_connected.store(false, Ordering::Release);
                                            break;
                                        }

                                        // Signal connection ready after receiving Begin message
                                        if session_id.read().await.is_some()
                                            && let Some(tx) = connected_tx.take()
                                        {
                                            is_connected.store(true, Ordering::Release);
                                            let _ = tx.send(());
                                        }
                                    }
                                    Err(e) => {
                                        error!("AssemblyAI streaming error: {}", e);
                                        let _ = error_tx.try_send(e);
                                        is_connected.store(false, Ordering::Release);
                                        break;
                                    }
                                }
                            }
                            Ok(Some(Err(e))) => {
                                let stt_error = STTError::NetworkError(format!(
                                    "WebSocket error: {e}"
                                ));
                                error!("{}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                is_connected.store(false, Ordering::Release);
                                break;
                            }
                            Ok(None) => {
                                info!("AssemblyAI WebSocket stream ended");
                                is_connected.store(false, Ordering::Release);
                                break;
                            }
                            Err(_elapsed) => {
                                // Idle timeout - no message received for 60s
                                let stt_error = STTError::NetworkError(
                                    "WebSocket idle timeout - no message for 60 seconds".into()
                                );
                                error!("AssemblyAI STT idle timeout: {}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                is_connected.store(false, Ordering::Release);
                                break;
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        info!("Received shutdown signal for AssemblyAI STT");

                        // Send terminate message for graceful shutdown
                        let terminate_msg = TerminateMessage::default();
                        if let Ok(json) = serde_json::to_string(&terminate_msg) {
                            let _ = ws_sink.send(Message::Text(json.into())).await;
                        }

                        let _ = ws_sink.send(Message::Close(None)).await;
                        is_connected.store(false, Ordering::Release);
                        break;
                    }
                }
            }

            info!("AssemblyAI STT WebSocket connection closed");
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
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        let config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        self.start_connection(config.clone()).await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
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
        };

        self.config = Some(assemblyai_config);

        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "AssemblyAI Streaming STT v3"
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

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

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

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

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

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

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

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

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

        let result = AssemblyAISTT::handle_websocket_message(msg, &tx, &session_id).await;

        assert!(result.is_err());
        if let Err(STTError::ProviderError(msg)) = result {
            assert!(msg.contains("Rate limit"));
        } else {
            panic!("Expected ProviderError");
        }
    }
}
