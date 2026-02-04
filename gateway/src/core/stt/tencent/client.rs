//! Tencent Cloud Speech Recognition WebSocket Client
//!
//! This module implements the `BaseSTT` trait for Tencent Cloud's
//! real-time Speech-to-Text service.
//!
//! # Architecture
//!
//! The client uses WebSocket streaming with HMAC-SHA1 signature authentication.
//! Audio is sent in 40ms chunks (1280 bytes at 16kHz).
//!
//! # WebSocket Message Flow
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with signature ----->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ Binary audio (40ms) -------->|
//!   |<----- JSON (slice_type=0) ---------|  (interim)
//!   |                                    |
//!   |------ Binary audio (40ms) -------->|
//!   |<----- JSON (slice_type=1) ---------|  (segment end)
//!   |                                    |
//!   |------ Close or timeout ----------->|
//!   |<----- JSON (final=1) --------------|  (complete)
//! ```
//!
//! # Authentication
//!
//! Signature parameters are embedded in the WebSocket URL:
//! `wss://asr.cloud.tencent.com/asr/v2/{app_id}?secretid=...&signature=...`

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::config::{TENCENT_ASR_WS_URL, TencentSttConfig};
use super::messages::TencentAsrResponse;
use super::signature::TencentSignatureBuilder;
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Tencent Cloud Speech (腾讯云语音)";

/// WebSocket connection timeout.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

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
// Tencent STT Client
// =============================================================================

/// Tencent Cloud Speech-to-Text client.
///
/// Provides real-time speech recognition using WebSocket streaming.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::tencent::TencentStt;
///
/// let config = STTConfig {
///     // API key format: secret_id|secret_key|app_id
///     api_key: "your_secret_id|your_secret_key|your_app_id".to_string(),
///     language: "zh".to_string(),
///     sample_rate: 16000,
///     encoding: "pcm".to_string(),
///     model: "16k_zh".to_string(),
///     ..Default::default()
/// };
///
/// let mut stt = TencentStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct TencentStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// Tencent-specific configuration.
    config: TencentSttConfig,

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

    /// Current voice ID for the session.
    voice_id: String,
}

impl TencentStt {
    /// Create a new Tencent STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let tencent_config = TencentSttConfig::from_base(config.clone())?;
        tencent_config.validate()?;

        let voice_id = TencentSignatureBuilder::generate_voice_id();

        Ok(Self {
            base_config: config,
            config: tencent_config,
            connected: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            voice_id,
        })
    }

    /// Build the WebSocket URL with signature.
    fn build_ws_url(&self) -> Result<String, STTError> {
        let mut signature_builder =
            TencentSignatureBuilder::new(&self.config.secret_id, &self.config.secret_key)
                .with_engine_model(self.config.engine_model_type.as_str())
                .with_voice_format(self.config.voice_format.value())
                .with_voice_id(&self.voice_id)
                .with_needvad(self.config.needvad)
                .with_filter_dirty(self.config.filter_dirty.value() as u32)
                .with_filter_modal(self.config.filter_modal.value() as u32)
                .with_filter_punc(self.config.filter_punc)
                .with_word_info(self.config.word_info.value() as u32)
                .with_convert_num_mode(self.config.convert_num_mode.value() as u32);

        // Add reinforce_hotword if enabled
        if self.config.reinforce_hotword {
            signature_builder = signature_builder.with_reinforce_hotword(true);
        }

        // Add filter_empty_result if enabled
        if self.config.filter_empty_result {
            signature_builder = signature_builder.with_filter_empty_result(true);
        }

        // Add optional VAD silence time
        if let Some(vad_time) = self.config.vad_silence_time {
            signature_builder = signature_builder.with_vad_silence_time(vad_time);
        }

        // Add optional max speak time
        if let Some(max_speak) = self.config.max_speak_time {
            signature_builder = signature_builder.with_max_speak_time(max_speak);
        }

        // Add optional hotword ID
        if let Some(ref hotword_id) = self.config.hotword_id {
            signature_builder = signature_builder.with_hotword_id(hotword_id);
        }

        // Add optional hotword list
        if let Some(ref hotword_list) = self.config.hotword_list {
            signature_builder = signature_builder.with_hotword_list(hotword_list);
        }

        // Add optional customization ID
        if let Some(ref customization_id) = self.config.customization_id {
            signature_builder = signature_builder.with_customization_id(customization_id);
        }

        signature_builder
            .build_url(TENCENT_ASR_WS_URL, &self.config.app_id)
            .map_err(|e| STTError::ConfigurationError(format!("Failed to build signature: {}", e)))
    }

    /// Handle a WebSocket text message (JSON response).
    fn handle_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) {
        match TencentAsrResponse::from_json(text) {
            Ok(response) => {
                if response.is_error() {
                    if let Some(err_msg) = response.get_error_message() {
                        warn!("Tencent ASR error: {}", err_msg);
                        let _ = error_tx.try_send(STTError::ProviderError(err_msg));
                    }
                } else if let Some(transcript) = response.get_transcript() {
                    // Determine if this is a final result
                    let is_final = response.is_segment_final()
                        || response.is_session_final()
                        || response
                            .result
                            .as_ref()
                            .map(|r| r.is_recognition_final())
                            .unwrap_or(false);

                    let result = STTResult::new(
                        transcript.to_string(),
                        is_final,
                        response.is_session_final(),
                        1.0, // Tencent doesn't provide confidence scores
                    );

                    debug!(
                        "Tencent transcript ({}): {}",
                        if is_final { "final" } else { "interim" },
                        transcript
                    );

                    let _ = result_tx.try_send(result);
                }
            }
            Err(e) => {
                warn!("Failed to parse Tencent response: {} - raw: {}", e, text);
            }
        }
    }

    /// Get the recommended chunk size for audio streaming.
    pub fn get_chunk_size(&self) -> usize {
        self.config.get_chunk_size()
    }

    /// Get the current voice ID.
    pub fn voice_id(&self) -> &str {
        &self.voice_id
    }

    /// Generate a new voice ID for the next session.
    fn regenerate_voice_id(&mut self) {
        self.voice_id = TencentSignatureBuilder::generate_voice_id();
    }
}

#[async_trait::async_trait]
impl BaseSTT for TencentStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        TencentStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Build URL with signature
        let url = self.build_ws_url()?;

        info!(
            "Connecting to Tencent ASR: {}",
            url.split('?').next().unwrap_or(&url)
        );

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

        info!("Connected to Tencent ASR");

        // Split stream
        let (mut write, mut read) = ws_stream.split();

        // Create channels
        let (audio_tx, mut audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);

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
                    // Send binary audio data directly
                    if write
                        .send(Message::Binary(audio.to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                write
            });

            let recv_task = tokio::spawn(async move {
                while let Some(msg_result) = read.next().await {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            Self::handle_response(&text, &result_tx_clone, &error_tx_clone);
                        }
                        Ok(Message::Close(_)) => {
                            debug!("Tencent WebSocket closed");
                            break;
                        }
                        Ok(Message::Ping(data)) => {
                            debug!("Received ping from Tencent, len={}", data.len());
                        }
                        Err(e) => {
                            error!("Tencent WebSocket error: {}", e);
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
                    debug!("Tencent shutdown signal received");
                }
                _ = recv_task => {
                    debug!("Tencent receive task completed");
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

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Always regenerate voice ID for next session, even if not connected
        // This ensures fresh IDs after connection failures
        self.regenerate_voice_id();

        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from Tencent STT...");

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

        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!("Disconnected from Tencent STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        if let Some(sender) = &self.ws_sender {
            sender.send(audio).await.map_err(|_| {
                STTError::ProviderError("Failed to send audio to channel".to_string())
            })?;
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
        let tencent_config = TencentSttConfig::from_base(config.clone())?;
        tencent_config.validate()?;

        self.config = tencent_config;
        self.base_config = config;

        // Regenerate voice ID for new config
        self.regenerate_voice_id();

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
            api_key: "test_secret_id|test_secret_key|test_app_id".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "16k_zh".to_string(),
            ..Default::default()
        }
    }

    // =========================================================================
    // Client Creation Tests
    // =========================================================================

    #[test]
    fn test_new_client() {
        let config = create_test_config();
        let result = TencentStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_client_invalid_api_key_format() {
        let config = STTConfig {
            api_key: "only_one_part".to_string(),
            ..Default::default()
        };
        let result = TencentStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_client_missing_app_id() {
        let config = STTConfig {
            api_key: "id|key".to_string(), // Only 2 parts, needs 3
            ..Default::default()
        };
        let result = TencentStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_client_empty_credentials() {
        let config = STTConfig {
            api_key: "||".to_string(),
            ..Default::default()
        };
        let result = TencentStt::new(config);
        assert!(result.is_err());
    }

    // =========================================================================
    // State Tests
    // =========================================================================

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_voice_id_generated() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        assert!(!stt.voice_id().is_empty());
        assert_eq!(stt.voice_id().len(), 16);
    }

    #[test]
    fn test_voice_id_unique() {
        let config1 = create_test_config();
        let config2 = create_test_config();
        let stt1 = TencentStt::new(config1).unwrap();
        let stt2 = TencentStt::new(config2).unwrap();

        // Voice IDs should be unique (very unlikely to be equal)
        assert_ne!(stt1.voice_id(), stt2.voice_id());
    }

    // =========================================================================
    // Provider Info Tests
    // =========================================================================

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Tencent") || info.contains("腾讯"));
    }

    // =========================================================================
    // Config Tests
    // =========================================================================

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let stt = TencentStt::new(config.clone()).unwrap();

        assert!(stt.get_config().is_some());
        assert!(stt.get_config().unwrap().api_key.contains("test_secret_id"));
    }

    #[test]
    fn test_chunk_size() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();

        // 40ms at 16kHz, 16-bit: 16000 * 2 * 40 / 1000 = 1280 bytes
        assert_eq!(stt.get_chunk_size(), 1280);
    }

    #[test]
    fn test_chunk_size_8k() {
        let mut config = create_test_config();
        config.model = "8k_zh".to_string();
        let stt = TencentStt::new(config).unwrap();

        // 40ms at 8kHz, 16-bit: 8000 * 2 * 40 / 1000 = 640 bytes
        assert_eq!(stt.get_chunk_size(), 640);
    }

    // =========================================================================
    // URL Building Tests
    // =========================================================================

    #[test]
    fn test_build_ws_url() {
        let config = create_test_config();
        let stt = TencentStt::new(config).unwrap();
        let url = stt.build_ws_url().unwrap();

        assert!(url.starts_with("wss://asr.cloud.tencent.com/asr/v2/test_app_id"));
        assert!(url.contains("secretid=test_secret_id"));
        assert!(url.contains("engine_model_type=16k_zh"));
        assert!(url.contains("signature="));
    }

    #[test]
    fn test_build_ws_url_with_english_model() {
        let mut config = create_test_config();
        config.model = "16k_en".to_string();
        let stt = TencentStt::new(config).unwrap();
        let url = stt.build_ws_url().unwrap();

        assert!(url.contains("engine_model_type=16k_en"));
    }

    // =========================================================================
    // Async Operation Tests
    // =========================================================================

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0u8; 1280])).await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let result_cb: STTResultCallback = Arc::new(|_| Box::pin(async {}));
        let error_cb: STTErrorCallback = Arc::new(|_| Box::pin(async {}));

        assert!(stt.on_result(result_cb).await.is_ok());
        assert!(stt.on_error(error_cb).await.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let old_voice_id = stt.voice_id().to_string();

        let new_config = STTConfig {
            api_key: "new_id|new_key|new_app".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            encoding: "opus".to_string(),
            model: "16k_en".to_string(),
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());
        assert!(stt.get_config().unwrap().api_key.contains("new_id"));

        // Voice ID should be regenerated
        assert_ne!(stt.voice_id(), old_voice_id);
    }

    #[tokio::test]
    async fn test_update_config_invalid() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let invalid_config = STTConfig {
            api_key: "invalid".to_string(),
            ..Default::default()
        };

        let result = stt.update_config(invalid_config).await;
        assert!(result.is_err());
    }

    // =========================================================================
    // Response Handling Tests
    // =========================================================================

    #[test]
    fn test_handle_response_success() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 1,
                "index": 0,
                "start_time": 0,
                "end_time": 1500,
                "voice_text_str": "你好世界"
            },
            "final": 0
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好世界");
        assert!(result.is_final);
    }

    #[test]
    fn test_handle_response_interim() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 0,
                "index": 0,
                "start_time": 0,
                "end_time": 500,
                "voice_text_str": "你好"
            }
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好");
        assert!(!result.is_final); // slice_type=0 is interim
    }

    #[test]
    fn test_handle_response_session_final() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id",
            "result": {
                "slice_type": 2,
                "index": 5,
                "start_time": 0,
                "end_time": 10000,
                "voice_text_str": "完整识别结果"
            },
            "final": 1
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert!(result.is_final);
        assert!(result.is_speech_final);
    }

    #[test]
    fn test_handle_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 4002,
            "message": "Authentication failed",
            "voice_id": "test_voice_id"
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_handle_response_invalid_json() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        TencentStt::handle_response("not valid json", &result_tx, &error_tx);

        // Should not produce result or error (just log warning)
        assert!(result_rx.try_recv().is_err());
        assert!(error_rx.try_recv().is_err());
    }

    #[test]
    fn test_handle_response_empty_transcript() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "code": 0,
            "message": "success",
            "voice_id": "test_voice_id"
        }"#;

        TencentStt::handle_response(json, &result_tx, &error_tx);

        // No result because no transcript
        assert!(result_rx.try_recv().is_err());
    }

    // =========================================================================
    // Voice ID Regeneration Tests
    // =========================================================================

    #[tokio::test]
    async fn test_voice_id_regenerated_on_disconnect() {
        let config = create_test_config();
        let mut stt = TencentStt::new(config).unwrap();

        let old_voice_id = stt.voice_id().to_string();

        // Disconnect (even when not connected) should regenerate
        stt.disconnect().await.unwrap();

        assert_ne!(stt.voice_id(), old_voice_id);
    }
}
