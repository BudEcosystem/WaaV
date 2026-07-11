//! Phonexia STT Client
//!
//! WebSocket-based streaming speech-to-text client for Phonexia on-premises server.

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Message, client::IntoClientRequest, http::HeaderValue},
};
use tracing::{debug, error, info, trace, warn};

use crate::config::utils::parse_bool;
use crate::core::stt::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback, STTStats,
};

use super::SESSION_ID_HEADER;
use super::config::{PhonexiaAuth, PhonexiaSTTConfig};
use super::messages::{PhonexiaCloseCode, PhonexiaErrorCode, ServerMessage};

// =============================================================================
// Type Aliases
// =============================================================================

const WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV: &str = "WAAV_PHONEXIA_ALLOW_UNVERIFIED";

type WebSocketSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

fn phonexia_allow_unverified_from_env() -> Result<bool, STTError> {
    match std::env::var(WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV) {
        Ok(value) => parse_bool(&value).ok_or_else(|| {
            STTError::ConfigurationError(format!(
                "{WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV} must be a boolean (true/false, 1/0, yes/no), got {value:?}"
            ))
        }),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(std::env::VarError::NotUnicode(_)) => Err(STTError::ConfigurationError(format!(
            "{WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV} must be valid UTF-8 boolean text"
        ))),
    }
}

// =============================================================================
// Phonexia STT Client
// =============================================================================

/// Phonexia STT client implementing BaseSTT trait
pub struct PhonexiaSTT {
    /// Base STT configuration
    config: STTConfig,

    /// Phonexia-specific configuration
    phonexia_config: PhonexiaSTTConfig,

    /// WebSocket sender (write half)
    ws_sink: Option<Arc<RwLock<WebSocketSink>>>,

    /// Connection state flag
    connected: AtomicBool,

    /// Stream ID from server
    stream_id: Arc<RwLock<Option<String>>>,

    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,

    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,

    /// Statistics
    stats: Arc<RwLock<STTStats>>,
}

impl PhonexiaSTT {
    /// Create a new Phonexia STT client from Phonexia config
    pub fn from_phonexia_config(config: PhonexiaSTTConfig) -> Result<Self, STTError> {
        config.validate()?;

        let base_config: STTConfig = config.clone().into();

        Ok(Self {
            config: base_config,
            phonexia_config: config,
            ws_sink: None,
            connected: AtomicBool::new(false),
            stream_id: Arc::new(RwLock::new(None)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(STTStats::default())),
        })
    }

    /// W1 keystone — construct directly from the standardized config so the advanced features
    /// Phonexia can express (per-word timestamps, key terms / phrase hints) are honored
    /// END-TO-END. Mirrors `DeepgramSTT::new_standard`: validate the credential, then build the
    /// provider from the standardized->provider config mapping (`PhonexiaSTTConfig::from_standard`).
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        // Phonexia is on-premises: the server URL is carried in the api_key field.
        if std.base.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Phonexia server URL is required (carried in the api_key field)".to_string(),
            ));
        }
        Self::from_phonexia_config(
            crate::core::stt::phonexia::config::PhonexiaSTTConfig::from_standard(std)?,
        )
    }

    /// Get the stream ID
    pub async fn get_stream_id(&self) -> Option<String> {
        self.stream_id.read().await.clone()
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
        stream_id: &Arc<RwLock<Option<String>>>,
        stats: &Arc<RwLock<STTStats>>,
    ) {
        match ServerMessage::from_json(message) {
            Ok(server_msg) => match server_msg {
                ServerMessage::Result(result) => {
                    let text = result.text();
                    let confidence = result.average_confidence() as f32;
                    let is_final = result.is_last;

                    if is_final {
                        debug!(
                            text = %text,
                            confidence = %confidence,
                            word_count = %result.word_count(),
                            "Phonexia: Final transcript"
                        );
                    } else {
                        trace!(text = %text, "Phonexia: Partial transcript");
                    }

                    let stt_result = STTResult::new(text, is_final, is_final, confidence);

                    // Update stats for final results
                    if is_final {
                        stats.write().await.update_with_result(&stt_result);
                    }

                    if let Some(ref callback) = *result_callback.read().await {
                        callback(stt_result).await;
                    }
                }
                ServerMessage::Error(error) => {
                    let error_code = error
                        .code
                        .map(PhonexiaErrorCode::from_code)
                        .unwrap_or(PhonexiaErrorCode::Unknown);

                    error!(
                        code = ?error.code,
                        message = %error.message,
                        description = %error_code.description(),
                        "Phonexia: Server error"
                    );

                    if let Some(ref callback) = *error_callback.read().await {
                        callback(STTError::ProviderError(format!("{}", error))).await;
                    }
                }
                ServerMessage::Status(status) => {
                    if let Some(ref id) = status.stream_id {
                        info!(stream_id = %id, "Phonexia: Connected to stream");
                        *stream_id.write().await = Some(id.clone());
                    }

                    if let Some(ref msg) = status.message {
                        debug!(message = %msg, "Phonexia: Status message");
                    }
                }
            },
            Err(e) => {
                warn!(error = %e, message = %message, "Phonexia: Failed to parse server message");
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
        let close_code = PhonexiaCloseCode::from_code(code);
        let description = close_code.description();

        if close_code == PhonexiaCloseCode::Normal {
            info!("Phonexia: WebSocket closed normally");
        } else {
            warn!(
                code = %code,
                reason = %reason,
                description = %description,
                retryable = %close_code.is_retryable(),
                "Phonexia: WebSocket closed with error"
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
impl BaseSTT for PhonexiaSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        // For Phonexia, the api_key field contains the server URL
        if config.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Phonexia server URL is required. Phonexia is an on-premises solution - \
                 set the server URL in the api_key field (e.g., 'https://your-phonexia-server.com')."
                    .to_string(),
            ));
        }

        let phonexia_config = PhonexiaSTTConfig::from_base(&config)?;

        Ok(Self {
            config,
            phonexia_config,
            ws_sink: None,
            connected: AtomicBool::new(false),
            stream_id: Arc::new(RwLock::new(None)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(STTStats::default())),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.connected.load(Ordering::Acquire) {
            warn!("Phonexia: Already connected");
            return Ok(());
        }

        // FAIL CLOSED (BRUTAL_REVIEW.md "Phonexia" / PRODUCTION_PLAN.md W3): the real Phonexia
        // Speech Platform 4 exposes gRPC + a REST async-job API, NOT the generic `/ws` +
        // `X-SessionID` protocol implemented here. That protocol is unverified against any real
        // Phonexia server, so we refuse by default rather than silently "succeed" against a
        // fabricated wire format. Operators who have a server matching this protocol can opt in
        // with WAAV_PHONEXIA_ALLOW_UNVERIFIED=1.
        if !phonexia_allow_unverified_from_env()? {
            return Err(STTError::ConfigurationError(
                "Phonexia STT is not validated against the real Phonexia API (gRPC/REST). It is \
                 disabled by default to avoid a fabricated-protocol connection. Set \
                 WAAV_PHONEXIA_ALLOW_UNVERIFIED=1 only if your server matches the implemented \
                 WebSocket protocol. Tracked by PRODUCTION_PLAN.md W3."
                    .to_string(),
            ));
        }

        info!(
            server = %self.phonexia_config.server_url,
            "Phonexia: Connecting to server"
        );

        // Build WebSocket URL with query parameters
        let ws_url = self.phonexia_config.build_websocket_url();
        debug!(url = %ws_url, "Phonexia: WebSocket URL");

        // Create request with authentication headers
        let mut request = ws_url
            .into_client_request()
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to create request: {}", e)))?;

        // Add authentication header if configured
        match &self.phonexia_config.auth {
            PhonexiaAuth::Token { token } => {
                request.headers_mut().insert(
                    SESSION_ID_HEADER,
                    HeaderValue::from_str(token).map_err(|e| {
                        STTError::AuthenticationFailed(format!("Invalid token: {}", e))
                    })?,
                );
            }
            PhonexiaAuth::Basic { username, password } => {
                let credentials = format!("{}:{}", username, password);
                let encoded = base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    credentials.as_bytes(),
                );
                request.headers_mut().insert(
                    "Authorization",
                    HeaderValue::from_str(&format!("Basic {}", encoded)).map_err(|e| {
                        STTError::AuthenticationFailed(format!("Invalid credentials: {}", e))
                    })?,
                );
            }
            PhonexiaAuth::None => {
                // No authentication
            }
        }

        // Connect to WebSocket
        let (ws_stream, response) = connect_async(request).await.map_err(|e| {
            STTError::ConnectionFailed(format!("WebSocket connection failed: {}", e))
        })?;

        debug!(status = ?response.status(), "Phonexia: WebSocket connection established");

        // Split the stream
        let (ws_sink, mut ws_stream) = ws_stream.split();
        self.ws_sink = Some(Arc::new(RwLock::new(ws_sink)));

        // Set connected flag
        self.connected.store(true, Ordering::Release);

        // Clone necessary references for the message handler task
        let result_callback = Arc::clone(&self.result_callback);
        let error_callback = Arc::clone(&self.error_callback);
        let stream_id = Arc::clone(&self.stream_id);
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
                            &stream_id,
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
                        trace!("Phonexia: Received ping");
                        let _ = data;
                    }
                    Ok(Message::Pong(_)) => {
                        trace!("Phonexia: Received pong");
                    }
                    Ok(Message::Binary(data)) => {
                        // Phonexia may send binary results
                        if let Ok(text) = String::from_utf8(data.to_vec()) {
                            Self::handle_message(
                                &text,
                                &result_callback,
                                &error_callback,
                                &stream_id,
                                &stats,
                            )
                            .await;
                        } else {
                            warn!(
                                len = data.len(),
                                "Phonexia: Received non-UTF8 binary message"
                            );
                        }
                    }
                    Ok(Message::Frame(_)) => {
                        // Raw frame, ignore
                    }
                    Err(e) => {
                        error!(error = %e, "Phonexia: WebSocket error");
                        if let Some(ref callback) = *error_callback.read().await {
                            callback(STTError::NetworkError(format!("WebSocket error: {}", e)))
                                .await;
                        }
                        connected.store(false, Ordering::Release);
                        break;
                    }
                }
            }

            info!("Phonexia: Message handler task ended");
        });

        // Wait for connection to stabilize
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        info!("Phonexia: Successfully connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.connected.load(Ordering::Acquire) {
            return Ok(());
        }

        info!("Phonexia: Disconnecting");

        // Close WebSocket gracefully
        if let Some(ref ws_sink) = self.ws_sink {
            let mut sink = ws_sink.write().await;

            // Wait a bit for final results
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

            // Close the WebSocket
            if let Err(e) = sink.close().await {
                warn!(error = %e, "Phonexia: Error closing WebSocket");
            }
        }

        self.connected.store(false, Ordering::Release);
        self.ws_sink = None;
        *self.stream_id.write().await = None;

        info!("Phonexia: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.connected.load(Ordering::Acquire) && self.ws_sink.is_some()
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_ready() {
            return Err(STTError::ConnectionFailed(
                "Not connected to Phonexia server".to_string(),
            ));
        }

        let ws_sink = self
            .ws_sink
            .as_ref()
            .ok_or_else(|| STTError::ConnectionFailed("WebSocket not available".to_string()))?;

        // Send binary audio data (RAW s16le format)
        let mut sink = ws_sink.write().await;
        sink.send(Message::Binary(bytes::Bytes::from(audio_data.to_vec())))
            .await
            .map_err(|e| STTError::NetworkError(format!("Failed to send audio: {}", e)))?;

        // Update stats
        self.stats.write().await.total_audio_bytes += audio_data.len() as u64;

        trace!(bytes = audio_data.len(), "Phonexia: Sent audio chunk");
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
        let new_phonexia_config = PhonexiaSTTConfig::from_base(&config)?;

        // If connected, need to reconnect with new config
        if self.is_ready() {
            self.disconnect().await?;
            self.config = config;
            self.phonexia_config = new_phonexia_config;
            self.connect().await?;
        } else {
            self.config = config;
            self.phonexia_config = new_phonexia_config;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Phonexia Speech-to-Text (On-Premises WebSocket)"
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::future::Future;
    use std::pin::Pin;

    struct PhonexiaAllowUnverifiedEnvGuard(Option<String>);

    impl Drop for PhonexiaAllowUnverifiedEnvGuard {
        fn drop(&mut self) {
            match self.0.as_deref() {
                Some(value) => unsafe {
                    std::env::set_var(WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV, value)
                },
                None => unsafe { std::env::remove_var(WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV) },
            }
        }
    }

    fn set_phonexia_allow_unverified_env(value: Option<&str>) -> PhonexiaAllowUnverifiedEnvGuard {
        let previous = std::env::var(WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV).ok();
        match value {
            Some(value) => unsafe { std::env::set_var(WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV, value) },
            None => unsafe { std::env::remove_var(WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV) },
        }
        PhonexiaAllowUnverifiedEnvGuard(previous)
    }

    fn create_test_config() -> STTConfig {
        STTConfig {
            provider: "phonexia".to_string(),
            api_key: "https://test-phonexia.example.com".to_string(),
            language: "en-US".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "s16le".to_string(),
            model: "one_best".to_string(),
        }
    }

    #[test]
    fn test_phonexia_stt_new_success() {
        let config = create_test_config();
        let result = PhonexiaSTT::new(config);
        assert!(result.is_ok());
    }

    // Phonexia is fail-closed by default: its WS protocol is unverified against the real
    // Phonexia gRPC/REST API, so connect() must refuse with a clear error unless explicitly
    // opted in. (Prevents shipping a fabricated-protocol "success".)
    #[tokio::test]
    #[serial]
    async fn test_phonexia_connect_fails_closed_by_default() {
        let _guard = set_phonexia_allow_unverified_env(None);

        let mut stt = PhonexiaSTT::new(create_test_config()).unwrap();
        let err = stt
            .connect()
            .await
            .expect_err("must fail closed by default");
        assert!(
            err.to_string()
                .contains("not validated against the real Phonexia"),
            "expected fail-closed message, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_phonexia_connect_explicit_false_fails_closed() {
        let _guard = set_phonexia_allow_unverified_env(Some("0"));

        let mut stt = PhonexiaSTT::new(create_test_config()).unwrap();
        let err = stt
            .connect()
            .await
            .expect_err("explicit false must keep Phonexia disabled");
        assert!(
            err.to_string()
                .contains("not validated against the real Phonexia"),
            "expected fail-closed message, got: {err}"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_phonexia_connect_malformed_allow_override_rejected() {
        let _guard = set_phonexia_allow_unverified_env(Some("sure"));

        let mut stt = PhonexiaSTT::new(create_test_config()).unwrap();
        let err = stt
            .connect()
            .await
            .expect_err("malformed explicit override must fail config");
        assert!(
            err.to_string().contains(WAAV_PHONEXIA_ALLOW_UNVERIFIED_ENV),
            "error should name env var: {err}"
        );
    }

    #[test]
    #[serial]
    fn test_phonexia_allow_unverified_accepts_explicit_true() {
        let _guard = set_phonexia_allow_unverified_env(Some("yes"));

        assert!(phonexia_allow_unverified_from_env().unwrap());
    }

    #[test]
    fn test_phonexia_stt_new_empty_server_url() {
        let mut config = create_test_config();
        config.api_key = String::new();

        let result = PhonexiaSTT::new(config);
        assert!(result.is_err());
        if let Err(STTError::ConfigurationError(msg)) = result {
            assert!(msg.contains("server URL"));
        }
    }

    #[test]
    fn test_phonexia_stt_from_phonexia_config() {
        let config = PhonexiaSTTConfig::new("https://test-phonexia.example.com")
            .with_language("cs")
            .with_basic_auth("admin", "password");

        let result = PhonexiaSTT::from_phonexia_config(config);
        assert!(result.is_ok());
    }

    // W1 keystone: advanced features Phonexia can express (word_timestamps -> enable_timestamps,
    // keyterms -> preferred_phrases) survive through the provider-struct `new_standard` method to
    // the provider-specific config — proving the standardized path is honored end-to-end.
    #[test]
    fn test_phonexia_new_standard_unlocks_advanced_features() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "phonexia".into(),
                api_key: "https://test-phonexia.example.com".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Phonexia".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let stt = PhonexiaSTT::new_standard(&std).expect("new_standard must succeed");
        assert!(stt.phonexia_config.enable_timestamps); // word_timestamps
        assert_eq!(
            stt.phonexia_config.preferred_phrases,
            vec!["WaaV".to_string(), "Phonexia".to_string()]
        ); // keyterms

        // Missing server URL (carried in api_key) is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig {
            provider: "phonexia".into(),
            api_key: String::new(),
            ..Default::default()
        });
        assert!(PhonexiaSTT::new_standard(&bad).is_err());
    }

    #[test]
    fn test_phonexia_stt_not_connected_initially() {
        let config = create_test_config();
        let stt = PhonexiaSTT::new(config).unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_phonexia_stt_get_config() {
        let config = create_test_config();
        let stt = PhonexiaSTT::new(config.clone()).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, config.api_key);
        assert_eq!(stored_config.language, config.language);
        assert_eq!(stored_config.sample_rate, config.sample_rate);
    }

    #[test]
    fn test_phonexia_stt_get_provider_info() {
        let config = create_test_config();
        let stt = PhonexiaSTT::new(config).unwrap();

        let info = stt.get_provider_info();
        assert!(info.contains("Phonexia"));
        assert!(info.contains("On-Premises"));
    }

    #[tokio::test]
    async fn test_phonexia_stt_send_audio_not_connected() {
        let config = create_test_config();
        let mut stt = PhonexiaSTT::new(config).unwrap();

        let audio_data = Bytes::from(vec![0u8; 1024]);
        let result = stt.send_audio(audio_data).await;

        assert!(result.is_err());
        if let Err(STTError::ConnectionFailed(msg)) = result {
            assert!(msg.contains("Not connected"));
        }
    }

    #[tokio::test]
    async fn test_phonexia_stt_on_result() {
        let config = create_test_config();
        let mut stt = PhonexiaSTT::new(config).unwrap();

        let callback: STTResultCallback = Arc::new(|result: STTResult| {
            Box::pin(async move {
                println!("Received: {:?}", result);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_phonexia_stt_on_error() {
        let config = create_test_config();
        let mut stt = PhonexiaSTT::new(config).unwrap();

        let callback: STTErrorCallback = Arc::new(|error: STTError| {
            Box::pin(async move {
                println!("Error: {:?}", error);
            }) as Pin<Box<dyn Future<Output = ()> + Send>>
        });

        let result = stt.on_error(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_phonexia_stt_get_stream_id_none() {
        let config = create_test_config();
        let stt = PhonexiaSTT::new(config).unwrap();

        let stream_id = stt.get_stream_id().await;
        assert!(stream_id.is_none());
    }

    #[tokio::test]
    async fn test_phonexia_stt_get_stats_default() {
        let config = create_test_config();
        let stt = PhonexiaSTT::new(config).unwrap();

        let stats = stt.get_stats().await;
        assert_eq!(stats.total_audio_bytes, 0);
        assert_eq!(stats.results_count, 0);
        assert_eq!(stats.final_results_count, 0);
    }

    #[tokio::test]
    async fn test_phonexia_stt_disconnect_when_not_connected() {
        let config = create_test_config();
        let mut stt = PhonexiaSTT::new(config).unwrap();

        // Should not error when disconnecting a non-connected client
        let result = stt.disconnect().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_phonexia_stt_update_config() {
        let config = create_test_config();
        let mut stt = PhonexiaSTT::new(config).unwrap();

        let mut new_config = create_test_config();
        new_config.language = "cs".to_string();
        new_config.sample_rate = 44100;

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.language, "cs");
        assert_eq!(stored_config.sample_rate, 44100);
    }

    #[tokio::test]
    async fn test_handle_message_result() {
        let result_callback: Arc<RwLock<Option<STTResultCallback>>> = Arc::new(RwLock::new(None));
        let error_callback: Arc<RwLock<Option<STTErrorCallback>>> = Arc::new(RwLock::new(None));
        let stream_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let stats: Arc<RwLock<STTStats>> = Arc::new(RwLock::new(STTStats::default()));

        let message = r#"{
            "is_last": true,
            "segments": [{
                "words": [{"text": "hello", "confidence": 0.95}]
            }]
        }"#;

        PhonexiaSTT::handle_message(
            message,
            &result_callback,
            &error_callback,
            &stream_id,
            &stats,
        )
        .await;

        let stats_val = stats.read().await;
        assert_eq!(stats_val.results_count, 1);
        assert_eq!(stats_val.final_results_count, 1);
    }

    #[tokio::test]
    async fn test_handle_message_status_with_stream_id() {
        let result_callback: Arc<RwLock<Option<STTResultCallback>>> = Arc::new(RwLock::new(None));
        let error_callback: Arc<RwLock<Option<STTErrorCallback>>> = Arc::new(RwLock::new(None));
        let stream_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let stats: Arc<RwLock<STTStats>> = Arc::new(RwLock::new(STTStats::default()));

        let message = r#"{
            "status": "connected",
            "stream_id": "test-stream-123"
        }"#;

        PhonexiaSTT::handle_message(
            message,
            &result_callback,
            &error_callback,
            &stream_id,
            &stats,
        )
        .await;

        let id = stream_id.read().await;
        assert_eq!(id.as_deref(), Some("test-stream-123"));
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
        let stream_id: Arc<RwLock<Option<String>>> = Arc::new(RwLock::new(None));
        let stats: Arc<RwLock<STTStats>> = Arc::new(RwLock::new(STTStats::default()));

        let message = r#"{
            "is_last": true,
            "segments": [{
                "words": [{"text": "hello", "confidence": 0.95}]
            }]
        }"#;

        PhonexiaSTT::handle_message(
            message,
            &result_callback,
            &error_callback,
            &stream_id,
            &stats,
        )
        .await;

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
