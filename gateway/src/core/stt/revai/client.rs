//! Rev AI STT Client
//!
//! WebSocket-based streaming speech-to-text client for Rev AI API.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};
use tracing::{debug, error, info, trace, warn};

use crate::core::stt::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback, STTStats,
};

use super::config::RevAISTTConfig;
use super::messages::{RevAICloseCode, ServerMessage};
use super::EOS_MESSAGE;

// =============================================================================
// Type Aliases
// =============================================================================

type WebSocketSink =
    futures_util::stream::SplitSink<tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, Message>;

// =============================================================================
// Rev AI STT Client
// =============================================================================

/// Rev AI STT client implementing BaseSTT trait
pub struct RevAISTT {
    /// Base STT configuration
    config: STTConfig,

    /// Rev AI specific configuration
    revai_config: RevAISTTConfig,

    /// WebSocket sender (write half)
    ws_sink: Option<Arc<RwLock<WebSocketSink>>>,

    /// Connection state flag
    connected: AtomicBool,

    /// Session ID from connected message
    session_id: Arc<RwLock<Option<String>>>,

    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,

    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,

    /// Statistics
    stats: Arc<RwLock<STTStats>>,
}

impl RevAISTT {
    /// Create a new Rev AI STT client from Rev AI config
    pub fn from_revai_config(config: RevAISTTConfig) -> Result<Self, STTError> {
        config.validate()?;

        let base_config: STTConfig = config.clone().into();

        Ok(Self {
            config: base_config,
            revai_config: config,
            ws_sink: None,
            connected: AtomicBool::new(false),
            session_id: Arc::new(RwLock::new(None)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(STTStats::default())),
        })
    }

    /// Get the session ID
    pub async fn get_session_id(&self) -> Option<String> {
        self.session_id.read().await.clone()
    }

    /// Get statistics
    pub async fn get_stats(&self) -> STTStats {
        self.stats.read().await.clone()
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
                        let result = STTResult::new(
                            text,
                            false,
                            false,
                            partial.average_confidence() as f32,
                        );
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
            ws_sink: None,
            connected: AtomicBool::new(false),
            session_id: Arc::new(RwLock::new(None)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(STTStats::default())),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::Acquire) {
            warn!("Rev AI: Already connected");
            return Ok(());
        }

        info!("Rev AI: Connecting to streaming endpoint");

        // Build WebSocket URL with all parameters
        let ws_url = self.revai_config.build_websocket_url();
        debug!(url = %ws_url.replace(&self.revai_config.api_key, "[REDACTED]"), "Rev AI: WebSocket URL");

        // Create request
        let request = ws_url
            .into_client_request()
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to create request: {}", e)))?;

        // Connect to WebSocket
        let (ws_stream, response) = connect_async(request)
            .await
            .map_err(|e| STTError::ConnectionFailed(format!("WebSocket connection failed: {}", e)))?;

        debug!(status = ?response.status(), "Rev AI: WebSocket connection established");

        // Split the stream
        let (ws_sink, mut ws_stream) = ws_stream.split();
        self.ws_sink = Some(Arc::new(RwLock::new(ws_sink)));

        // Set connected flag
        self.connected.store(true, Ordering::Release);

        // Clone necessary references for the message handler task
        let result_callback = Arc::clone(&self.result_callback);
        let error_callback = Arc::clone(&self.error_callback);
        let session_id = Arc::clone(&self.session_id);
        let stats = Arc::clone(&self.stats);
        let connected = AtomicBool::new(true);

        // Spawn message handler task
        tokio::spawn(async move {
            while let Some(message) = ws_stream.next().await {
                match message {
                    Ok(Message::Text(text)) => {
                        Self::handle_message(
                            &text,
                            &result_callback,
                            &error_callback,
                            &session_id,
                            &stats,
                        )
                        .await;
                    }
                    Ok(Message::Close(frame)) => {
                        let (code, reason) = frame
                            .map(|f| (f.code.into(), f.reason.to_string()))
                            .unwrap_or((1000, String::new()));

                        Self::handle_close(code, &reason, &error_callback).await;
                        connected.store(false, Ordering::Release);
                        break;
                    }
                    Ok(Message::Ping(data)) => {
                        trace!("Rev AI: Received ping");
                        // Pong is handled automatically by tungstenite
                        let _ = data;
                    }
                    Ok(Message::Pong(_)) => {
                        trace!("Rev AI: Received pong");
                    }
                    Ok(Message::Binary(data)) => {
                        warn!(
                            len = data.len(),
                            "Rev AI: Received unexpected binary message"
                        );
                    }
                    Ok(Message::Frame(_)) => {
                        // Raw frame, ignore
                    }
                    Err(e) => {
                        error!(error = %e, "Rev AI: WebSocket error");
                        if let Some(ref callback) = *error_callback.read().await {
                            callback(STTError::NetworkError(format!("WebSocket error: {}", e)))
                                .await;
                        }
                        connected.store(false, Ordering::Release);
                        break;
                    }
                }
            }

            info!("Rev AI: Message handler task ended");
        });

        // Wait for connected message
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        info!("Rev AI: Successfully connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(());
        }

        info!("Rev AI: Disconnecting");

        // Send EOS message for graceful close
        if let Some(ref ws_sink) = self.ws_sink {
            let mut sink = ws_sink.write().await;

            // Send EOS text message
            if let Err(e) = sink.send(Message::Text(EOS_MESSAGE.to_string().into())).await {
                warn!(error = %e, "Rev AI: Failed to send EOS message");
            }

            // Wait a bit for final results
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Close the WebSocket
            if let Err(e) = sink.close().await {
                warn!(error = %e, "Rev AI: Error closing WebSocket");
            }
        }

        self.connected.store(false, Ordering::Release);
        self.ws_sink = None;
        *self.session_id.write().await = None;

        info!("Rev AI: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.ws_sink.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Rev AI".to_string(),
            ));
        }

        let ws_sink = self
            .ws_sink
            .as_ref()
            .ok_or_else(|| STTError::ConnectionFailed("WebSocket not available".to_string()))?;

        // Send binary audio data
        let mut sink = ws_sink.write().await;
        sink.send(Message::Binary(bytes::Bytes::from(audio_data.to_vec())))
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to send audio: {}", e)))?;

        // Update stats
        self.stats.write().await.total_audio_bytes += audio_data.len() as u64;

        trace!(bytes = audio_data.len(), "Rev AI: Sent audio chunk");
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
