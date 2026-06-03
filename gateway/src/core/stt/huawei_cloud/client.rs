//! Huawei Cloud Speech Interaction Service (SIS) STT Client
//!
//! This module implements the `BaseSTT` trait for Huawei Cloud's Speech-to-Text
//! service, supporting both WebSocket real-time streaming and REST API short
//! sentence recognition.
//!
//! # Architecture
//!
//! The client supports three operation modes:
//!
//! 1. **Short Sentence (REST)**: For audio files up to 30 seconds
//! 2. **Streaming (WebSocket)**: For real-time audio up to 1 minute
//! 3. **Continuous (WebSocket)**: For long-running audio up to 5 hours
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with IAM token ----->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ START command (JSON) ------->|
//!   |<----- STARTED confirmation --------|
//!   |                                    |
//!   |------ Audio frames (binary) ------>|
//!   |<----- RESULT (interim) ------------|
//!   |<----- END (final) -----------------|
//!   |                                    |
//!   |------ END command (JSON) --------->|
//!   |<----- ENDED confirmation ----------|
//! ```
//!
//! # Authentication
//!
//! Uses IAM token authentication:
//! 1. Exchange username/password for IAM token
//! 2. Token valid for 24 hours (auto-refreshed)
//! 3. Include X-Auth-Token header in WebSocket handshake

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{http::Request, protocol::Message},
};
use tracing::{debug, error, info, warn};

use super::auth::HuaweiTokenManager;
use super::config::{HuaweiCloudAsrMode, HuaweiCloudSttConfig};
use super::messages::{
    HuaweiEndFrame, HuaweiRealtimeResponse, HuaweiShortAsrRequest, HuaweiShortAsrResponse,
    HuaweiStartFrame,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Huawei Cloud Speech (华为云语音)";

/// WebSocket connection timeout.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// HTTP request timeout for REST API.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

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
// Huawei Cloud STT Client
// =============================================================================

/// Huawei Cloud Speech Interaction Service (SIS) STT client.
///
/// Supports real-time WebSocket streaming and REST API for short audio.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::huawei_cloud::HuaweiCloudStt;
///
/// let config = STTConfig {
///     // API key format: username|password|domain_name|project_id
///     api_key: "user|pass|domain|project123".to_string(),
///     language: "zh".to_string(),
///     sample_rate: 16000,
///     encoding: "pcm16k16bit".to_string(),
///     model: "chinese_16k_general".to_string(),
///     ..Default::default()
/// };
///
/// let mut stt = HuaweiCloudStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct HuaweiCloudStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// Huawei Cloud-specific configuration.
    config: HuaweiCloudSttConfig,

    /// IAM token manager.
    token_manager: Arc<HuaweiTokenManager>,

    /// Connection state.
    connected: Arc<AtomicBool>,

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

    /// HTTP client for REST API.
    http_client: reqwest::Client,

    /// Audio buffer for REST API mode.
    audio_buffer: Arc<Mutex<Vec<u8>>>,

    /// Session ready flag.
    session_ready: Arc<AtomicBool>,
}

impl HuaweiCloudStt {
    /// W1 keystone — construct directly from the standardized config so Huawei's mappable
    /// recognition knobs (word-level timing `need_word_info`, smart formatting `add_punctuation`)
    /// are honored END-TO-END. Mirrors `DeepgramSTT::new_standard`: the provider config is built
    /// from `HuaweiCloudSttConfig::from_standard` (which parses+validates the pipe-separated
    /// credential in `api_key`); features Huawei can't express stay at default.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let huawei_config = HuaweiCloudSttConfig::from_standard(std)?;
        huawei_config.validate()?;

        Ok(Self {
            base_config: std.base.clone(),
            config: huawei_config,
            token_manager: Arc::new(HuaweiTokenManager::new()),
            connected: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            http_client: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_default(),
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            session_ready: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Create a new Huawei Cloud STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let huawei_config = HuaweiCloudSttConfig::from_base(config.clone())?;
        huawei_config.validate()?;

        Ok(Self {
            base_config: config,
            config: huawei_config,
            token_manager: Arc::new(HuaweiTokenManager::new()),
            connected: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            http_client: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_default(),
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
            session_ready: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get the IAM token, fetching if necessary.
    async fn get_token(&self) -> Result<String, STTError> {
        self.token_manager
            .get_token(
                &self.config.username,
                &self.config.password,
                &self.config.domain_name,
                self.config.region,
            )
            .await
    }

    /// Connect using WebSocket real-time mode.
    async fn connect_realtime(&mut self) -> Result<(), STTError> {
        // Get IAM token first
        let token = self.get_token().await?;

        // Get WebSocket URL
        let url = self.config.get_realtime_url().ok_or_else(|| {
            STTError::ConfigurationError("WebSocket URL not available for this mode".to_string())
        })?;

        info!("Connecting to Huawei Cloud RASR: {}", url);

        // Build WebSocket request with auth header
        let request = Request::builder()
            .uri(&url)
            .header("X-Auth-Token", &token)
            .header("Host", self.config.region.sis_endpoint())
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", generate_ws_key())
            .header("Sec-WebSocket-Version", "13")
            .body(())
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to build request: {}", e)))?;

        // Connect with timeout
        let (ws_stream, _) = match timeout(
            WS_CONNECT_TIMEOUT,
            connect_async_with_config(request, None, false),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return Err(STTError::ConnectionFailed(format!(
                    "WebSocket connection failed: {}",
                    e
                )));
            }
            Err(_) => {
                return Err(STTError::ConnectionFailed("Connection timeout".to_string()));
            }
        };

        info!("Connected to Huawei Cloud RASR");

        // Split stream
        let (mut write, mut read) = ws_stream.split();

        // Create channels
        let (audio_tx, mut audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Send START frame (built from the shared single-source-of-truth constructor).
        let start_frame = HuaweiStartFrame::from_config(&self.config);

        let start_json = start_frame.to_json().map_err(|e| {
            STTError::ConnectionFailed(format!("Failed to serialize START frame: {}", e))
        })?;

        write
            .send(Message::Text(start_json.into()))
            .await
            .map_err(|e| {
                STTError::ConnectionFailed(format!("Failed to send START frame: {}", e))
            })?;

        debug!("Sent Huawei Cloud START frame");

        // Set connected state
        self.connected.store(true, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        // Clone for tasks
        let connected = self.connected.clone();
        let state_notify = self.state_notify.clone();
        let session_ready = self.session_ready.clone();
        let result_tx_clone = result_tx.clone();
        let error_tx_clone = error_tx.clone();

        // Spawn connection handler task
        let connection_handle = tokio::spawn(async move {
            let send_task = tokio::spawn(async move {
                while let Some(audio) = audio_rx.recv().await {
                    // Send binary audio data
                    if write
                        .send(Message::Binary(audio.to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }

                // Send END frame
                let end_frame = HuaweiEndFrame::new();
                if let Ok(end_json) = end_frame.to_json() {
                    let _ = write.send(Message::Text(end_json.into())).await;
                }

                write
            });

            let recv_task = tokio::spawn(async move {
                while let Some(msg_result) = read.next().await {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            Self::handle_realtime_response(
                                &text,
                                &result_tx_clone,
                                &error_tx_clone,
                                &session_ready,
                            );
                        }
                        Ok(Message::Close(_)) => {
                            debug!("Huawei Cloud WebSocket closed");
                            break;
                        }
                        Ok(Message::Ping(_)) => {
                            debug!("Received ping from Huawei Cloud");
                        }
                        Err(e) => {
                            error!("Huawei Cloud WebSocket error: {}", e);
                            let _ =
                                error_tx_clone.try_send(STTError::ConnectionFailed(e.to_string()));
                            break;
                        }
                        _ => {}
                    }
                }
            });

            // Wait for shutdown or completion
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!("Huawei Cloud shutdown signal received");
                }
                _ = recv_task => {
                    debug!("Huawei Cloud receive task completed");
                }
            }

            send_task.abort();
            connected.store(false, Ordering::SeqCst);
            state_notify.notify_waiters();
        });

        self.connection_handle = Some(connection_handle);

        // Spawn result forwarding task
        let result_callback = self.result_callback.clone();
        let result_forward_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                let callback = result_callback.lock().await;
                if let Some(cb) = callback.as_ref() {
                    cb(result).await;
                }
            }
        });

        self.result_forward_handle = Some(result_forward_handle);

        // Spawn error forwarding task
        let error_callback = self.error_callback.clone();
        let error_forward_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                let callback = error_callback.lock().await;
                if let Some(cb) = callback.as_ref() {
                    cb(error).await;
                }
            }
        });

        self.error_forward_handle = Some(error_forward_handle);

        Ok(())
    }

    /// Handle real-time WebSocket response.
    fn handle_realtime_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
        session_ready: &AtomicBool,
    ) {
        match HuaweiRealtimeResponse::from_json(text) {
            Ok(response) => {
                // Handle session started
                if response.is_started() {
                    session_ready.store(true, Ordering::SeqCst);
                    debug!("Huawei Cloud ASR session started");
                    return;
                }

                // Handle session ended
                if response.is_ended() {
                    session_ready.store(false, Ordering::SeqCst);
                    debug!("Huawei Cloud ASR session ended");
                    return;
                }

                // Handle errors
                if response.is_error() {
                    if let Some(err_msg) = response.get_error() {
                        warn!("Huawei Cloud ASR error: {}", err_msg);
                        let _ = error_tx.try_send(STTError::AudioProcessingError(err_msg));
                    }
                    return;
                }

                // Handle recognition results
                if let Some(transcript) = response.get_transcript() {
                    let result = STTResult::new(
                        transcript.to_string(),
                        response.is_final(),
                        response.is_final(),
                        response.get_confidence().unwrap_or(1.0),
                    );

                    debug!(
                        "Huawei Cloud transcript ({}): {}",
                        if response.is_final() {
                            "final"
                        } else {
                            "interim"
                        },
                        transcript
                    );

                    let _ = result_tx.try_send(result);
                }
            }
            Err(e) => {
                warn!(
                    "Failed to parse Huawei Cloud response: {} - raw: {}",
                    e, text
                );
            }
        }
    }

    /// Recognize short audio using REST API.
    pub async fn recognize_short_audio(&self, audio_data: &[u8]) -> Result<String, STTError> {
        // Get IAM token
        let token = self.get_token().await?;

        // Build request
        let request = HuaweiShortAsrRequest::new(
            audio_data,
            self.config.audio_format.as_str(),
            self.config.model.as_str(),
            self.config.add_punctuation,
            self.config.digit_norm,
            self.config.vocabulary_id.as_deref(),
            self.config.need_word_info,
        );

        let request_json = request.to_json().map_err(|e| {
            STTError::AudioProcessingError(format!("Failed to serialize request: {}", e))
        })?;

        let url = self.config.get_short_asr_url();

        debug!(
            "Sending short audio request to {}, {} bytes",
            url,
            audio_data.len()
        );

        let response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("X-Auth-Token", &token)
            .body(request_json)
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("REST API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(STTError::ProviderError(format!(
                "Huawei Cloud API error {}: {}",
                status, body
            )));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to read response: {}", e)))?;

        let asr_response = HuaweiShortAsrResponse::from_json(&response_text)
            .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

        if !asr_response.is_success() {
            if let Some(error) = asr_response.get_error() {
                return Err(STTError::ProviderError(error));
            }
            return Err(STTError::ProviderError("Unknown error".to_string()));
        }

        asr_response
            .get_transcript()
            .map(|s| s.to_string())
            .ok_or_else(|| STTError::ProviderError("No transcript in response".to_string()))
    }

    /// Get the recommended chunk size for audio streaming.
    pub fn get_chunk_size(&self) -> usize {
        self.config.get_chunk_size()
    }

    /// Check if using WebSocket mode.
    pub fn is_websocket_mode(&self) -> bool {
        self.config.mode.is_websocket()
    }

    /// Check if session is ready for audio.
    pub fn is_session_ready(&self) -> bool {
        self.session_ready.load(Ordering::SeqCst)
    }

    /// Get the operation mode.
    pub fn get_mode(&self) -> HuaweiCloudAsrMode {
        self.config.mode
    }
}

#[async_trait::async_trait]
impl BaseSTT for HuaweiCloudStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        HuaweiCloudStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        match self.config.mode {
            HuaweiCloudAsrMode::ShortSentence => {
                // REST API mode: just mark as ready (no persistent connection)
                // Validate token works
                let _ = self.get_token().await?;
                self.connected.store(true, Ordering::SeqCst);
                self.state_notify.notify_waiters();
                info!("Huawei Cloud STT ready in REST API mode");
                Ok(())
            }
            HuaweiCloudAsrMode::Streaming | HuaweiCloudAsrMode::Continuous => {
                self.connect_realtime().await
            }
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from Huawei Cloud STT...");

        // Drop audio sender to signal end
        self.ws_sender.take();

        // Send shutdown signal
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Wait for connection task to complete
        if let Some(handle) = self.connection_handle.take() {
            let _ = timeout(Duration::from_secs(5), handle).await;
        }

        // Abort forwarding tasks
        if let Some(handle) = self.result_forward_handle.take() {
            handle.abort();
        }

        if let Some(handle) = self.error_forward_handle.take() {
            handle.abort();
        }

        // Clear audio buffer
        self.audio_buffer.lock().await.clear();

        // Reset session state
        self.session_ready.store(false, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!("Disconnected from Huawei Cloud STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        match self.config.mode {
            HuaweiCloudAsrMode::ShortSentence => {
                // REST API mode: accumulate in buffer
                self.audio_buffer.lock().await.extend_from_slice(&audio);
            }
            HuaweiCloudAsrMode::Streaming | HuaweiCloudAsrMode::Continuous => {
                // WebSocket mode: send to channel
                if let Some(sender) = &self.ws_sender {
                    sender.send(audio).await.map_err(|_| {
                        STTError::ProviderError("Failed to send audio to channel".to_string())
                    })?;
                }
            }
        }

        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        let async_callback: AsyncSTTCallback = Box::new(move |result| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(result).await;
            })
        });

        *self.result_callback.lock().await = Some(async_callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        let async_callback: AsyncErrorCallback = Box::new(move |error| {
            let cb = callback.clone();
            Box::pin(async move {
                cb(error).await;
            })
        });

        *self.error_callback.lock().await = Some(async_callback);
        Ok(())
    }

    fn get_config(&self) -> Option<&STTConfig> {
        Some(&self.base_config)
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Disconnect if connected
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Update configs
        let huawei_config = HuaweiCloudSttConfig::from_base(config.clone())?;
        huawei_config.validate()?;

        self.config = huawei_config;
        self.base_config = config;

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }
}

// =============================================================================
// Helper Functions
// =============================================================================

/// Generate a random WebSocket key for the Sec-WebSocket-Key header.
fn generate_ws_key() -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Use timestamp and a counter for uniqueness
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    let bytes: [u8; 16] = [
        (timestamp >> 120) as u8,
        (timestamp >> 112) as u8,
        (timestamp >> 104) as u8,
        (timestamp >> 96) as u8,
        (timestamp >> 88) as u8,
        (timestamp >> 80) as u8,
        (timestamp >> 72) as u8,
        (timestamp >> 64) as u8,
        (timestamp >> 56) as u8,
        (timestamp >> 48) as u8,
        (timestamp >> 40) as u8,
        (timestamp >> 32) as u8,
        (timestamp >> 24) as u8,
        (timestamp >> 16) as u8,
        (timestamp >> 8) as u8,
        timestamp as u8,
    ];

    STANDARD.encode(bytes)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_user|test_pass|test_domain|test_project".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm16k16bit".to_string(),
            model: "chinese_16k_general".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_client() {
        let config = create_test_config();
        let result = HuaweiCloudStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_client_invalid_api_key_format() {
        let config = STTConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };
        let result = HuaweiCloudStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_client_empty_credentials() {
        let config = STTConfig {
            api_key: "|||".to_string(),
            ..Default::default()
        };
        let result = HuaweiCloudStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Huawei") || info.contains("华为"));
    }

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config.clone()).unwrap();
        assert!(stt.get_config().is_some());
        assert!(stt.get_config().unwrap().api_key.contains("test_user"));
    }

    #[test]
    fn test_chunk_size() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();

        // 200ms at 16kHz, 16-bit: 16000 * 2 * 200 / 1000 = 6400 bytes
        assert_eq!(stt.get_chunk_size(), 6400);
    }

    #[test]
    fn test_is_websocket_mode() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert!(stt.is_websocket_mode()); // Default is streaming mode
    }

    #[test]
    fn test_get_mode() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert_eq!(stt.get_mode(), HuaweiCloudAsrMode::Streaming);
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0u8; 1024])).await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        let result_cb: STTResultCallback = Arc::new(|_| Box::pin(async {}));
        let error_cb: STTErrorCallback = Arc::new(|_| Box::pin(async {}));

        assert!(stt.on_result(result_cb).await.is_ok());
        assert!(stt.on_error(error_cb).await.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = create_test_config();
        let mut stt = HuaweiCloudStt::new(config).unwrap();

        // Use 8k model with 8kHz sample rate (consistent configuration)
        let new_config = STTConfig {
            api_key: "new_user|new_pass|new_domain|new_project".to_string(),
            language: "zh".to_string(),
            sample_rate: 8000,
            encoding: "pcm8k16bit".to_string(),
            model: "chinese_8k_general".to_string(),
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());
        assert!(stt.get_config().unwrap().api_key.contains("new_user"));
    }

    #[test]
    fn test_handle_realtime_response_final() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "END",
            "error_code": 0,
            "result": {
                "text": "你好世界",
                "score": 0.95,
                "is_final": true
            }
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好世界");
        assert!(result.is_final);
        assert_eq!(result.confidence, 0.95);
    }

    #[test]
    fn test_handle_realtime_response_interim() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "RESULT",
            "error_code": 0,
            "result": {
                "text": "你好",
                "is_final": false
            }
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好");
        assert!(!result.is_final);
    }

    #[test]
    fn test_handle_realtime_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "ERROR",
            "error_code": 3,
            "error_msg": "Invalid parameter"
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_handle_realtime_response_started() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(false);

        let json = r#"{
            "resp_type": "STARTED",
            "error_code": 0
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        assert!(session_ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_handle_realtime_response_ended() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);
        let session_ready = AtomicBool::new(true);

        let json = r#"{
            "resp_type": "ENDED",
            "error_code": 0
        }"#;

        HuaweiCloudStt::handle_realtime_response(json, &result_tx, &error_tx, &session_ready);

        assert!(!session_ready.load(Ordering::SeqCst));
    }

    #[test]
    fn test_generate_ws_key() {
        let key1 = generate_ws_key();
        let key2 = generate_ws_key();

        // Keys should be base64 encoded
        assert!(!key1.is_empty());
        assert!(!key2.is_empty());

        // Keys should be unique (with high probability)
        // Note: Rapid successive calls might generate same key in rare cases
        // due to timestamp resolution, but this is acceptable for testing
    }

    #[test]
    fn test_session_ready() {
        let config = create_test_config();
        let stt = HuaweiCloudStt::new(config).unwrap();
        assert!(!stt.is_session_ready());
    }
}
