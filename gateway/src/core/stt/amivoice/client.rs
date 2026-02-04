//! AmiVoice STT WebSocket client implementation.
//!
//! This module implements the `BaseSTT` trait for real-time speech-to-text
//! using the AmiVoice Cloud Platform WebSocket API.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
//! │   send_audio()  │────▶│  ws_sender (mpsc)│────▶│  WebSocket Task │
//! └─────────────────┘     └──────────────────┘     └────────┬────────┘
//!                                                           │
//!                         ┌──────────────────┐              │
//!                         │  result_tx (mpsc)│◀─────────────┘
//!                         └────────┬─────────┘
//!                                  │
//!                         ┌────────▼─────────┐
//!                         │ Result Forward   │────▶ User Callback
//!                         │      Task        │
//!                         └──────────────────┘
//! ```
//!
//! # AmiVoice-Specific Protocol
//!
//! Unlike standard JSON-over-WebSocket, AmiVoice uses a proprietary protocol:
//!
//! - `s <format> <engine> [params]` - Start recognition session
//! - `p<binary_data>` - Send audio (binary message)
//! - `e` - End session
//!
//! Server responses use single-letter prefixes (S, E, C, U, A, G, e).

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::config::AmiVoiceSTTConfig;
use super::messages::AmiVoiceMessage;
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Constants
// =============================================================================

/// Per-message idle timeout for WebSocket message reception.
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Connection timeout for WebSocket establishment.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Keep-alive interval for sending silence frames.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Time after which to send keep-alive if no audio sent.
const AUDIO_IDLE_THRESHOLD: Duration = Duration::from_secs(10);

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
enum ConnectionState {
    /// Not connected to AmiVoice.
    Disconnected,
    /// In the process of establishing connection.
    Connecting,
    /// Connected and ready to receive audio.
    Connected,
    /// An error occurred.
    #[allow(dead_code)]
    Error(String),
}

// =============================================================================
// AmiVoiceSTT Client
// =============================================================================

/// AmiVoice Speech-to-Text WebSocket client.
///
/// This struct implements real-time speech-to-text using the AmiVoice
/// Cloud Platform WebSocket API. It manages:
///
/// - WebSocket connection lifecycle
/// - Audio data streaming to AmiVoice
/// - Transcription result callbacks (both interim and final)
/// - Error handling and recovery
/// - Keep-alive mechanism
///
/// # Example
///
/// ```rust,no_run
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::amivoice::AmiVoiceSTT;
/// use std::sync::Arc;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let config = STTConfig {
///         api_key: "your-amivoice-appkey".to_string(),
///         language: "ja".to_string(),
///         sample_rate: 16000,
///         model: "-a-general".to_string(),
///         ..Default::default()
///     };
///
///     let mut stt = AmiVoiceSTT::new(config)?;
///
///     // Register result callback
///     stt.on_result(Arc::new(|result| {
///         Box::pin(async move {
///             println!("Transcript: {}", result.transcript);
///         })
///     })).await?;
///
///     // Connect to AmiVoice
///     stt.connect().await?;
///
///     // Send audio data
///     let audio_data = vec![0u8; 1024]; // Your PCM audio data
///     stt.send_audio(audio_data.into()).await?;
///
///     // Disconnect when done
///     stt.disconnect().await?;
///
///     Ok(())
/// }
/// ```
pub struct AmiVoiceSTT {
    /// Configuration for the STT client.
    config: Option<AmiVoiceSTTConfig>,

    /// Current connection state.
    state: ConnectionState,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Result channel sender.
    result_tx: Option<mpsc::Sender<STTResult>>,

    /// Error channel sender.
    error_tx: Option<mpsc::Sender<STTError>>,

    /// Connection task handle.
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result forwarding task handle.
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Error forwarding task handle.
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Shared callback storage for async access.
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,

    /// Error callback storage.
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,

    /// Connection ready flag for atomic checks.
    connected: Arc<AtomicBool>,
}

impl Default for AmiVoiceSTT {
    fn default() -> Self {
        Self {
            config: None,
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl AmiVoiceSTT {
    /// Handle incoming WebSocket messages from AmiVoice.
    fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
        interim_results_enabled: bool,
    ) -> Result<bool, STTError> {
        match message {
            Message::Text(text) => {
                debug!("Received AmiVoice message: {}", text);

                match AmiVoiceMessage::parse(&text) {
                    Ok(parsed_msg) => match parsed_msg {
                        AmiVoiceMessage::SessionStartOk => {
                            info!("AmiVoice session started successfully");
                        }

                        AmiVoiceMessage::SessionStartError(error) => {
                            error!("AmiVoice session start failed: {}", error);
                            let stt_error = if error.to_lowercase().contains("auth")
                                || error.to_lowercase().contains("appkey")
                                || error.to_lowercase().contains("unauthorized")
                            {
                                STTError::AuthenticationFailed(format!(
                                    "AmiVoice authentication failed: {}",
                                    error
                                ))
                            } else {
                                STTError::ConnectionFailed(format!(
                                    "AmiVoice session start failed: {}",
                                    error
                                ))
                            };
                            let _ = error_tx.try_send(stt_error);
                            return Ok(true); // Signal to close connection
                        }

                        AmiVoiceMessage::SpeechStart { timestamp_ms } => {
                            debug!("Speech detected at {}ms", timestamp_ms);
                        }

                        AmiVoiceMessage::SpeechEnd { timestamp_ms } => {
                            debug!("Speech ended at {}ms", timestamp_ms);
                        }

                        AmiVoiceMessage::ProcessingStart => {
                            debug!("Recognition processing started");
                        }

                        AmiVoiceMessage::InterimResult(result) => {
                            if interim_results_enabled && !result.text.is_empty() {
                                let stt_result = result.to_stt_result(false);
                                if !stt_result.transcript.is_empty() {
                                    if result_tx.try_send(stt_result).is_err() {
                                        warn!("Failed to send interim result - channel closed");
                                    }
                                }
                            }
                        }

                        AmiVoiceMessage::FinalResult(result) => {
                            if !result.text.is_empty() {
                                let stt_result = result.to_stt_result(true);
                                if !stt_result.transcript.is_empty() {
                                    if result_tx.try_send(stt_result).is_err() {
                                        warn!("Failed to send final result - channel closed");
                                    }
                                }
                            }
                        }

                        AmiVoiceMessage::ServerInfo(info) => {
                            debug!("AmiVoice server info: {}", info);
                        }

                        AmiVoiceMessage::SessionEndOk => {
                            info!("AmiVoice session ended successfully");
                            return Ok(true); // Close connection gracefully
                        }

                        AmiVoiceMessage::SessionEndError(error) => {
                            error!("AmiVoice session end error: {}", error);
                            let stt_error = STTError::ProviderError(format!(
                                "AmiVoice session error: {}",
                                error
                            ));
                            let _ = error_tx.try_send(stt_error);
                            return Ok(true); // Close connection
                        }
                    },
                    Err(e) => {
                        warn!("Failed to parse AmiVoice message: {} - raw: {}", e, text);
                    }
                }
            }

            Message::Binary(data) => {
                // AmiVoice doesn't typically send binary responses
                debug!(
                    "Received binary message from AmiVoice: {} bytes",
                    data.len()
                );
            }

            Message::Close(close_frame) => {
                info!("AmiVoice WebSocket closed: {:?}", close_frame);
                return Ok(true); // Signal to close
            }

            Message::Ping(_) => {
                debug!("Received ping from AmiVoice");
            }

            Message::Pong(_) => {
                debug!("Received pong from AmiVoice");
            }

            _ => {
                debug!("Received unexpected message type from AmiVoice");
            }
        }

        Ok(false) // Don't close connection
    }

    /// Start the WebSocket connection to AmiVoice.
    async fn start_connection(&mut self) -> Result<(), STTError> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| STTError::ConfigurationError("No configuration available".to_string()))?
            .clone();

        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        // Build WebSocket URL
        let ws_url = config.get_websocket_url();

        // Create channels for communication
        let (ws_tx, mut ws_rx) = mpsc::channel::<Bytes>(32);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(256);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(64);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        // Store channels
        self.ws_sender = Some(ws_tx);
        self.shutdown_tx = Some(shutdown_tx);
        self.result_tx = Some(result_tx.clone());
        self.error_tx = Some(error_tx.clone());

        // Clone necessary data for the connection task
        let start_command = config.build_start_command();
        let interim_results_enabled = config.interim_results;
        let connected_flag = self.connected.clone();

        // Start the connection task
        let connection_handle = tokio::spawn(async move {
            // Connect to AmiVoice with timeout
            let connect_result = match timeout(WS_CONNECT_TIMEOUT, connect_async(ws_url)).await {
                Ok(result) => result,
                Err(_) => {
                    let stt_error = STTError::ConnectionFailed(
                        "Connection to AmiVoice timed out after 30 seconds".to_string(),
                    );
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            };

            let (ws_stream, _response) = match connect_result {
                Ok(result) => result,
                Err(e) => {
                    let error_msg = format!("Failed to connect to AmiVoice: {e}");
                    let stt_error = STTError::ConnectionFailed(error_msg);
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            };

            info!("Connected to AmiVoice WebSocket");

            let (mut ws_sink, mut ws_stream) = ws_stream.split();

            // Send start command (s command)
            debug!("Sending start command: {}", start_command);
            if let Err(e) = ws_sink.send(Message::Text(start_command.into())).await {
                let stt_error =
                    STTError::ConnectionFailed(format!("Failed to send start command: {e}"));
                error!("{}", stt_error);
                let _ = error_tx.try_send(stt_error);
                return;
            }

            // Wait for session start response ('s' or 's <error>')
            let session_start_timeout = timeout(Duration::from_secs(10), async {
                while let Some(msg) = ws_stream.next().await {
                    if let Ok(Message::Text(text)) = msg {
                        if text.starts_with('s') {
                            return AmiVoiceMessage::parse(&text);
                        }
                    }
                }
                Err("No session start response received".to_string())
            })
            .await;

            match session_start_timeout {
                Ok(Ok(AmiVoiceMessage::SessionStartOk)) => {
                    info!("AmiVoice session started successfully");
                    connected_flag.store(true, Ordering::Release);
                    let _ = connected_tx.send(());
                }
                Ok(Ok(AmiVoiceMessage::SessionStartError(e))) => {
                    let stt_error = if e.to_lowercase().contains("auth")
                        || e.to_lowercase().contains("appkey")
                    {
                        STTError::AuthenticationFailed(format!("AmiVoice auth failed: {e}"))
                    } else {
                        STTError::ConnectionFailed(format!("AmiVoice session failed: {e}"))
                    };
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
                Ok(Err(e)) => {
                    let stt_error = STTError::ConnectionFailed(format!(
                        "Failed to parse session response: {e}"
                    ));
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
                Err(_) => {
                    let stt_error = STTError::ConnectionFailed(
                        "Timeout waiting for AmiVoice session start".to_string(),
                    );
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
                _ => {
                    let stt_error =
                        STTError::ConnectionFailed("Unexpected response from AmiVoice".to_string());
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            }

            // Keep-alive mechanism
            let mut keepalive_timer = interval(KEEPALIVE_INTERVAL);
            let mut last_audio_time = Instant::now();

            // Main event loop
            loop {
                tokio::select! {
                    // Prioritize audio sending for lowest latency
                    biased;

                    // Handle outgoing audio data
                    Some(audio_data) = ws_rx.recv() => {
                        // AmiVoice expects raw audio as binary WebSocket frames
                        // Protocol: 'p' prefix followed by binary data
                        // But for WebSocket binary frames, we send raw data
                        let message = Message::Binary(audio_data);
                        if let Err(e) = ws_sink.send(message).await {
                            let stt_error = STTError::NetworkError(format!(
                                "Failed to send audio to AmiVoice: {e}"
                            ));
                            error!("{}", stt_error);
                            let _ = error_tx.try_send(stt_error);
                            break;
                        }
                        last_audio_time = Instant::now();
                    }

                    // Handle incoming messages with idle timeout
                    message = timeout(WS_MESSAGE_TIMEOUT, ws_stream.next()) => {
                        match message {
                            Ok(Some(Ok(msg))) => {
                                match Self::handle_websocket_message(
                                    msg,
                                    &result_tx,
                                    &error_tx,
                                    interim_results_enabled,
                                ) {
                                    Ok(should_close) => {
                                        if should_close {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        error!("AmiVoice message handling error: {}", e);
                                        let _ = error_tx.try_send(e);
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
                                break;
                            }
                            Ok(None) => {
                                info!("AmiVoice WebSocket stream ended");
                                break;
                            }
                            Err(_elapsed) => {
                                let stt_error = STTError::NetworkError(
                                    "WebSocket idle timeout - no message for 60 seconds".into()
                                );
                                error!("AmiVoice STT idle timeout: {}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                break;
                            }
                        }
                    }

                    // Handle keep-alive timer
                    _ = keepalive_timer.tick() => {
                        if last_audio_time.elapsed() >= AUDIO_IDLE_THRESHOLD {
                            // Send a small buffer of silence (32 samples at 16kHz = 2ms)
                            let silence_frame = vec![0u8; 64];
                            let message = Message::Binary(silence_frame.into());
                            if let Err(e) = ws_sink.send(message).await {
                                let stt_error = STTError::NetworkError(format!(
                                    "Failed to send keep-alive: {e}"
                                ));
                                error!("{}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                break;
                            }
                            debug!("Sent keep-alive silence frame to AmiVoice");
                            last_audio_time = Instant::now();
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        info!("Received shutdown signal for AmiVoice STT");
                        // Send end command to gracefully end recognition
                        if let Err(e) = ws_sink.send(Message::Text("e".into())).await {
                            debug!("Failed to send end command: {}", e);
                        }
                        let _ = ws_sink.send(Message::Close(None)).await;
                        break;
                    }
                }
            }

            connected_flag.store(false, Ordering::Release);
            info!("AmiVoice STT WebSocket connection closed");
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
                        "AmiVoice STT result (no callback): {} (final: {}, confidence: {})",
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
                    error!("AmiVoice STT error (no callback registered): {}", error);
                }
            }
        });

        self.error_forward_handle = Some(error_forwarding_handle);

        // Update state and wait for connection
        self.state = ConnectionState::Connecting;

        // Wait for connection with timeout
        match timeout(WS_CONNECT_TIMEOUT, connected_rx).await {
            Ok(Ok(())) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to AmiVoice Speech-to-Text");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Connection channel closed before confirmation".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Connection timeout waiting for AmiVoice".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }

    /// Set the speech recognition engine.
    pub fn set_engine(&mut self, engine: super::config::AmiVoiceEngine) {
        if let Some(config) = &mut self.config {
            config.engine = engine;
        }
    }

    /// Enable or disable interim (partial) results.
    pub fn set_interim_results(&mut self, enabled: bool) {
        if let Some(config) = &mut self.config {
            config.interim_results = enabled;
        }
    }

    /// Enable or disable data logging.
    pub fn set_no_logging(&mut self, no_logging: bool) {
        if let Some(config) = &mut self.config {
            config.no_logging = no_logging;
        }
    }

    /// Set custom word definitions (for hybrid engines only).
    pub fn set_profile_words(&mut self, words: Option<String>) {
        if let Some(config) = &mut self.config {
            config.profile_words = words;
        }
    }

    /// Enable or disable speaker diarization.
    pub fn set_diarization(&mut self, enabled: bool) {
        if let Some(config) = &mut self.config {
            config.enable_diarization = enabled;
        }
    }

    /// Get the AmiVoice-specific configuration.
    pub fn get_amivoice_config(&self) -> Option<&AmiVoiceSTTConfig> {
        self.config.as_ref()
    }
}

impl Drop for AmiVoiceSTT {
    fn drop(&mut self) {
        // Send shutdown signal if still connected
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

// =============================================================================
// BaseSTT Trait Implementation
// =============================================================================

#[async_trait::async_trait]
impl BaseSTT for AmiVoiceSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "AmiVoice APPKEY is required".to_string(),
            ));
        }

        // Create AmiVoice-specific configuration
        let amivoice_config = AmiVoiceSTTConfig::from_base(config);

        Ok(Self {
            config: Some(amivoice_config),
            state: ConnectionState::Disconnected,
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            result_tx: None,
            error_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Check if already connected
        if self.connected.load(Ordering::Acquire) {
            return Err(STTError::ConnectionFailed(
                "Already connected to AmiVoice".to_string(),
            ));
        }

        self.start_connection().await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for connection task to finish with timeout
        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // Clean up result forwarding task
        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Clean up error forwarding task
        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Clear all channels
        self.ws_sender = None;
        self.result_tx = None;
        self.error_tx = None;

        // Clear callbacks
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;

        // Update state
        self.state = ConnectionState::Disconnected;
        self.connected.store(false, Ordering::Release);
        self.state_notify.notify_waiters();

        info!("Disconnected from AmiVoice Speech-to-Text");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to AmiVoice Speech-to-Text".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();

            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio data: {e}")))?;

            debug!("Queued {} bytes of audio for AmiVoice STT", data_len);
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
        // Disconnect if currently connected
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Preserve AmiVoice-specific settings from existing config
        let existing = self.config.take();

        let amivoice_config = AmiVoiceSTTConfig {
            base: config.clone(),
            app_key: config.api_key.clone(),
            engine: existing.as_ref().map(|c| c.engine).unwrap_or_default(),
            audio_format: existing
                .as_ref()
                .map(|c| c.audio_format)
                .unwrap_or_default(),
            no_logging: existing.as_ref().is_some_and(|c| c.no_logging),
            interim_results: existing.as_ref().map(|c| c.interim_results).unwrap_or(true),
            result_updated_interval: existing
                .as_ref()
                .map(|c| c.result_updated_interval)
                .unwrap_or(1000),
            profile_words: existing.as_ref().and_then(|c| c.profile_words.clone()),
            profile_id: existing.as_ref().and_then(|c| c.profile_id.clone()),
            enable_sentiment: existing.as_ref().is_some_and(|c| c.enable_sentiment),
            enable_diarization: existing.as_ref().is_some_and(|c| c.enable_diarization),
            segmenter_properties: existing
                .as_ref()
                .and_then(|c| c.segmenter_properties.clone()),
            connection_timeout_secs: existing
                .as_ref()
                .map(|c| c.connection_timeout_secs)
                .unwrap_or(30),
            inactivity_timeout_secs: existing
                .as_ref()
                .map(|c| c.inactivity_timeout_secs)
                .unwrap_or(30),
        };

        self.config = Some(amivoice_config);

        // Reconnect with new configuration
        self.connect().await
    }

    fn get_provider_info(&self) -> &'static str {
        "AmiVoice Cloud Platform (Advanced Media)"
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_appkey".to_string(),
            language: "ja".to_string(),
            sample_rate: 16000,
            model: "-a-general".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = AmiVoiceSTT::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..make_test_config()
        };

        let result = AmiVoiceSTT::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("APPKEY"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_config_parsing() {
        let config = STTConfig {
            api_key: "test_key".to_string(),
            sample_rate: 8000,
            model: "-a-medical".to_string(),
            ..Default::default()
        };

        let stt = AmiVoiceSTT::new(config).unwrap();
        let amivoice_config = stt.get_amivoice_config().unwrap();

        assert_eq!(amivoice_config.app_key, "test_key");
        assert_eq!(
            amivoice_config.engine,
            super::super::config::AmiVoiceEngine::HybridJapaneseMedical
        );
        assert_eq!(
            amivoice_config.audio_format,
            super::super::config::AmiVoiceAudioFormat::Pcm8kHz
        );
    }

    #[test]
    fn test_provider_info() {
        let stt = AmiVoiceSTT::new(make_test_config()).unwrap();
        let info = stt.get_provider_info();
        assert!(info.contains("AmiVoice"));
        assert!(info.contains("Advanced Media"));
    }

    #[test]
    fn test_set_engine() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();
        stt.set_engine(super::super::config::AmiVoiceEngine::E2EJapaneseGeneral);

        let config = stt.get_amivoice_config().unwrap();
        assert_eq!(
            config.engine,
            super::super::config::AmiVoiceEngine::E2EJapaneseGeneral
        );
    }

    #[test]
    fn test_set_interim_results() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

        stt.set_interim_results(false);
        assert!(!stt.get_amivoice_config().unwrap().interim_results);

        stt.set_interim_results(true);
        assert!(stt.get_amivoice_config().unwrap().interim_results);
    }

    #[test]
    fn test_set_no_logging() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

        stt.set_no_logging(true);
        assert!(stt.get_amivoice_config().unwrap().no_logging);

        let url = stt.get_amivoice_config().unwrap().get_websocket_url();
        assert!(url.contains("nolog"));
    }

    #[test]
    fn test_set_diarization() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

        stt.set_diarization(true);
        assert!(stt.get_amivoice_config().unwrap().enable_diarization);

        let cmd = stt.get_amivoice_config().unwrap().build_start_command();
        assert!(cmd.contains("useDiarizer=1"));
    }

    #[test]
    fn test_default_state() {
        let stt = AmiVoiceSTT::default();
        assert!(stt.config.is_none());
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();
        let audio = Bytes::from_static(b"test audio");

        let result = stt.send_audio(audio).await;
        assert!(result.is_err());

        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("Not connected"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    #[tokio::test]
    async fn test_connect_already_connected() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

        // Simulate being connected
        stt.connected.store(true, Ordering::Release);

        let result = stt.connect().await;
        assert!(result.is_err());

        match result {
            Err(STTError::ConnectionFailed(msg)) => {
                assert!(msg.contains("Already connected"));
            }
            _ => panic!("Expected ConnectionFailed error"),
        }
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

        let result_called = Arc::new(AtomicBool::new(false));
        let result_called_clone = result_called.clone();

        stt.on_result(Arc::new(move |_result| {
            let flag = result_called_clone.clone();
            Box::pin(async move {
                flag.store(true, Ordering::Release);
            })
        }))
        .await
        .unwrap();

        // Verify callback is stored
        let guard = stt.result_callback.lock().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_error_callback_registration() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

        let error_called = Arc::new(AtomicBool::new(false));
        let error_called_clone = error_called.clone();

        stt.on_error(Arc::new(move |_error| {
            let flag = error_called_clone.clone();
            Box::pin(async move {
                flag.store(true, Ordering::Release);
            })
        }))
        .await
        .unwrap();

        // Verify callback is stored
        let guard = stt.error_callback.lock().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let mut stt = AmiVoiceSTT::new(make_test_config()).unwrap();

        // Should not fail even if not connected
        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_get_config() {
        let config = make_test_config();
        let stt = AmiVoiceSTT::new(config.clone()).unwrap();

        let retrieved_config = stt.get_config().unwrap();
        assert_eq!(retrieved_config.api_key, config.api_key);
        assert_eq!(retrieved_config.language, config.language);
    }
}
