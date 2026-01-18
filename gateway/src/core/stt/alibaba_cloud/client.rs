//! Alibaba Cloud DashScope STT WebSocket Client
//!
//! This module implements the `BaseSTT` trait for Alibaba Cloud's DashScope
//! Speech-to-Text WebSocket API.
//!
//! # Architecture
//!
//! The client supports two message formats:
//!
//! 1. **Qwen Format**: For Qwen3-ASR models using OpenAI-like realtime protocol
//! 2. **Inference Format**: For Paraformer models using DashScope inference protocol
//!
//! # WebSocket Message Flow (Qwen Format)
//!
//! ```text
//! Client                              Server
//!   |                                    |
//!   |------ Connect with Bearer -------->|
//!   |<----- HTTP 101 Upgrade ------------|
//!   |                                    |
//!   |------ session.update ------------->|
//!   |<----- session.created -------------|
//!   |                                    |
//!   |------ audio buffer append -------->|
//!   |<----- transcription results -------|
//!   |                                    |
//!   |------ session.finish ------------->|
//!   |<----- session.finished ------------|
//! ```

use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::{Mutex, Notify, mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        http::Request,
        protocol::Message,
    },
};
use tracing::{debug, error, info, warn};

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};
use super::config::{DashScopeSttConfig, TurnDetectionMode};
use super::messages::{
    QwenSessionUpdate, QwenAudioBufferAppend, QwenSessionFinish, QwenServerMessage,
    ParaformerRunTask, ParaformerFinishTask, ParaformerResponse,
};

// =============================================================================
// Constants
// =============================================================================

/// Provider information string.
const PROVIDER_INFO: &str = "Alibaba Cloud DashScope STT (阿里云)";

/// WebSocket connection timeout.
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// WebSocket message timeout (idle detection).
#[allow(dead_code)]
const WS_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);

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
// DashScope STT Client
// =============================================================================

/// Alibaba Cloud DashScope Speech-to-Text WebSocket client.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::{BaseSTT, STTConfig};
/// use waav_gateway::core::stt::alibaba_cloud::DashScopeStt;
///
/// let config = STTConfig {
///     api_key: "sk-xxxxxxxx".to_string(),
///     language: "zh".to_string(),
///     sample_rate: 16000,
///     ..Default::default()
/// };
///
/// let mut stt = DashScopeStt::new(config)?;
/// stt.connect().await?;
/// stt.send_audio(audio_data).await?;
/// stt.disconnect().await?;
/// ```
pub struct DashScopeStt {
    /// Base configuration for BaseSTT trait.
    base_config: STTConfig,

    /// DashScope-specific configuration.
    config: DashScopeSttConfig,

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

    /// Task ID for Paraformer format.
    task_id: Arc<Mutex<Option<String>>>,
}

impl DashScopeStt {
    /// Create a new DashScope STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let dashscope_config = DashScopeSttConfig::from_base(config.clone())?;
        dashscope_config.validate()?;

        Ok(Self {
            base_config: config,
            config: dashscope_config,
            connected: Arc::new(AtomicBool::new(false)),
            state_notify: Arc::new(Notify::new()),
            ws_sender: None,
            shutdown_tx: None,
            connection_handle: None,
            result_forward_handle: None,
            error_forward_handle: None,
            result_callback: Arc::new(Mutex::new(None)),
            error_callback: Arc::new(Mutex::new(None)),
            task_id: Arc::new(Mutex::new(None)),
        })
    }

    /// Build WebSocket request with authentication headers.
    fn build_request(&self) -> Result<Request<()>, STTError> {
        let url = self.config.get_websocket_url();

        let mut request = Request::builder()
            .uri(&url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("User-Agent", "WaaV-Gateway/1.0");

        // Add OpenAI-Beta header for Qwen models
        if self.config.model.is_qwen_model() {
            request = request.header("OpenAI-Beta", "realtime=v1");
        }

        request
            .body(())
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to build request: {}", e)))
    }

    /// Create session update message for Qwen format.
    fn create_qwen_session_update(&self) -> String {
        let turn_detection_type = match self.config.turn_detection {
            TurnDetectionMode::ServerVad => "server_vad",
            TurnDetectionMode::Manual => "manual",
            TurnDetectionMode::None => "none",
        };

        let msg = QwenSessionUpdate::new(
            self.config.language.as_code(),
            self.config.sample_rate,
            self.config.audio_format.as_format_str(),
            self.config.silence_duration_ms,
            turn_detection_type,
        );

        msg.to_json().unwrap_or_default()
    }

    /// Create run-task message for Paraformer format.
    fn create_paraformer_run_task(&self) -> (String, String) {
        let msg = ParaformerRunTask::new(
            self.config.model.as_model_id(),
            self.config.audio_format.as_format_str(),
            self.config.sample_rate,
            self.config.language.as_code(),
            self.config.disfluency_removal,
            self.config.punctuation,
        );

        let task_id = msg.task_id().to_string();
        let json = msg.to_json().unwrap_or_default();
        (json, task_id)
    }

    /// Handle Qwen format response.
    fn handle_qwen_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) {
        match QwenServerMessage::from_json(text) {
            Ok(msg) => {
                if msg.is_error() {
                    if let Some(err) = &msg.error {
                        let _ = error_tx.try_send(STTError::AudioProcessingError(
                            err.message.clone(),
                        ));
                    }
                } else if msg.is_transcription_completed() {
                    if let Some(transcript) = msg.get_transcript() {
                        let result = STTResult::new(transcript.to_string(), true, true, 1.0);
                        let _ = result_tx.try_send(result);
                    }
                } else if msg.is_session_created() || msg.is_session_updated() {
                    debug!("DashScope session event: {}", msg.msg_type);
                } else if msg.is_session_finished() {
                    debug!("DashScope session finished");
                }
            }
            Err(e) => {
                warn!("Failed to parse Qwen response: {}", e);
            }
        }
    }

    /// Handle Paraformer format response.
    fn handle_paraformer_response(
        text: &str,
        result_tx: &mpsc::Sender<STTResult>,
        error_tx: &mpsc::Sender<STTError>,
    ) {
        match ParaformerResponse::from_json(text) {
            Ok(msg) => {
                if msg.is_task_failed() {
                    if let Some((code, message)) = msg.get_error() {
                        let _ = error_tx.try_send(STTError::AudioProcessingError(
                            format!("[{}] {}", code, message),
                        ));
                    }
                } else if msg.is_result_generated() {
                    if let Some(transcript) = msg.get_transcript() {
                        let result = STTResult::new(
                            transcript.to_string(),
                            msg.is_final(),
                            msg.is_final(),
                            1.0,
                        );
                        let _ = result_tx.try_send(result);
                    }
                } else if msg.is_task_started() {
                    debug!("DashScope Paraformer task started");
                } else if msg.is_task_finished() {
                    debug!("DashScope Paraformer task finished");
                }
            }
            Err(e) => {
                warn!("Failed to parse Paraformer response: {}", e);
            }
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for DashScopeStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        DashScopeStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Connecting to Alibaba Cloud DashScope STT...");

        // Build request
        let request = self.build_request()?;
        let url = self.config.get_websocket_url();

        // Connect with timeout
        let (ws_stream, _) = match timeout(WS_CONNECT_TIMEOUT, connect_async(request)).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                return Err(STTError::ConnectionFailed(format!(
                    "WebSocket connection failed: {}",
                    e
                )));
            }
            Err(_) => {
                return Err(STTError::ConnectionFailed(
                    "Connection timeout".to_string(),
                ));
            }
        };

        info!("Connected to DashScope: {}", url);

        // Split stream
        let (mut write, mut read) = ws_stream.split();

        // Create channels
        let (audio_tx, mut audio_rx) = mpsc::channel::<Bytes>(AUDIO_CHANNEL_BUFFER);
        let (result_tx, mut result_rx) = mpsc::channel::<STTResult>(RESULT_CHANNEL_BUFFER);
        let (error_tx, mut error_rx) = mpsc::channel::<STTError>(ERROR_CHANNEL_BUFFER);
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

        self.ws_sender = Some(audio_tx);
        self.shutdown_tx = Some(shutdown_tx);

        // Send initial message based on model type
        let is_qwen = self.config.model.is_qwen_model();
        let task_id = if is_qwen {
            let session_update = self.create_qwen_session_update();
            write
                .send(Message::Text(session_update.into()))
                .await
                .map_err(|e| STTError::ConnectionFailed(format!("Failed to send session update: {}", e)))?;
            None
        } else {
            let (run_task_json, task_id) = self.create_paraformer_run_task();
            write
                .send(Message::Text(run_task_json.into()))
                .await
                .map_err(|e| STTError::ConnectionFailed(format!("Failed to send run-task: {}", e)))?;
            Some(task_id)
        };

        // Store task ID for Paraformer
        if let Some(tid) = &task_id {
            *self.task_id.lock().await = Some(tid.clone());
        }

        // Set connected state
        self.connected.store(true, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        // Clone for tasks
        let connected = self.connected.clone();
        let state_notify = self.state_notify.clone();
        let result_tx_clone = result_tx.clone();
        let error_tx_clone = error_tx.clone();
        let task_id_clone = task_id.clone();

        // Spawn connection handler task
        let connection_handle = tokio::spawn(async move {
            let send_task = tokio::spawn(async move {
                while let Some(audio) = audio_rx.recv().await {
                    let msg = if is_qwen {
                        let audio_msg = QwenAudioBufferAppend::from_bytes(&audio);
                        Message::Text(audio_msg.to_json().unwrap_or_default().into())
                    } else {
                        // Paraformer expects binary audio
                        Message::Binary(audio.to_vec().into())
                    };

                    if write.send(msg).await.is_err() {
                        break;
                    }
                }

                // Send finish message
                let finish_msg = if is_qwen {
                    Message::Text(QwenSessionFinish::new().to_json().unwrap_or_default().into())
                } else if let Some(tid) = &task_id_clone {
                    Message::Text(ParaformerFinishTask::new(tid).to_json().unwrap_or_default().into())
                } else {
                    return write;
                };

                let _ = write.send(finish_msg).await;
                write
            });

            let recv_task = tokio::spawn(async move {
                while let Some(msg_result) = read.next().await {
                    match msg_result {
                        Ok(Message::Text(text)) => {
                            if is_qwen {
                                Self::handle_qwen_response(&text, &result_tx_clone, &error_tx_clone);
                            } else {
                                Self::handle_paraformer_response(&text, &result_tx_clone, &error_tx_clone);
                            }
                        }
                        Ok(Message::Close(_)) => {
                            debug!("DashScope WebSocket closed");
                            break;
                        }
                        Ok(Message::Ping(data)) => {
                            debug!("Received ping");
                            // Pong is handled automatically by tungstenite
                            let _ = data;
                        }
                        Err(e) => {
                            error!("WebSocket error: {}", e);
                            let _ = error_tx_clone.try_send(STTError::ConnectionFailed(e.to_string()));
                            break;
                        }
                        _ => {}
                    }
                }
            });

            // Wait for shutdown or completion
            tokio::select! {
                _ = &mut shutdown_rx => {
                    debug!("Shutdown signal received");
                }
                _ = recv_task => {
                    debug!("Receive task completed");
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
        if !self.connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        info!("Disconnecting from DashScope STT...");

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

        // Clear task ID
        *self.task_id.lock().await = None;

        self.connected.store(false, Ordering::SeqCst);
        self.state_notify.notify_waiters();

        info!("Disconnected from DashScope STT");
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
            sender
                .send(audio)
                .await
                .map_err(|_| STTError::ProviderError("Failed to send audio to channel".to_string()))?;
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
        let dashscope_config = DashScopeSttConfig::from_base(config.clone())?;
        self.config = dashscope_config;
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
            api_key: "test_api_key".to_string(),
            language: "zh".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            model: "qwen3-asr-flash-realtime".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_client() {
        let config = create_test_config();
        let result = DashScopeStt::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_new_client_empty_api_key() {
        let config = STTConfig {
            api_key: "".to_string(),
            ..Default::default()
        };
        let result = DashScopeStt::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_not_connected_initially() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_provider_info() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("DashScope") || info.contains("阿里云"));
    }

    #[test]
    fn test_config_access() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config.clone()).unwrap();
        assert_eq!(stt.get_config().unwrap().api_key, config.api_key);
    }

    #[tokio::test]
    async fn test_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

        let result = stt.send_audio(Bytes::from_static(&[0u8; 1024])).await;
        assert!(matches!(result, Err(STTError::ConnectionFailed(_))));
    }

    #[tokio::test]
    async fn test_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = create_test_config();
        let mut stt = DashScopeStt::new(config).unwrap();

        let result_cb: STTResultCallback = Arc::new(|_| Box::pin(async {}));
        let error_cb: STTErrorCallback = Arc::new(|_| Box::pin(async {}));

        assert!(stt.on_result(result_cb).await.is_ok());
        assert!(stt.on_error(error_cb).await.is_ok());
    }

    #[test]
    fn test_qwen_session_update_creation() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();

        let session_update = stt.create_qwen_session_update();
        assert!(session_update.contains("session.update"));
        assert!(session_update.contains("zh"));
    }

    #[test]
    fn test_paraformer_run_task_creation() {
        let mut config = create_test_config();
        config.model = "paraformer-realtime-v2".to_string();

        let stt = DashScopeStt::new(config).unwrap();
        let (json, task_id) = stt.create_paraformer_run_task();

        assert!(json.contains("run-task"));
        assert!(json.contains("paraformer-realtime-v2"));
        assert!(!task_id.is_empty());
    }

    #[test]
    fn test_websocket_url_qwen() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();

        let url = stt.config.get_websocket_url();
        assert!(url.contains("realtime"));
        assert!(url.contains("qwen3-asr-flash-realtime"));
    }

    #[test]
    fn test_websocket_url_paraformer() {
        let mut config = create_test_config();
        config.model = "paraformer-realtime-v2".to_string();

        let stt = DashScopeStt::new(config).unwrap();
        let url = stt.config.get_websocket_url();
        assert!(url.contains("inference"));
    }

    #[test]
    fn test_build_request_qwen() {
        let config = create_test_config();
        let stt = DashScopeStt::new(config).unwrap();

        let request = stt.build_request();
        assert!(request.is_ok());
    }

    #[test]
    fn test_handle_qwen_response_transcription() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "type": "conversation.item.input_audio_transcription.completed",
            "transcript": "你好世界"
        }"#;

        DashScopeStt::handle_qwen_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        assert_eq!(result.unwrap().transcript, "你好世界");
    }

    #[test]
    fn test_handle_qwen_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        let json = r#"{
            "type": "error",
            "error": {
                "type": "invalid_request",
                "code": "400",
                "message": "Invalid audio format"
            }
        }"#;

        DashScopeStt::handle_qwen_response(json, &result_tx, &error_tx);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }

    #[test]
    fn test_handle_paraformer_response_result() {
        let (result_tx, mut result_rx) = mpsc::channel(10);
        let (error_tx, _error_rx) = mpsc::channel(10);

        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "result-generated"
            },
            "payload": {
                "output": {
                    "sentence": {
                        "begin_time": 0,
                        "end_time": 1500,
                        "text": "你好世界",
                        "words": [],
                        "sentence_end": true
                    }
                }
            }
        }"#;

        DashScopeStt::handle_paraformer_response(json, &result_tx, &error_tx);

        let result = result_rx.try_recv();
        assert!(result.is_ok());
        let result = result.unwrap();
        assert_eq!(result.transcript, "你好世界");
        assert!(result.is_final);
    }

    #[test]
    fn test_handle_paraformer_response_error() {
        let (result_tx, _result_rx) = mpsc::channel(10);
        let (error_tx, mut error_rx) = mpsc::channel(10);

        let json = r#"{
            "header": {
                "task_id": "test-task-id",
                "event": "task-failed",
                "error_code": "401",
                "error_message": "Unauthorized"
            }
        }"#;

        DashScopeStt::handle_paraformer_response(json, &result_tx, &error_tx);

        let error = error_rx.try_recv();
        assert!(error.is_ok());
    }
}
