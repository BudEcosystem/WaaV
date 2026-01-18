//! iFlytek STT WebSocket Client
//!
//! This module implements the `BaseSTT` trait for iFlytek's Speech-to-Text WebSocket API.
//!
//! # Architecture
//!
//! The client uses a WebSocket connection for real-time streaming:
//!
//! 1. Connect with HMAC-SHA256 signed URL
//! 2. Send first frame with app_id and business parameters
//! 3. Stream audio frames (1280 bytes @ 40ms intervals)
//! 4. Receive partial results with dynamic correction
//! 5. Send last frame and receive final result
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect (with auth) -------->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ First frame (status=0) ----->|
//!   |------ Audio frames (status=1) ---->|
//!   |------ Last frame (status=2) ------>|
//!   |                                    |
//!   |<----- Partial results -------------|
//!   |<----- Final result (status=2) -----|
//!   |                                    |
//!   |<----- Server closes connection ----|
//! ```

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::super::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use super::config::IFlytekSttConfig;
use super::messages::{SttRequest, SttResponse};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "iFlytek STT WebSocket v2.0 (科大讯飞)";

/// WebSocket connection timeout.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// WebSocket message timeout (idle detection).
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// Audio frame interval for streaming.
const FRAME_INTERVAL: Duration = Duration::from_millis(40);

/// Default audio frame size (1280 bytes @ 16kHz, 40ms).
const DEFAULT_FRAME_SIZE: usize = 1280;

/// Channel buffer size for audio frames.
const AUDIO_CHANNEL_BUFFER: usize = 64;

/// Channel buffer size for results.
const RESULT_CHANNEL_BUFFER: usize = 256;

/// Channel buffer size for errors.
const ERROR_CHANNEL_BUFFER: usize = 64;

// =============================================================================
// Type Aliases
// =============================================================================

/// Async callback type for STT results.
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Async callback type for errors.
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

// =============================================================================
// iFlytek STT Client
// =============================================================================

/// iFlytek Speech-to-Text WebSocket client.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::iflytek::IFlytekStt;
///
/// let config = STTConfig {
///     api_key: "app_id|api_key|api_secret".to_string(),
///     language: "zh_cn".to_string(),
///     sample_rate: 16000,
///     ..Default::default()
/// };
///
/// let mut stt = IFlytekStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct IFlytekStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// iFlytek-specific configuration.
    config: IFlytekSttConfig,

    /// Connection state.
    connected: AtomicBool,

    /// State change notification.
    state_notify: Arc<Notify>,

    /// WebSocket sender for audio data.
    ws_sender: Option<mpsc::Sender<Bytes>>,

    /// Shutdown signal sender.
    shutdown_tx: Option<oneshot::Sender<()>>,

    /// Connection task handle.
    connection_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result forwarding task handle.
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Error forwarding task handle.
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Result callback storage.
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,

    /// Error callback storage.
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,

    /// Frame counter for status tracking.
    frame_count: Arc<std::sync::atomic::AtomicU64>,
}

impl IFlytekStt {
    /// Create a new iFlytek STT client.
    fn create_internal(config: STTConfig) -> Result<Self, STTError> {
        let iflytek_config = IFlytekSttConfig::from_base(config.clone())?;

        // Validate configuration
        iflytek_config.validate()?;

        Ok(Self {
            base_config: config,
            config: iflytek_config,
            connected: AtomicBool::new(false),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            frame_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        })
    }

    /// Public constructor.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        Self::create_internal(config)
    }

    /// Handle incoming WebSocket message.
    fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) -> Result<bool, STTError> {
        match message {
            Message::Text(text) => {
                debug!("iFlytek STT received: {}", text);

                let response = SttResponse::from_json(&text)
                    .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

                // Check for errors
                if !response.is_success() {
                    let error_code = response.error_code();
                    let error = STTError::ProviderError(format!(
                        "iFlytek error: {}",
                        error_code
                    ));

                    // Send error through channel
                    if let Err(e) = error_tx.try_send(error.clone()) {
                        warn!("Failed to send error: {:?}", e);
                    }

                    // Retryable errors don't terminate the connection
                    if !error_code.is_retryable() {
                        return Err(error);
                    }
                    return Ok(false);
                }

                // Extract transcript
                if let Some(transcript) = response.transcript() {
                    if !transcript.is_empty() {
                        let is_final = response.is_final();
                        let is_replacement = response.is_replacement();

                        let result = STTResult::new(
                            transcript,
                            is_final,
                            is_final, // speech_final matches is_final for iFlytek
                            response.confidence(),
                        );

                        // Log replacement events for debugging
                        if is_replacement {
                            debug!(
                                "iFlytek dynamic correction: sn={:?}",
                                response.sentence_number()
                            );
                        }

                        // Send result through channel
                        if let Err(e) = result_tx.try_send(result) {
                            match e {
                                mpsc::error::TrySendError::Full(_) => {
                                    warn!("iFlytek result channel full - dropping result");
                                }
                                mpsc::error::TrySendError::Closed(_) => {
                                    warn!("iFlytek result channel closed");
                                }
                            }
                        }
                    }
                }

                // Return true if this is the final response
                Ok(response.is_final())
            }
            Message::Close(frame) => {
                info!("iFlytek WebSocket closed: {:?}", frame);
                Ok(true)
            }
            Message::Ping(_) => {
                debug!("iFlytek received ping");
                Ok(false)
            }
            Message::Pong(_) => {
                debug!("iFlytek received pong");
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Start the WebSocket connection task.
    async fn start_connection(&mut self) -> Result<(), STTError> {
        // Build signed WebSocket URL
        let ws_url = self.config.auth
            .build_signed_url(self.config.host(), self.config.path())
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to build signed URL: {}", e)))?;

        debug!("Connecting to iFlytek: {}", ws_url);

        // Create channels
        let (ws_tx, mut ws_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let (connected_tx, connected_rx) = oneshot::channel::<()>();

        // Store channels
        self.ws_sender = Some(ws_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Clone config values for the connection task
        let app_id = self.config.auth.app_id.clone();
        let language = self.config.language.as_code().to_string();
        let domain = self.config.domain.as_str().to_string();
        let accent = self.config.accent.clone();
        let vad_eos_ms = self.config.vad_eos_ms;
        let dynamic_correction = self.config.dynamic_correction;
        let punctuation = self.config.punctuation;
        let convert_numbers = self.config.convert_numbers;
        let audio_format = self.config.audio_format_string();
        let encoding = self.config.encoding.as_str().to_string();
        let frame_count = self.frame_count.clone();

        // Start connection task
        let connection_handle = tokio::spawn(async move {
            // Connect to WebSocket
            let ws_stream = match timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url)).await {
                Ok(Ok((stream, _))) => stream,
                Ok(Err(e)) => {
                    let err = STTError::ConnectionFailed(format!("WebSocket connection failed: {}", e));
                    error!("{}", err);
                    let _ = error_tx.try_send(err);
                    return;
                }
                Err(_) => {
                    let err = STTError::ConnectionFailed("Connection timeout".to_string());
                    error!("{}", err);
                    let _ = error_tx.try_send(err);
                    return;
                }
            };

            info!("Connected to iFlytek STT WebSocket");
            let _ = connected_tx.send(());

            let (mut ws_sink, mut ws_stream) = ws_stream.split();

            // Frame interval timer
            let mut frame_timer = interval(FRAME_INTERVAL);
            let mut _last_activity = Instant::now();
            let mut is_first_frame = true;
            let mut audio_buffer: Vec<u8> = Vec::with_capacity(DEFAULT_FRAME_SIZE * 2);
            let mut session_ended = false;

            loop {
                tokio::select! {
                    // Handle incoming audio data
                    Some(audio_data) = ws_rx.recv() => {
                        audio_buffer.extend_from_slice(&audio_data);
                        _last_activity = Instant::now();
                    }

                    // Send frames at regular intervals
                    _ = frame_timer.tick() => {
                        if session_ended {
                            break;
                        }

                        // Check if we have enough data to send
                        if audio_buffer.len() >= DEFAULT_FRAME_SIZE || !is_first_frame {
                            let frame_size = audio_buffer.len().min(DEFAULT_FRAME_SIZE);
                            let frame_data: Vec<u8> = audio_buffer.drain(..frame_size).collect();

                            let request = if is_first_frame {
                                is_first_frame = false;
                                SttRequest::first_frame(
                                    &app_id,
                                    &language,
                                    &domain,
                                    Some(&accent),
                                    vad_eos_ms,
                                    dynamic_correction,
                                    punctuation,
                                    convert_numbers,
                                    &audio_format,
                                    &encoding,
                                    &frame_data,
                                )
                            } else {
                                SttRequest::continue_frame(
                                    &app_id,
                                    &audio_format,
                                    &encoding,
                                    &frame_data,
                                )
                            };

                            let json = match request.to_json() {
                                Ok(j) => j,
                                Err(e) => {
                                    error!("Failed to serialize request: {}", e);
                                    continue;
                                }
                            };

                            if let Err(e) = ws_sink.send(Message::Text(json.into())).await {
                                let err = STTError::NetworkError(format!("Failed to send frame: {}", e));
                                error!("{}", err);
                                let _ = error_tx.try_send(err);
                                break;
                            }

                            frame_count.fetch_add(1, Ordering::Relaxed);
                            debug!("Sent iFlytek frame #{}", frame_count.load(Ordering::Relaxed));
                        }
                    }

                    // Handle incoming messages with timeout
                    message = timeout(WS_MESSAGE_TIMEOUT, ws_stream.next()) => {
                        match message {
                            Ok(Some(Ok(msg))) => {
                                match Self::handle_websocket_message(msg, &result_tx, &error_tx) {
                                    Ok(is_final) => {
                                        if is_final {
                                            info!("iFlytek STT session complete");
                                            session_ended = true;
                                        }
                                    }
                                    Err(e) => {
                                        error!("iFlytek message handling error: {}", e);
                                        break;
                                    }
                                }
                            }
                            Ok(Some(Err(e))) => {
                                let err = STTError::NetworkError(format!("WebSocket error: {}", e));
                                error!("{}", err);
                                let _ = error_tx.try_send(err);
                                break;
                            }
                            Ok(None) => {
                                info!("iFlytek WebSocket stream ended");
                                break;
                            }
                            Err(_) => {
                                let err = STTError::NetworkError(
                                    "iFlytek WebSocket idle timeout".to_string()
                                );
                                error!("{}", err);
                                let _ = error_tx.try_send(err);
                                break;
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        info!("iFlytek STT shutdown signal received");

                        // Send remaining buffer as last frame
                        if !audio_buffer.is_empty() || !is_first_frame {
                            let request = SttRequest::last_frame(
                                &app_id,
                                &audio_format,
                                &encoding,
                                &audio_buffer,
                            );

                            if let Ok(json) = request.to_json() {
                                let _ = ws_sink.send(Message::Text(json.into())).await;
                                debug!("Sent iFlytek last frame");
                            }
                        }

                        break;
                    }
                }
            }

            // Close WebSocket gracefully
            let _ = ws_sink.close().await;
            info!("iFlytek STT WebSocket connection closed");
        });

        self.connection_handle = Some(connection_handle);

        // Start result forwarding task
        let callback_ref = self.result_callback.clone();
        let result_forward_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                if let Some(callback) = callback_ref.lock().await.as_ref() {
                    callback(result).await;
                } else {
                    debug!(
                        "iFlytek STT result (no callback): {} (confidence: {})",
                        result.transcript, result.confidence
                    );
                }
            }
        });
        self.result_forward_handle = Some(result_forward_handle);

        // Start error forwarding task
        let error_callback_ref = self.error_callback.clone();
        let error_forward_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                if let Some(callback) = error_callback_ref.lock().await.as_ref() {
                    callback(error).await;
                } else {
                    error!("iFlytek STT error (no callback): {}", error);
                }
            }
        });
        self.error_forward_handle = Some(error_forward_handle);

        // Wait for connection to be established
        match timeout(WS_CONNECT_TIMEOUT, connected_rx).await {
            Ok(Ok(())) => {
                self.connected.store(true, Ordering::SeqCst);
                self.state_notify.notify_waiters();
                info!("iFlytek STT connected successfully");
                Ok(())
            }
            Ok(Err(_)) => {
                Err(STTError::ConnectionFailed("Connection channel closed".to_string()))
            }
            Err(_) => {
                Err(STTError::ConnectionFailed("Connection timeout".to_string()))
            }
        }
    }
}

impl Default for IFlytekStt {
    fn default() -> Self {
        Self {
            base_config: STTConfig::default(),
            config: IFlytekSttConfig::default(),
            connected: AtomicBool::new(false),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            frame_count: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for IFlytekStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        Self::create_internal(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!(
            "Connecting to iFlytek STT (language: {}, mode: {:?})",
            self.config.language.display_name(),
            self.config.mode
        );

        self.frame_count.store(0, Ordering::Relaxed);
        self.start_connection().await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from iFlytek STT");

        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for connection task to finish
        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // Clean up forwarding tasks
        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }
        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
            let _ = handle.await;
        }

        // Clear state
        self.ws_sender = None;
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;
        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!(
            "Disconnected from iFlytek STT (frames sent: {})",
            self.frame_count.load(Ordering::Relaxed)
        );
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to iFlytek STT".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();

            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to queue audio: {}", e)))?;

            debug!("Queued {} bytes of audio for iFlytek", data_len);
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
        Some(&self.base_config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Need to reconnect to update config
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Parse new config
        let iflytek_config = IFlytekSttConfig::from_base(config.clone())?;
        iflytek_config.validate()?;

        self.base_config = config;
        self.config = iflytek_config;

        self.connect().await
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }
}

impl Drop for IFlytekStt {
    fn drop(&mut self) {
        // Send shutdown signal if still connected
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_api_key() -> String {
        "test_app_id|test_api_key_xxxxx|test_api_secret_xx".to_string()
    }

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: create_test_api_key(),
            language: "zh_cn".to_string(),
            sample_rate: 16000,
            encoding: "raw".to_string(),
            punctuation: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_iflytek_stt_creation() {
        let config = create_test_config();
        let result = IFlytekStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), PROVIDER_INFO);
    }

    #[test]
    fn test_iflytek_stt_invalid_api_key() {
        let config = STTConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };
        let result = IFlytekStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_iflytek_stt_get_config() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();

        let stored = stt.get_config();
        assert!(stored.is_some());
        assert_eq!(stored.unwrap().language, "zh_cn");
    }

    #[test]
    fn test_iflytek_stt_initial_state() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();

        assert!(!stt.is_ready());
        assert_eq!(stt.frame_count.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = IFlytekStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());

        if let Err(STTError::ConnectionFailed(_)) = result {
            // Expected
        } else {
            panic!("Expected ConnectionFailed error");
        }
    }

    #[test]
    fn test_iflytek_stt_default() {
        let stt = IFlytekStt::default();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = IFlytekStt::new(config).unwrap();

        let info = stt.get_provider_info();
        assert!(info.contains("iFlytek"));
        assert!(info.contains("科大讯飞"));
    }

    #[test]
    fn test_message_handling_success() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "你好", "sc": 0.95}]}],
                    "sn": 1,
                    "ls": false
                },
                "status": 1
            }
        }"#;

        let (result_tx, mut result_rx) = mpsc::channel(256);
        let (error_tx, _error_rx) = mpsc::channel(64);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekStt::handle_websocket_message(message, &result_tx, &error_tx);
        assert!(result.is_ok());
        assert!(!result.unwrap()); // Not final

        // Check result was sent
        let received = result_rx.try_recv();
        assert!(received.is_ok());
        assert_eq!(received.unwrap().transcript, "你好");
    }

    #[test]
    fn test_message_handling_error() {
        let json = r#"{
            "code": 10005,
            "message": "authorization failure",
            "sid": "test_sid"
        }"#;

        let (result_tx, _result_rx) = mpsc::channel(256);
        let (error_tx, mut error_rx) = mpsc::channel(64);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekStt::handle_websocket_message(message, &result_tx, &error_tx);
        assert!(result.is_err());

        // Check error was sent
        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_message_handling_final() {
        let json = r#"{
            "code": 0,
            "message": "success",
            "sid": "test_sid",
            "data": {
                "result": {
                    "ws": [{"bg": 0, "cw": [{"w": "完成"}]}],
                    "sn": 1,
                    "ls": true
                },
                "status": 2
            }
        }"#;

        let (result_tx, _) = mpsc::channel(256);
        let (error_tx, _) = mpsc::channel(64);

        let message = Message::Text(json.to_string().into());
        let result = IFlytekStt::handle_websocket_message(message, &result_tx, &error_tx);
        assert!(result.is_ok());
        assert!(result.unwrap()); // Final
    }

    #[test]
    fn test_constants() {
        assert_eq!(DEFAULT_FRAME_SIZE, 1280);
        assert_eq!(FRAME_INTERVAL, Duration::from_millis(40));
        assert_eq!(WS_CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(WS_MESSAGE_TIMEOUT, Duration::from_secs(60));
    }
}
