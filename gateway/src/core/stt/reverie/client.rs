//! Reverie STT Client Implementation
//!
//! Implements the BaseSTT trait for Reverie real-time speech-to-text.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, trace, warn};

use crate::core::stt::base::{
    BaseSTT, STTConfig, STTConnectionState, STTError, STTErrorCallback, STTResult,
    STTResultCallback, STTStats,
};

use super::EOF_MARKER;
use super::config::ReverieSTTConfig;
use super::messages::ReverieServerMessage;

// =============================================================================
// Type Aliases
// =============================================================================

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;

// =============================================================================
// ReverieSTT Implementation
// =============================================================================

/// Reverie STT client implementing BaseSTT trait
pub struct ReverieSTT {
    /// Provider-specific configuration
    reverie_config: ReverieSTTConfig,
    /// Base STT configuration (stored for get_config)
    base_config: Option<STTConfig>,
    /// Current connection state
    state: Arc<RwLock<STTConnectionState>>,
    /// WebSocket sink for sending messages
    ws_sink: Arc<RwLock<Option<WsSink>>>,
    /// Session ID from server
    session_id: Arc<RwLock<Option<String>>>,
    /// Result callback
    on_result: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback
    on_error: Arc<RwLock<Option<STTErrorCallback>>>,
    /// Ready flag
    is_ready: Arc<AtomicBool>,
    /// Statistics
    stats: Arc<RwLock<STTStats>>,
    /// Bytes sent counter
    bytes_sent: Arc<AtomicU64>,
}

impl ReverieSTT {
    /// Create a new Reverie STT client from provider-specific config
    pub fn with_config(config: ReverieSTTConfig) -> Result<Self, STTError> {
        // Validate configuration
        config.validate().map_err(STTError::ConfigurationError)?;

        Ok(Self {
            reverie_config: config,
            base_config: None,
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sink: Arc::new(RwLock::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
        })
    }

    /// W1 keystone — construct directly from the standardized config. Reverie exposes no
    /// advanced-feature query knobs, so this is a uniform standardized entry point that delegates
    /// to `ReverieSTTConfig::from_standard` (a `from_base` passthrough) and then `with_config`.
    /// Mirrors `DeepgramSTT::new_standard`: validate the api_key, then build from the
    /// standardized->provider config mapping.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        if std.base.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "API key is required".to_string(),
            ));
        }
        let cfg = crate::core::stt::reverie::config::ReverieSTTConfig::from_standard(std)
            .map_err(STTError::ConfigurationError)?;
        Self::with_config(cfg)
    }

    /// Spawn the WebSocket message receiver task
    fn spawn_receiver_task(&self, mut ws_stream: futures_util::stream::SplitStream<WsStream>) {
        let on_result = self.on_result.clone();
        let on_error = self.on_error.clone();
        let state = self.state.clone();
        let is_ready = self.is_ready.clone();
        let stats = self.stats.clone();
        let session_id = self.session_id.clone();
        let continuous = self.reverie_config.continuous;

        tokio::spawn(async move {
            while let Some(message_result) = ws_stream.next().await {
                match message_result {
                    Ok(Message::Text(text)) => {
                        trace!("Received text message: {}", text);

                        let server_msg = ReverieServerMessage::from_json(&text);

                        match server_msg {
                            ReverieServerMessage::Partial(result)
                            | ReverieServerMessage::Final(result) => {
                                // Store session ID if present
                                if let Some(id) = &result.id {
                                    *session_id.write().await = Some(id.clone());
                                }

                                // Create STTResult
                                let stt_result = STTResult::new(
                                    result.best_text().unwrap_or("").to_string(),
                                    result.r#final,
                                    result.r#final,
                                    result.confidence_f64().unwrap_or(0.0) as f32,
                                );

                                // Update stats
                                {
                                    let mut s = stats.write().await;
                                    s.update_with_result(&stt_result);
                                }

                                // Invoke callback
                                if let Some(callback) = on_result.read().await.as_ref() {
                                    callback(stt_result).await;
                                }

                                // Check if we should close
                                if result.should_close(continuous) {
                                    debug!("Final result received, closing connection");
                                    is_ready.store(false, Ordering::SeqCst);
                                    *state.write().await = STTConnectionState::Disconnected;
                                    break;
                                }
                            }
                            ReverieServerMessage::Error(err_msg) => {
                                error!("Reverie error: {}", err_msg);
                                if let Some(callback) = on_error.read().await.as_ref() {
                                    callback(STTError::ProviderError(err_msg)).await;
                                }
                            }
                            ReverieServerMessage::Unknown(msg) => {
                                warn!("Unknown message: {}", msg);
                            }
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        trace!("Received binary message: {} bytes", data.len());
                    }
                    Ok(Message::Close(frame)) => {
                        info!("WebSocket closed: {:?}", frame);
                        is_ready.store(false, Ordering::SeqCst);
                        *state.write().await = STTConnectionState::Disconnected;
                        break;
                    }
                    Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                        // Handled by tungstenite
                    }
                    Ok(Message::Frame(_)) => {
                        // Raw frame, ignore
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        is_ready.store(false, Ordering::SeqCst);
                        *state.write().await = STTConnectionState::Error(e.to_string());
                        if let Some(callback) = on_error.read().await.as_ref() {
                            callback(STTError::NetworkError(e.to_string())).await;
                        }
                        break;
                    }
                }
            }
            debug!("Reverie receiver task ended");
        });
    }
}

#[async_trait::async_trait]
impl BaseSTT for ReverieSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        // Check for API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Reverie API key is required. Set api_key or REVERIE_API_KEY env var".to_string(),
            ));
        }

        let reverie_config =
            ReverieSTTConfig::from_base(&config).map_err(STTError::ConfigurationError)?;

        Ok(Self {
            reverie_config,
            base_config: Some(config),
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sink: Arc::new(RwLock::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        // Check if already connected
        {
            let current_state = self.state.read().await;
            if matches!(*current_state, STTConnectionState::Connected) {
                return Ok(());
            }
        }

        // Update state to connecting
        *self.state.write().await = STTConnectionState::Connecting;
        info!("Connecting to Reverie STT...");

        // Build WebSocket URL with all params
        let url = self.reverie_config.build_websocket_url();
        debug!(
            "WebSocket URL: {}",
            url.replace(&self.reverie_config.api_key, "[REDACTED]")
        );

        // Connect to WebSocket
        let (ws_stream, response) = connect_async(&url).await.map_err(|e| {
            // Check for specific error types
            let error_str = e.to_string();
            if error_str.contains("401") || error_str.contains("Unauthorized") {
                STTError::AuthenticationFailed(format!("Invalid API credentials: {}", e))
            } else if error_str.contains("400") || error_str.contains("Bad Request") {
                STTError::ConfigurationError(format!("Invalid configuration: {}", e))
            } else {
                STTError::ConnectionFailed(format!("WebSocket connection failed: {}", e))
            }
        })?;

        debug!(
            "WebSocket connected, response status: {:?}",
            response.status()
        );

        // Split the stream
        let (sink, stream) = ws_stream.split();

        // Store the sink
        *self.ws_sink.write().await = Some(sink);

        // Spawn receiver task
        self.spawn_receiver_task(stream);

        // Update state
        *self.state.write().await = STTConnectionState::Connected;
        self.is_ready.store(true, Ordering::SeqCst);

        info!(
            "Connected to Reverie STT (lang: {}, domain: {})",
            self.reverie_config.language, self.reverie_config.domain
        );
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        info!("Disconnecting from Reverie STT...");

        // Send EOF marker to signal end of stream
        if let Some(sink) = self.ws_sink.write().await.as_mut() {
            // Send EOF as binary message
            match sink.send(Message::Binary(EOF_MARKER.to_vec().into())).await {
                Ok(_) => debug!("Sent EOF marker"),
                Err(e) => warn!("Failed to send EOF marker: {}", e),
            }

            // Close the WebSocket
            let _ = sink.close().await;
        }

        // Clear state
        *self.ws_sink.write().await = None;
        *self.session_id.write().await = None;
        *self.state.write().await = STTConnectionState::Disconnected;
        self.is_ready.store(false, Ordering::SeqCst);

        info!("Disconnected from Reverie STT");
        Ok(())
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        let mut sink_guard = self.ws_sink.write().await;
        let sink = sink_guard
            .as_mut()
            .ok_or_else(|| STTError::ConnectionFailed("Not connected".to_string()))?;

        // Send as binary message (Reverie expects raw audio bytes)
        sink.send(Message::Binary(audio_data.to_vec().into()))
            .await
            .map_err(|e| STTError::NetworkError(e.to_string()))?;

        // Update stats
        self.bytes_sent
            .fetch_add(audio_data.len() as u64, Ordering::Relaxed);
        {
            let mut stats = self.stats.write().await;
            stats.total_audio_bytes += audio_data.len() as u64;
        }

        trace!("Sent {} bytes of audio", audio_data.len());
        Ok(())
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        *self.on_result.write().await = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        *self.on_error.write().await = Some(callback);
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.base_config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        // Update the reverie config
        let new_reverie_config =
            ReverieSTTConfig::from_base(&config).map_err(STTError::ConfigurationError)?;
        self.reverie_config = new_reverie_config;
        self.base_config = Some(config);

        // If connected, we need to reconnect with the new config
        if self.is_ready() {
            self.disconnect().await?;
            self.connect().await?;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Reverie STT (Indian Languages)"
    }
}

// =============================================================================
// Factory Function
// =============================================================================

/// Create a new Reverie STT provider from base configuration
pub fn create_reverie_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    // Check for API key
    if config.api_key.is_empty() {
        return Err(STTError::AuthenticationFailed(
            "Reverie API key is required. Set api_key or REVERIE_API_KEY env var".to_string(),
        ));
    }

    let stt = ReverieSTT::new(config)?;
    Ok(Box::new(stt))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_reverie_config() -> ReverieSTTConfig {
        ReverieSTTConfig::new("test-api-key", "test-app-id")
    }

    fn create_test_base_config() -> STTConfig {
        STTConfig {
            provider: "reverie".to_string(),
            api_key: "test-api-key".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(), // app_id goes in model field
        }
    }

    #[test]
    fn test_reverie_stt_with_config() {
        let config = create_test_reverie_config();
        let stt = ReverieSTT::with_config(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Reverie STT (Indian Languages)");
    }

    // W1 keystone: Reverie maps zero standardized feature knobs, so `new_standard` is a pure
    // passthrough of the base config — assert the base (api_key + app_id from the model field +
    // language) survives through the provider-struct method to the provider-specific config.
    #[test]
    fn test_reverie_new_standard_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: create_test_base_config(),
            features: SttFeatures {
                // None of these can map to a real Reverie field; they must be ignored.
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let stt = ReverieSTT::new_standard(&std).expect("new_standard must succeed");
        assert_eq!(stt.reverie_config.api_key, "test-api-key");
        assert_eq!(stt.reverie_config.app_id, "test-app-id"); // parsed from model field
        assert_eq!(
            stt.reverie_config.language,
            super::super::config::ReverieLanguage::Hindi
        );

        // Missing api_key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            provider: "reverie".into(),
            api_key: String::new(),
            model: "test-app-id".into(),
            ..Default::default()
        });
        assert!(ReverieSTT::new_standard(&bad).is_err());
    }

    #[test]
    fn test_reverie_stt_with_empty_api_key() {
        let config = ReverieSTTConfig::default();
        let stt = ReverieSTT::with_config(config);
        assert!(stt.is_err());
    }

    #[test]
    fn test_reverie_stt_new() {
        let config = create_test_base_config();
        let stt = ReverieSTT::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[test]
    fn test_reverie_stt_new_empty_api_key() {
        let config = STTConfig {
            provider: "reverie".to_string(),
            api_key: String::new(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(),
        };
        let stt = ReverieSTT::new(config);
        assert!(stt.is_err());
        if let Err(err) = stt {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    #[test]
    fn test_reverie_stt_new_missing_app_id() {
        let config = STTConfig {
            provider: "reverie".to_string(),
            api_key: "test-key".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: String::new(), // No app_id
        };
        // This will try to read from REVERIE_APP_ID env var, which won't be set in tests
        let stt = ReverieSTT::new(config);
        assert!(stt.is_err());
        if let Err(err) = stt {
            assert!(matches!(err, STTError::ConfigurationError(_)));
        }
    }

    #[test]
    fn test_create_reverie_stt_factory() {
        let config = create_test_base_config();
        let result = create_reverie_stt(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Reverie STT (Indian Languages)");
    }

    #[test]
    fn test_create_reverie_stt_factory_empty_api_key() {
        let config = STTConfig {
            provider: "reverie".to_string(),
            api_key: String::new(),
            language: "hi".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "test-app-id".to_string(),
        };

        let result = create_reverie_stt(config);
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    #[tokio::test]
    async fn test_reverie_stt_initial_state() {
        let config = create_test_base_config();
        let stt = ReverieSTT::new(config).unwrap();

        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[tokio::test]
    async fn test_reverie_stt_send_audio_not_connected() {
        let config = create_test_base_config();
        let mut stt = ReverieSTT::new(config).unwrap();

        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::ConnectionFailed(_)));
        }
    }

    #[tokio::test]
    async fn test_reverie_stt_callbacks() {
        use std::sync::atomic::AtomicUsize;

        let config = create_test_base_config();
        let mut stt = ReverieSTT::new(config).unwrap();

        let result_count = Arc::new(AtomicUsize::new(0));
        let result_count_clone = result_count.clone();

        let callback = Arc::new(move |_result: STTResult| {
            let count = result_count_clone.clone();
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_reverie_stt_get_config() {
        let config = create_test_base_config();
        let stt = ReverieSTT::new(config).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "test-api-key");
        assert_eq!(stored_config.language, "hi");
        assert_eq!(stored_config.sample_rate, 16000);
    }

    #[test]
    fn test_reverie_websocket_url_construction() {
        let config = ReverieSTTConfig::new("test-key", "test-app")
            .with_language(super::super::config::ReverieLanguage::Hindi)
            .with_timeout(60)
            .with_continuous(true);

        let url = config.build_websocket_url();
        assert!(url.starts_with("wss://revapi.reverieinc.com/stream?"));
        assert!(url.contains("apikey=test-key"));
        assert!(url.contains("appid=test-app"));
        assert!(url.contains("src_lang=hi"));
        assert!(url.contains("timeout=60"));
        assert!(url.contains("continuous=1"));
    }
}
