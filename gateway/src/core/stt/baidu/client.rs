//! Baidu Speech Recognition WebSocket Client
//!
//! This module implements the `BaseSTT` trait for Baidu AI Cloud's
//! Speech-to-Text service, supporting both WebSocket real-time streaming
//! and REST API short audio recognition.
//!
//! # Architecture
//!
//! The client supports two operation modes:
//!
//! 1. **Real-time WebSocket**: For streaming audio recognition
//! 2. **REST API**: For short audio files (up to 60 seconds)
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with sn UUID ------->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ START frame (JSON) --------->|
//!   |                                    |
//!   |------ Audio frames (binary) ------>|
//!   |<----- MID_TEXT (interim) ----------|
//!   |<----- FIN_TEXT (final) ------------|
//!   |                                    |
//!   |------ FINISH frame (JSON) -------->|
//!   |<----- Close ----------------------|
//! ```
//!
//! # Authentication
//!
//! Baidu uses OAuth 2.0 client credentials for authentication:
//! 1. Exchange API Key + Secret Key for access token
//! 2. Token valid for 30 days (cached and refreshed automatically)
//! 3. WebSocket uses appid + appkey in START frame

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, RwLock, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::config::{BAIDU_OAUTH_URL, BaiduOAuthResponse, BaiduSttConfig};
use super::messages::{
    BaiduFinishFrame, BaiduRealtimeResponse, BaiduShortAsrRequest, BaiduShortAsrResponse,
    BaiduStartFrame,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Baidu AI Cloud Speech (百度语音)";

/// WebSocket connection timeout.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP request timeout for OAuth and REST API.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Channel buffer size for audio frames.
const AUDIO_CHANNEL_BUFFER: usize = 64;

/// Channel buffer size for results.
const RESULT_CHANNEL_BUFFER: usize = 256;

/// Channel buffer size for errors.
const ERROR_CHANNEL_BUFFER: usize = 64;

/// Default audio chunk duration in milliseconds.
const DEFAULT_CHUNK_DURATION_MS: u32 = 160;

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
// Token Manager
// =============================================================================

/// OAuth token manager with automatic refresh.
#[derive(Debug)]
struct TokenManager {
    /// Cached access token.
    access_token: RwLock<Option<String>>,

    /// Token expiration timestamp (Unix seconds).
    expires_at: RwLock<Option<u64>>,

    /// HTTP client for token requests.
    client: reqwest::Client,
}

impl TokenManager {
    /// Create a new token manager.
    fn new() -> Self {
        Self {
            access_token: RwLock::new(None),
            expires_at: RwLock::new(None),
            client: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    /// Get current timestamp in Unix seconds.
    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Check if the token is valid (with 1-hour buffer).
    async fn is_token_valid(&self) -> bool {
        let expires_at = *self.expires_at.read().await;
        let token = self.access_token.read().await;

        match (token.as_ref(), expires_at) {
            (Some(_), Some(exp)) => Self::now_secs() < exp.saturating_sub(3600),
            _ => false,
        }
    }

    /// Get or refresh the access token.
    async fn get_token(&self, api_key: &str, secret_key: &str) -> Result<String, STTError> {
        // Check if we have a valid cached token
        if self.is_token_valid().await
            && let Some(token) = self.access_token.read().await.clone() {
                return Ok(token);
            }

        // Fetch new token
        let url = format!(
            "{}?grant_type=client_credentials&client_id={}&client_secret={}",
            BAIDU_OAUTH_URL, api_key, secret_key
        );

        debug!("Fetching new Baidu OAuth token...");

        let response =
            self.client.post(&url).send().await.map_err(|e| {
                STTError::AuthenticationFailed(format!("OAuth request failed: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(STTError::AuthenticationFailed(format!(
                "OAuth failed with status {}: {}",
                status, body
            )));
        }

        let oauth_response: BaiduOAuthResponse = response.json().await.map_err(|e| {
            STTError::AuthenticationFailed(format!("Failed to parse OAuth response: {}", e))
        })?;

        // Cache the token
        let expires_at = Self::now_secs() + oauth_response.expires_in;
        *self.access_token.write().await = Some(oauth_response.access_token.clone());
        *self.expires_at.write().await = Some(expires_at);

        info!(
            "Baidu OAuth token obtained, expires in {} seconds",
            oauth_response.expires_in
        );

        Ok(oauth_response.access_token)
    }

    /// Clear the cached token.
    #[allow(dead_code)]
    async fn clear(&self) {
        *self.access_token.write().await = None;
        *self.expires_at.write().await = None;
    }
}

// =============================================================================
// Baidu STT Client
// =============================================================================

/// Baidu AI Cloud Speech-to-Text client.
///
/// Supports both WebSocket real-time streaming and REST API for short audio.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::baidu::BaiduStt;
///
/// let config = STTConfig {
///     // API key format: api_key|secret_key
///     api_key: "your_api_key|your_secret_key".to_string(),
///     language: "zh".to_string(),
///     sample_rate: 16000,
///     encoding: "pcm".to_string(),
///     model: "mandarin".to_string(),
///     ..Default::default()
/// };
///
/// let mut stt = BaiduStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct BaiduStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// Baidu-specific configuration.
    config: BaiduSttConfig,

    /// OAuth token manager.
    token_manager: Arc<TokenManager>,

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

    /// Session ID for WebSocket connection.
    session_id: String,

    /// HTTP client for REST API.
    http_client: reqwest::Client,

    /// Audio buffer for REST API mode (accumulates audio for batch processing).
    audio_buffer: Arc<Mutex<Vec<u8>>>,
}

impl BaiduStt {
    /// Create a new Baidu STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let baidu_config = BaiduSttConfig::from_base(config.clone())?;
        Self::from_baidu_config(config, baidu_config)
    }

    /// W1 keystone — construct directly from the standardized config so the knobs Baidu can honor
    /// (its custom-vocabulary model id `lm_id`, read from `provider_extras`) are carried END-TO-END.
    /// The flat `BaseSTT::new` path uses `from_base` and drops the extras; this is the reachable
    /// standardized path. Baidu has no boolean feature fields, so those features stay at default.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let baidu_config = BaiduSttConfig::from_standard(std)?;
        Self::from_baidu_config(std.base.clone(), baidu_config)
    }

    /// Internal: assemble the client from an already-mapped Baidu config (shared by `new` and
    /// `new_standard`).
    fn from_baidu_config(
        base_config: STTConfig,
        baidu_config: BaiduSttConfig,
    ) -> Result<Self, STTError> {
        baidu_config.validate()?;

        Ok(Self {
            base_config,
            config: baidu_config,
            token_manager: Arc::new(TokenManager::new()),
            connected: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            session_id: uuid::Uuid::new_v4().to_string(),
            http_client: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_default(),
            audio_buffer: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Connect using WebSocket real-time mode.
    async fn connect_realtime(&mut self) -> Result<(), STTError> {
        let url = self.config.get_realtime_url(&self.session_id);

        info!("Connecting to Baidu real-time ASR: {}", url);

        // Connect with timeout
        let (ws_stream, _) = match timeout(WS_CONNECT_TIMEOUT, connect_async(&url)).await {
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

        info!("Connected to Baidu real-time ASR");

        // Split stream
        let (mut write, mut read) = ws_stream.split();

        // Create channels
        let (audio_tx, mut audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Send START frame
        let start_frame = BaiduStartFrame::new(
            &self.config.api_key,
            &self.config.secret_key,
            self.config.model.dev_pid(),
            &self.config.cuid,
            self.config.sample_rate.value(),
            self.config.audio_format.as_str(),
        );

        let start_json = start_frame.to_json().map_err(|e| {
            STTError::ConnectionFailed(format!("Failed to serialize START frame: {}", e))
        })?;

        write
            .send(Message::Text(start_json.into()))
            .await
            .map_err(|e| {
                STTError::ConnectionFailed(format!("Failed to send START frame: {}", e))
            })?;

        debug!("Sent Baidu START frame");

        // Set connected state
        self.connected.store(true, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        // Clone for tasks
        let connected = self.connected.clone();
        let state_notify = self.state_notify.clone();
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

                // Send FINISH frame
                let finish_frame = BaiduFinishFrame::new();
                if let Ok(finish_json) = finish_frame.to_json() {
                    let _ = write.send(Message::Text(finish_json.into())).await;
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
                            );
                        }
                        Ok(Message::Close(_)) => {
                            debug!("Baidu WebSocket closed");
                            break;
                        }
                        Ok(Message::Ping(_)) => {
                            debug!("Received ping from Baidu");
                        }
                        Err(e) => {
                            error!("Baidu WebSocket error: {}", e);
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
                    debug!("Baidu shutdown signal received");
                }
                _ = recv_task => {
                    debug!("Baidu receive task completed");
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
    ) {
        match BaiduRealtimeResponse::from_json(text) {
            Ok(response) => {
                if response.is_error() {
                    if let Some(err_msg) = response.get_error() {
                        warn!("Baidu ASR error: {}", err_msg);
                        let _ = error_tx.try_send(STTError::AudioProcessingError(err_msg));
                    }
                } else if let Some(transcript) = response.get_transcript() {
                    let result = STTResult::new(
                        transcript.to_string(),
                        response.is_final(),
                        response.is_final(),
                        1.0, // Baidu doesn't provide confidence scores
                    );

                    debug!(
                        "Baidu transcript ({}): {}",
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
                warn!("Failed to parse Baidu response: {} - raw: {}", e, text);
            }
        }
    }

    /// Recognize short audio using REST API.
    pub async fn recognize_short_audio(&self, audio_data: &[u8]) -> Result<String, STTError> {
        // Get access token
        let token = self
            .token_manager
            .get_token(&self.config.api_key, &self.config.secret_key)
            .await?;

        // Build request
        let request = BaiduShortAsrRequest::new(
            self.config.audio_format.as_str(),
            self.config.sample_rate.value(),
            &self.config.cuid,
            &token,
            self.config.model.dev_pid(),
            audio_data,
        );

        // Add custom vocabulary if configured
        let request = if let Some(lm_id) = self.config.lm_id {
            request.with_lm_id(lm_id)
        } else {
            request
        };

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
            .post(url)
            .header("Content-Type", "application/json")
            .body(request_json)
            .send()
            .await
            .map_err(|e| STTError::NetworkError(format!("REST API request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(STTError::ProviderError(format!(
                "Baidu API error {}: {}",
                status, body
            )));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to read response: {}", e)))?;

        let asr_response = BaiduShortAsrResponse::from_json(&response_text)
            .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

        if !asr_response.is_success() {
            return Err(STTError::ProviderError(format!(
                "Baidu ASR error {}: {}",
                asr_response.err_no, asr_response.err_msg
            )));
        }

        asr_response
            .get_transcript()
            .map(|s| s.to_string())
            .ok_or_else(|| STTError::ProviderError("No transcript in response".to_string()))
    }

    /// Get the recommended chunk size for audio streaming.
    pub fn get_chunk_size(&self) -> usize {
        self.config
            .sample_rate
            .chunk_size_for_duration(DEFAULT_CHUNK_DURATION_MS)
    }

    /// Check if using real-time mode.
    pub fn is_realtime_mode(&self) -> bool {
        self.config.use_realtime
    }
}

#[async_trait::async_trait]
impl BaseSTT for BaiduStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        BaiduStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        if self.config.use_realtime {
            self.connect_realtime().await
        } else {
            // REST API mode: just mark as ready (no persistent connection)
            self.connected.store(true, Ordering::SeqCst);
            self.state_notify.notify_waiters();
            info!("Baidu STT ready in REST API mode");
            Ok(())
        }
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from Baidu STT...");

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

        // Generate new session ID for next connection
        self.session_id = uuid::Uuid::new_v4().to_string();

        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!("Disconnected from Baidu STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        if self.config.use_realtime {
            // Real-time mode: send to WebSocket
            if let Some(sender) = &self.ws_sender {
                sender.send(audio).await.map_err(|_| {
                    STTError::ProviderError("Failed to send audio to channel".to_string())
                })?;
            }
        } else {
            // REST API mode: accumulate in buffer
            self.audio_buffer.lock().await.extend_from_slice(&audio);
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
        let baidu_config = BaiduSttConfig::from_base(config.clone())?;
        baidu_config.validate()?;

        self.config = baidu_config;
        self.base_config = config;

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        PROVIDER_INFO
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> STTConfig {
        STTConfig {
            api_key: "test_api_key|test_secret_key".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "mandarin".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_client() {
        let config = create_test_config();
        let result = BaiduStt::new(config);
        assert!(result.is_ok());
    }

    // W1 keystone: Baidu's one honorable knob (custom-vocabulary `lm_id` from provider_extras)
    // survives through new_standard onto the stored provider config; the flat factory path drops it.
    #[test]
    fn new_standard_carries_lm_id_to_config() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("lm_id".into(), serde_json::json!(98765));
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "baidu".into(),
                api_key: "my_api_key|my_secret_key".into(),
                model: "mandarin".into(),
                ..create_test_config()
            },
            features: Default::default(),
            extras: ProviderExtras(extras),
        };
        let stt = BaiduStt::new_standard(&std).unwrap();
        assert_eq!(stt.config.lm_id, Some(98765));
    }

    #[test]
    fn test_new_client_invalid_api_key_format() {
        let config = STTConfig {
            api_key: "api_key_without_secret".to_string(),
            ..Default::default()
        };
        let result = BaiduStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_client_empty_api_key() {
        let config = STTConfig {
            api_key: "|secret_key".to_string(),
            ..Default::default()
        };
        let result = BaiduStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let stt = BaiduStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = BaiduStt::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Baidu") || info.contains("百度"));
    }

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let stt = BaiduStt::new(config.clone()).unwrap();
        assert!(stt.get_config().is_some());
        assert!(stt.get_config().unwrap().api_key.contains("test_api_key"));
    }

    #[test]
    fn test_chunk_size() {
        let config = create_test_config();
        let stt = BaiduStt::new(config).unwrap();

        // 160ms at 16kHz, 16-bit: 16000 * 2 * 160 / 1000 = 5120 bytes
        assert_eq!(stt.get_chunk_size(), 5120);
    }

    #[test]
    fn test_is_realtime_mode() {
        let config = create_test_config();
        let stt = BaiduStt::new(config).unwrap();
        assert!(stt.is_realtime_mode()); // Default is real-time
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = BaiduStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0u8; 1024])).await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = BaiduStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = create_test_config();
        let mut stt = BaiduStt::new(config).unwrap();

        let result_cb: STTResultCallback = Arc::new(|_| Box::pin(async {}));
        let error_cb: STTErrorCallback = Arc::new(|_| Box::pin(async {}));

        assert!(stt.on_result(result_cb).await.is_ok());
        assert!(stt.on_error(error_cb).await.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = create_test_config();
        let mut stt = BaiduStt::new(config).unwrap();

        let new_config = STTConfig {
            api_key: "new_api_key|new_secret_key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "english".to_string(),
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());
        assert!(stt.get_config().unwrap().api_key.contains("new_api_key"));
    }

    #[test]
    fn test_handle_realtime_response_final() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "err_no": 0,
            "err_msg": "success",
            "type": "FIN_TEXT",
            "result": "你好世界",
            "sn": "test-session"
        }"#;

        BaiduStt::handle_realtime_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好世界");
        assert!(result.is_final);
    }

    #[test]
    fn test_handle_realtime_response_interim() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "err_no": 0,
            "err_msg": "success",
            "type": "MID_TEXT",
            "result": "你好"
        }"#;

        BaiduStt::handle_realtime_response(json, &result_tx, &error_tx);

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

        let json = r#"{
            "err_no": 3301,
            "err_msg": "Authentication error",
            "type": "ERROR"
        }"#;

        BaiduStt::handle_realtime_response(json, &result_tx, &error_tx);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_session_id_generation() {
        let config = create_test_config();
        let stt = BaiduStt::new(config).unwrap();

        // Session ID should be a valid UUID
        assert!(!stt.session_id.is_empty());
        assert!(uuid::Uuid::parse_str(&stt.session_id).is_ok());
    }

    #[tokio::test]
    async fn test_rest_mode_connect() {
        let mut config = create_test_config();
        config.model = "mandarin".to_string();

        let mut stt = BaiduStt::new(config).unwrap();

        // Modify config to use REST mode
        stt.config.use_realtime = false;

        let result = stt.connect().await;
        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_rest_mode_audio_buffering() {
        let mut config = create_test_config();
        config.model = "mandarin".to_string();

        let mut stt = BaiduStt::new(config).unwrap();
        stt.config.use_realtime = false;

        // Connect in REST mode
        stt.connect().await.unwrap();

        // Send some audio
        stt.send_audio(Bytes::from_static(&[1, 2, 3, 4]))
            .await
            .unwrap();
        stt.send_audio(Bytes::from_static(&[5, 6, 7, 8]))
            .await
            .unwrap();

        // Check buffer
        let buffer = stt.audio_buffer.lock().await;
        assert_eq!(buffer.len(), 8);
        assert_eq!(&buffer[..], &[1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn test_token_manager_creation() {
        let manager = TokenManager::new();
        // Should start with no token
        let rt = tokio::runtime::Runtime::new().unwrap();
        let is_valid = rt.block_on(manager.is_token_valid());
        assert!(!is_valid);
    }

    #[test]
    fn test_model_dev_pid_in_start_frame() {
        let config = create_test_config();
        let stt = BaiduStt::new(config).unwrap();

        let start_frame = BaiduStartFrame::new(
            &stt.config.api_key,
            &stt.config.secret_key,
            stt.config.model.dev_pid(),
            &stt.config.cuid,
            stt.config.sample_rate.value(),
            stt.config.audio_format.as_str(),
        );

        let json = start_frame.to_json().unwrap();
        assert!(json.contains("\"type\":\"START\""));
        assert!(json.contains("\"dev_pid\":1537"));
    }
}
