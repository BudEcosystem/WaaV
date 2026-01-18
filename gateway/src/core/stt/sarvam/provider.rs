//! Sarvam.ai STT Provider Implementation
//!
//! WebSocket-based streaming STT provider for Sarvam.ai's Saarika model.
//! Specialized in Indian language speech recognition.
//!
//! # Authentication
//!
//! **IMPORTANT**: Sarvam uses a custom header `api-subscription-key` for authentication,
//! NOT the standard `Authorization: Bearer` header.
//!
//! # Protocol
//!
//! - Audio is sent as base64-encoded chunks in JSON messages
//! - Server responds with transcript, speech_start, and speech_end events
//! - Connection requires periodic ping (< 60 seconds) to stay alive

use bytes::Bytes;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio::time::{Instant, interval, timeout};
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::{debug, error, info, warn};

use super::config::{
    CONNECTION_TIMEOUT_SECS, KEEPALIVE_INTERVAL_SECS, MESSAGE_TIMEOUT_SECS, SarvamSTTConfig,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

/// Type alias for the complex callback function type
type AsyncSTTCallback = Box<
    dyn Fn(STTResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Type alias for the error callback function type
type AsyncErrorCallback = Box<
    dyn Fn(STTError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// Simplified connection state with atomic updates
#[derive(Debug, Clone)]
enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    #[allow(dead_code)]
    Error(String),
}

/// Sarvam audio message format
#[derive(Debug, Serialize)]
struct SarvamAudioMessage {
    /// Base64-encoded audio data
    audio: String,
}

/// Sarvam ping message format
#[derive(Debug, Serialize)]
struct SarvamPingMessage {
    #[serde(rename = "type")]
    msg_type: &'static str,
}

/// Sarvam response message types
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SarvamResponse {
    /// Transcript result
    #[serde(rename = "transcript")]
    Transcript(SarvamTranscript),
    /// Speech started event
    #[serde(rename = "speech_start")]
    SpeechStart,
    /// Speech ended event
    #[serde(rename = "speech_end")]
    SpeechEnd,
    /// Error response
    #[serde(rename = "error")]
    Error(SarvamError),
}

/// Sarvam transcript response
#[derive(Debug, Deserialize)]
struct SarvamTranscript {
    /// Transcribed text
    text: String,
    /// Whether this is a final result
    #[serde(default)]
    is_final: bool,
    /// Confidence score (optional)
    #[serde(default)]
    confidence: Option<f32>,
}

/// Sarvam error response
#[derive(Debug, Deserialize)]
struct SarvamError {
    /// Error message
    message: String,
    /// Error code (optional)
    #[serde(default)]
    code: Option<String>,
}

/// Sarvam.ai STT WebSocket client
pub struct SarvamSTT {
    /// Base configuration
    config: Option<STTConfig>,
    /// Sarvam-specific configuration
    sarvam_config: Option<SarvamSTTConfig>,
    /// Current connection state
    state: ConnectionState,
    /// State change notification
    state_notify: Arc<Notify>,
    /// WebSocket sender for audio data (bounded channel for backpressure)
    ws_sender: Option<mpsc::Sender<Bytes>>,
    /// Shutdown signal sender
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Result channel sender
    result_tx: Option<mpsc::Sender<STTResult>>,
    /// Error channel sender for streaming errors
    error_tx: Option<mpsc::Sender<STTError>>,
    /// Connection handle
    connection_handle: Option<tokio::task::JoinHandle<()>>,
    /// Result forwarding task handle
    result_forward_handle: Option<tokio::task::JoinHandle<()>>,
    /// Error forwarding task handle
    error_forward_handle: Option<tokio::task::JoinHandle<()>>,
    /// Shared callback storage for async access
    result_callback: Arc<Mutex<Option<AsyncSTTCallback>>>,
    /// Error callback storage for streaming errors
    error_callback: Arc<Mutex<Option<AsyncErrorCallback>>>,
}

impl SarvamSTT {
    /// Handle incoming WebSocket messages
    fn handle_websocket_message(
        message: Message,
        result_tx: &mpsc::Sender<STTResult>,
        in_speech: &mut bool,
    ) -> Result<(), STTError> {
        match message {
            Message::Text(text) => {
                debug!("Sarvam received: {}", text);

                // Try to parse as structured response
                match serde_json::from_str::<SarvamResponse>(&text) {
                    Ok(response) => match response {
                        SarvamResponse::Transcript(transcript) => {
                            let stt_result = STTResult::new(
                                transcript.text,
                                transcript.is_final,
                                transcript.is_final && !*in_speech,
                                transcript.confidence.unwrap_or(0.95),
                            );

                            if let Err(e) = result_tx.try_send(stt_result) {
                                match e {
                                    mpsc::error::TrySendError::Full(_) => {
                                        warn!("Sarvam STT result channel full - dropping result");
                                    }
                                    mpsc::error::TrySendError::Closed(_) => {
                                        warn!("Sarvam STT result channel closed");
                                    }
                                }
                            }
                        }
                        SarvamResponse::SpeechStart => {
                            debug!("Sarvam: Speech started");
                            *in_speech = true;
                        }
                        SarvamResponse::SpeechEnd => {
                            debug!("Sarvam: Speech ended");
                            *in_speech = false;
                        }
                        SarvamResponse::Error(err) => {
                            let error_msg = format!(
                                "Sarvam error: {} (code: {})",
                                err.message,
                                err.code.unwrap_or_default()
                            );
                            return Err(STTError::ProviderError(error_msg));
                        }
                    },
                    Err(parse_err) => {
                        // Try to parse as simple text transcript (fallback)
                        if let Ok(simple) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(text_value) = simple.get("text").and_then(|v| v.as_str()) {
                                let stt_result = STTResult::new(
                                    text_value.to_string(),
                                    simple
                                        .get("is_final")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false),
                                    false,
                                    simple
                                        .get("confidence")
                                        .and_then(|v| v.as_f64())
                                        .unwrap_or(0.95) as f32,
                                );

                                if let Err(e) = result_tx.try_send(stt_result) {
                                    warn!("Failed to send Sarvam result: {:?}", e);
                                }
                                return Ok(());
                            }
                        }

                        // Check if it's an error message
                        if text.contains("error") || text.contains("Error") {
                            return Err(STTError::ProviderError(format!(
                                "Sarvam error response: {}",
                                text
                            )));
                        }

                        debug!("Unrecognized Sarvam message format: {:?}", parse_err);
                    }
                }
            }
            Message::Close(close_frame) => {
                info!("Sarvam WebSocket closed: {:?}", close_frame);
            }
            Message::Pong(_) => {
                debug!("Sarvam: Received pong");
            }
            _ => {
                // Ignore other message types
            }
        }

        Ok(())
    }

    /// Start the WebSocket connection task
    async fn start_connection(
        &mut self,
        base_config: STTConfig,
        sarvam_config: SarvamSTTConfig,
    ) -> Result<(), STTError> {
        // Validate configuration
        sarvam_config.validate().map_err(STTError::ConfigurationError)?;

        let ws_url = sarvam_config.build_websocket_url();

        // Create channels for communication (bounded for backpressure)
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
        let api_key = base_config.api_key.clone();

        // Start the connection task
        let connection_handle = tokio::spawn(async move {
            // Connect to Sarvam
            // IMPORTANT: Sarvam uses "api-subscription-key" header, NOT "Authorization: Bearer"
            let request = match tokio_tungstenite::tungstenite::http::Request::builder()
                .method("GET")
                .uri(&ws_url)
                .header("Upgrade", "websocket")
                .header("Connection", "upgrade")
                .header("Sec-WebSocket-Key", generate_key())
                .header("Sec-WebSocket-Version", "13")
                .header("api-subscription-key", &api_key) // Sarvam's custom auth header
                .body(())
            {
                Ok(request) => request,
                Err(e) => {
                    let stt_error = STTError::ConnectionFailed(format!(
                        "Failed to create Sarvam WebSocket request: {e}"
                    ));
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            };

            let (ws_stream, _) = match connect_async(request).await {
                Ok(result) => result,
                Err(e) => {
                    let stt_error =
                        STTError::ConnectionFailed(format!("Failed to connect to Sarvam: {e}"));
                    error!("{}", stt_error);
                    let _ = error_tx.try_send(stt_error);
                    return;
                }
            };

            info!("Connected to Sarvam STT WebSocket");

            // Signal successful connection
            let _ = connected_tx.send(());

            let (mut ws_sink, mut ws_stream) = ws_stream.split();

            // Keep-alive mechanism - Sarvam requires ping within 60 seconds
            let mut keepalive_timer = interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
            let mut last_activity = Instant::now();

            // Track speech state for is_speech_final
            let mut in_speech = false;

            // Main event loop
            loop {
                tokio::select! {
                    // Handle outgoing audio data
                    Some(audio_data) = ws_rx.recv() => {
                        // Encode audio as base64 and send as JSON
                        let b64_audio = BASE64_STANDARD.encode(&audio_data);
                        let msg = SarvamAudioMessage { audio: b64_audio };

                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                let message = Message::Text(json.into());
                                if let Err(e) = ws_sink.send(message).await {
                                    let stt_error = STTError::NetworkError(format!(
                                        "Failed to send audio to Sarvam: {e}"
                                    ));
                                    error!("{}", stt_error);
                                    let _ = error_tx.try_send(stt_error);
                                    break;
                                }
                                // Update last activity time
                                last_activity = Instant::now();
                            }
                            Err(e) => {
                                warn!("Failed to serialize audio message: {}", e);
                            }
                        }
                    }

                    // Handle incoming messages with idle timeout
                    message = timeout(Duration::from_secs(MESSAGE_TIMEOUT_SECS), ws_stream.next()) => {
                        match message {
                            Ok(Some(Ok(msg))) => {
                                if let Err(e) = Self::handle_websocket_message(msg, &result_tx, &mut in_speech) {
                                    error!("Sarvam STT error: {}", e);
                                    let _ = error_tx.try_send(e);
                                    break;
                                }
                            }
                            Ok(Some(Err(e))) => {
                                let stt_error = STTError::NetworkError(format!(
                                    "Sarvam WebSocket error: {e}"
                                ));
                                error!("{}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                break;
                            }
                            Ok(None) => {
                                info!("Sarvam WebSocket stream ended");
                                break;
                            }
                            Err(_elapsed) => {
                                let stt_error = STTError::NetworkError(
                                    "Sarvam WebSocket idle timeout - no message for 60 seconds".into()
                                );
                                error!("{}", stt_error);
                                let _ = error_tx.try_send(stt_error);
                                break;
                            }
                        }
                    }

                    // Handle keep-alive timer
                    _ = keepalive_timer.tick() => {
                        // Send ping if no activity in the last interval
                        if last_activity.elapsed() >= Duration::from_secs(KEEPALIVE_INTERVAL_SECS) {
                            let ping_msg = SarvamPingMessage { msg_type: "ping" };
                            match serde_json::to_string(&ping_msg) {
                                Ok(json) => {
                                    let message = Message::Text(json.into());
                                    if let Err(e) = ws_sink.send(message).await {
                                        let stt_error = STTError::NetworkError(format!(
                                            "Failed to send Sarvam keep-alive: {e}"
                                        ));
                                        error!("{}", stt_error);
                                        let _ = error_tx.try_send(stt_error);
                                        break;
                                    }
                                    debug!("Sent keep-alive ping to Sarvam");
                                }
                                Err(e) => {
                                    warn!("Failed to serialize ping message: {}", e);
                                }
                            }
                        }
                    }

                    // Handle shutdown signal
                    _ = &mut shutdown_rx => {
                        info!("Sarvam STT received shutdown signal");
                        break;
                    }
                }
            }

            info!("Sarvam STT WebSocket connection closed");
        });

        self.connection_handle = Some(connection_handle);

        // Start result forwarding task with shared callback
        let callback_ref = self.result_callback.clone();
        let result_forwarding_handle = tokio::spawn(async move {
            while let Some(result) = result_rx.recv().await {
                if let Some(callback) = callback_ref.lock().await.as_ref() {
                    callback(result).await;
                } else {
                    debug!(
                        "Sarvam STT result: {} (confidence: {})",
                        result.transcript, result.confidence
                    );
                }
            }
        });

        self.result_forward_handle = Some(result_forwarding_handle);

        // Start error forwarding task with shared callback
        let error_callback_ref = self.error_callback.clone();
        let error_forwarding_handle = tokio::spawn(async move {
            while let Some(error) = error_rx.recv().await {
                if let Some(callback) = error_callback_ref.lock().await.as_ref() {
                    callback(error).await;
                } else {
                    error!("Sarvam STT error (no callback): {}", error);
                }
            }
        });

        self.error_forward_handle = Some(error_forwarding_handle);

        // Update state and wait for connection
        self.state = ConnectionState::Connecting;

        // Wait for connection to be established with timeout
        match timeout(Duration::from_secs(CONNECTION_TIMEOUT_SECS), connected_rx).await {
            Ok(Ok(())) => {
                self.state = ConnectionState::Connected;
                self.state_notify.notify_waiters();
                info!("Successfully connected to Sarvam STT");
                Ok(())
            }
            Ok(Err(_)) => {
                let error_msg = "Sarvam connection channel closed".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
            Err(_) => {
                let error_msg = "Sarvam connection timeout".to_string();
                self.state = ConnectionState::Error(error_msg.clone());
                Err(STTError::ConnectionFailed(error_msg))
            }
        }
    }
}

impl Default for SarvamSTT {
    fn default() -> Self {
        Self {
            config: None,
            sarvam_config: None,
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
        }
    }
}

#[async_trait::async_trait]
impl BaseSTT for SarvamSTT {
    fn new(config: STTConfig) -> Result<Self, STTError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "SARVAM_API_KEY is required".to_string(),
            ));
        }

        // Create Sarvam-specific configuration
        let sarvam_config = SarvamSTTConfig::from_base(&config);

        Ok(Self {
            config: Some(config),
            sarvam_config: Some(sarvam_config),
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
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Get the stored configurations
        let base_config = self.config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No configuration available".to_string())
        })?;

        let sarvam_config = self.sarvam_config.as_ref().ok_or_else(|| {
            STTError::ConfigurationError("No Sarvam configuration available".to_string())
        })?;

        // Start the connection
        self.start_connection(base_config.clone(), sarvam_config.clone())
            .await
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for connection task to finish
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

        // Clean up channels and callbacks
        self.ws_sender = None;
        self.result_tx = None;
        self.error_tx = None;
        *self.result_callback.lock().await = None;
        *self.error_callback.lock().await = None;

        // Update state
        self.state = ConnectionState::Disconnected;
        self.state_notify.notify_waiters();

        info!("Disconnected from Sarvam STT");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        matches!(self.state, ConnectionState::Connected) && self.ws_sender.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Sarvam STT".to_string(),
            ));
        }

        if let Some(ws_sender) = &self.ws_sender {
            let data_len = audio_data.len();

            // Send audio data with backpressure handling
            ws_sender
                .send(audio_data)
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio to Sarvam: {e}")))?;

            debug!("Sent {} bytes of audio to Sarvam", data_len);
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
        self.config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // For Sarvam, we need to reconnect to update configuration
        if self.is_ready() {
            self.disconnect().await?;
        }

        // Update stored configuration
        let sarvam_config = SarvamSTTConfig::from_base(&config);
        self.config = Some(config);
        self.sarvam_config = Some(sarvam_config);

        self.connect().await?;
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Sarvam.ai STT (Saarika)"
    }
}

impl Drop for SarvamSTT {
    fn drop(&mut self) {
        // Send shutdown signal if still connected
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sarvam_stt_creation() {
        let config = STTConfig {
            model: "saarika:v2.5".to_string(),
            provider: "sarvam".to_string(),
            api_key: "test_key".to_string(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
        };

        let stt = <SarvamSTT as BaseSTT>::new(config).unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Sarvam.ai STT (Saarika)");
    }

    #[tokio::test]
    async fn test_sarvam_stt_requires_api_key() {
        let config = STTConfig {
            model: "saarika:v2.5".to_string(),
            provider: "sarvam".to_string(),
            api_key: String::new(), // Empty API key should fail
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
        };

        let result = <SarvamSTT as BaseSTT>::new(config);
        assert!(result.is_err());
        if let Err(STTError::AuthenticationFailed(msg)) = result {
            assert!(msg.contains("SARVAM_API_KEY"));
        } else {
            panic!("Expected AuthenticationFailed error");
        }
    }

    #[test]
    fn test_audio_message_serialization() {
        let audio_data = vec![0u8, 1, 2, 3, 4, 5];
        let b64_audio = BASE64_STANDARD.encode(&audio_data);
        let msg = SarvamAudioMessage { audio: b64_audio };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("audio"));
        assert!(json.contains("AAECAwQF")); // Base64 of [0,1,2,3,4,5]
    }

    #[test]
    fn test_ping_message_serialization() {
        let msg = SarvamPingMessage { msg_type: "ping" };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);
    }

    #[test]
    fn test_transcript_response_parsing() {
        let json = r#"{"type":"transcript","text":"नमस्ते","is_final":true,"confidence":0.95}"#;
        let response: SarvamResponse = serde_json::from_str(json).unwrap();

        match response {
            SarvamResponse::Transcript(t) => {
                assert_eq!(t.text, "नमस्ते");
                assert!(t.is_final);
                assert_eq!(t.confidence, Some(0.95));
            }
            _ => panic!("Expected Transcript response"),
        }
    }

    #[test]
    fn test_speech_events_parsing() {
        let start_json = r#"{"type":"speech_start"}"#;
        let end_json = r#"{"type":"speech_end"}"#;

        let start: SarvamResponse = serde_json::from_str(start_json).unwrap();
        let end: SarvamResponse = serde_json::from_str(end_json).unwrap();

        assert!(matches!(start, SarvamResponse::SpeechStart));
        assert!(matches!(end, SarvamResponse::SpeechEnd));
    }

    #[test]
    fn test_error_response_parsing() {
        let json = r#"{"type":"error","message":"Rate limit exceeded","code":"RATE_LIMIT"}"#;
        let response: SarvamResponse = serde_json::from_str(json).unwrap();

        match response {
            SarvamResponse::Error(e) => {
                assert_eq!(e.message, "Rate limit exceeded");
                assert_eq!(e.code, Some("RATE_LIMIT".to_string()));
            }
            _ => panic!("Expected Error response"),
        }
    }

    #[tokio::test]
    async fn test_sarvam_config_from_base() {
        let config = STTConfig {
            model: String::new(),
            provider: "sarvam".to_string(),
            api_key: "test_key".to_string(),
            language: "ta-IN".to_string(),
            sample_rate: 8000,
            channels: 1,
            punctuation: true,
            encoding: "wav".to_string(),
        };

        let stt = <SarvamSTT as BaseSTT>::new(config.clone()).unwrap();
        let stored = stt.get_config().unwrap();

        assert_eq!(stored.api_key, "test_key");
        assert_eq!(stored.language, "ta-IN");
        assert_eq!(stored.sample_rate, 8000);
    }

    #[tokio::test]
    async fn test_send_audio_before_connect() {
        let config = STTConfig {
            model: "saarika:v2.5".to_string(),
            provider: "sarvam".to_string(),
            api_key: "test_key".to_string(),
            language: "hi-IN".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "pcm_s16le".to_string(),
        };

        let mut stt = <SarvamSTT as BaseSTT>::new(config).unwrap();

        // Should fail because not connected
        let result = stt.send_audio(Bytes::from(vec![0u8; 1024])).await;
        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("Not connected"));
        }
    }
}
