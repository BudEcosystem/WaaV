//! Gladia STT Client Implementation
//!
//! Implements the BaseSTT trait for Gladia real-time speech-to-text.

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

use super::config::GladiaSTTConfig;
use super::messages::{
    AudioChunkMessage, InitSessionRequest, InitSessionResponse, ServerMessage, StopRecordingMessage,
};

// =============================================================================
// Type Aliases
// =============================================================================

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = futures_util::stream::SplitSink<WsStream, Message>;

// =============================================================================
// GladiaSTT Implementation
// =============================================================================

/// Gladia STT client implementing BaseSTT trait
pub struct GladiaSTT {
    /// Provider-specific configuration
    gladia_config: GladiaSTTConfig,
    /// Base STT configuration (stored for get_config)
    base_config: Option<STTConfig>,
    /// Current connection state
    state: Arc<RwLock<STTConnectionState>>,
    /// WebSocket sink for sending messages
    ws_sink: Arc<RwLock<Option<WsSink>>>,
    /// Session ID from initialization
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
    /// HTTP client for session initialization
    http_client: reqwest::Client,
}

impl GladiaSTT {
    /// W1 keystone — construct directly from the standardized config so Gladia's nested feature
    /// surface (interim results, word timestamps, custom vocabulary/keyterms, named-entity
    /// recognition, automatic language detection) is honored END-TO-END. The flat `BaseSTT::new`
    /// path cannot reach those knobs; this is the reachable standardized path mirroring
    /// `DeepgramSTT::new_standard`. The api_key is resolved by `from_standard`/`from_base` (config
    /// or `GLADIA_API_KEY` env var) and validated by `with_config`.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let gladia_config = GladiaSTTConfig::from_standard(std)?;
        Self::with_config(gladia_config)
    }

    /// Create a new Gladia STT client from provider-specific config
    pub fn with_config(config: GladiaSTTConfig) -> Result<Self, STTError> {
        // Validate configuration
        config.validate()?;

        Ok(Self {
            gladia_config: config,
            base_config: None,
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sink: Arc::new(RwLock::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            http_client: reqwest::Client::new(),
        })
    }

    /// Build the `POST /v2/live` session-init request body from the provider config.
    ///
    /// Extracted from `init_session` so the exact JSON that hits the wire is unit-testable
    /// (the recurring "feature set on the config struct but never serialized" bug class).
    pub(super) fn build_init_request(&self) -> InitSessionRequest {
        // Omit post_processing entirely when no post-processing feature is requested, so the
        // default body stays byte-for-byte identical to before this feature was added.
        let post_processing = if self.gladia_config.post_processing.is_empty() {
            None
        } else {
            Some(self.gladia_config.post_processing.clone())
        };
        InitSessionRequest {
            encoding: self.gladia_config.encoding.as_str().to_string(),
            bit_depth: self.gladia_config.bit_depth.value(),
            sample_rate: self.gladia_config.sample_rate,
            channels: self.gladia_config.channels,
            model: Some(self.gladia_config.model.clone()),
            endpointing: Some(self.gladia_config.endpointing),
            maximum_duration_without_endpointing: Some(
                self.gladia_config.maximum_duration_without_endpointing,
            ),
            language_config: Some(self.gladia_config.language_config.clone()),
            pre_processing: Some(self.gladia_config.pre_processing.clone()),
            realtime_processing: Some(self.gladia_config.realtime_processing.clone()),
            post_processing,
            messages_config: Some(self.gladia_config.messages_config.clone()),
            custom_metadata: self.gladia_config.custom_metadata.clone(),
        }
    }

    /// Initialize a session via REST API
    async fn init_session(&self) -> Result<InitSessionResponse, STTError> {
        let url = self.gladia_config.api_url();
        debug!("Initializing Gladia session at {}", url);

        let request_body = self.build_init_request();

        trace!("Session init request: {:?}", request_body);

        let response = self
            .http_client
            .post(&url)
            .header("X-Gladia-Key", &self.gladia_config.api_key)
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| STTError::ConnectionFailed(format!("HTTP request failed: {}", e)))?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(match status.as_u16() {
                401 => STTError::AuthenticationFailed(format!("Invalid API key: {}", error_text)),
                400 | 422 => {
                    STTError::ConfigurationError(format!("Invalid request: {}", error_text))
                }
                _ => STTError::ConnectionFailed(format!(
                    "Session init failed ({}): {}",
                    status, error_text
                )),
            });
        }

        let init_response: InitSessionResponse = response
            .json()
            .await
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to parse response: {}", e)))?;

        debug!(
            "Session initialized: id={}, ws_url={}",
            init_response.id, init_response.url
        );

        Ok(init_response)
    }

    /// Spawn the WebSocket message receiver task
    fn spawn_receiver_task(&self, mut ws_stream: futures_util::stream::SplitStream<WsStream>) {
        let on_result = self.on_result.clone();
        let on_error = self.on_error.clone();
        let state = self.state.clone();
        let is_ready = self.is_ready.clone();
        let stats = self.stats.clone();

        tokio::spawn(async move {
            while let Some(message_result) = ws_stream.next().await {
                match message_result {
                    Ok(Message::Text(text)) => {
                        trace!("Received text message: {}", text);
                        match ServerMessage::from_json(&text) {
                            Ok(ServerMessage::Transcript(transcript)) => {
                                // Create STTResult (using only available fields)
                                let result = STTResult::new(
                                    transcript.data.utterance.text.clone(),
                                    transcript.data.is_final,
                                    transcript.data.is_final, // Use is_final for is_speech_final
                                    transcript.data.utterance.confidence as f32,
                                );

                                // Update stats
                                {
                                    let mut s = stats.write().await;
                                    s.update_with_result(&result);
                                }

                                // Invoke callback
                                if let Some(callback) = on_result.read().await.as_ref() {
                                    callback(result).await;
                                }
                            }
                            Ok(ServerMessage::Error(err)) => {
                                error!("Gladia error: {}", err);
                                if let Some(callback) = on_error.read().await.as_ref() {
                                    callback(STTError::ProviderError(err.message)).await;
                                }
                            }
                            Ok(ServerMessage::Unknown(msg_type)) => {
                                debug!("Unknown message type: {}", msg_type);
                            }
                            Err(e) => {
                                warn!("Failed to parse message: {}", e);
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
            debug!("Receiver task ended");
        });
    }
}

#[async_trait::async_trait]
impl BaseSTT for GladiaSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        // Check for API key
        if config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Gladia API key is required. Set api_key or GLADIA_API_KEY env var".to_string(),
            ));
        }

        let gladia_config = GladiaSTTConfig::from_base(&config)?;

        Ok(Self {
            gladia_config,
            base_config: Some(config),
            state: Arc::new(RwLock::new(STTConnectionState::Disconnected)),
            ws_sink: Arc::new(RwLock::new(None)),
            session_id: Arc::new(RwLock::new(None)),
            on_result: Arc::new(RwLock::new(None)),
            on_error: Arc::new(RwLock::new(None)),
            is_ready: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(RwLock::new(STTStats::default())),
            bytes_sent: Arc::new(AtomicU64::new(0)),
            http_client: reqwest::Client::new(),
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
        info!("Connecting to Gladia STT...");

        // Step 1: Initialize session via REST
        let init_response = self.init_session().await?;

        // Store session ID
        *self.session_id.write().await = Some(init_response.id.clone());

        // Step 2: Connect to WebSocket
        debug!("Connecting to WebSocket: {}", init_response.url);
        let (ws_stream, _) = connect_async(&init_response.url).await.map_err(|e| {
            STTError::ConnectionFailed(format!("WebSocket connection failed: {}", e))
        })?;

        // Split the stream
        let (sink, stream) = ws_stream.split();

        // Store the sink
        *self.ws_sink.write().await = Some(sink);

        // Spawn receiver task
        self.spawn_receiver_task(stream);

        // Update state
        *self.state.write().await = STTConnectionState::Connected;
        self.is_ready.store(true, Ordering::SeqCst);

        info!("Connected to Gladia STT (session: {})", init_response.id);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        info!("Disconnecting from Gladia STT...");

        // Send stop recording message
        if let Some(sink) = self.ws_sink.write().await.as_mut() {
            let stop_msg = StopRecordingMessage::new();
            if let Ok(json) = stop_msg.to_json() {
                let _ = sink.send(Message::Text(json.into())).await;
            }
            // Close the WebSocket
            let _ = sink.close().await;
        }

        // Clear state
        *self.ws_sink.write().await = None;
        *self.session_id.write().await = None;
        *self.state.write().await = STTConnectionState::Disconnected;
        self.is_ready.store(false, Ordering::SeqCst);

        info!("Disconnected from Gladia STT");
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

        // Send as JSON with base64 encoding (per Gladia docs)
        let chunk_msg = AudioChunkMessage::new(&audio_data);
        let json = chunk_msg
            .to_json()
            .map_err(|e| STTError::InvalidAudioFormat(e.to_string()))?;

        sink.send(Message::Text(json.into()))
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
        // Update the gladia config
        let new_gladia_config = GladiaSTTConfig::from_base(&config)?;
        self.gladia_config = new_gladia_config;
        self.base_config = Some(config);

        // If connected, we need to reconnect with the new config
        if self.is_ready() {
            self.disconnect().await?;
            self.connect().await?;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Gladia STT (solaria-1)"
    }
}

// =============================================================================
// Factory Function
// =============================================================================

/// Create a new Gladia STT provider from base configuration
pub fn create_gladia_stt(config: STTConfig) -> Result<Box<dyn BaseSTT>, STTError> {
    // Check for API key
    if config.api_key.is_empty() {
        return Err(STTError::AuthenticationFailed(
            "Gladia API key is required. Set api_key or GLADIA_API_KEY env var".to_string(),
        ));
    }

    let stt = GladiaSTT::new(config)?;
    Ok(Box::new(stt))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_gladia_config() -> GladiaSTTConfig {
        GladiaSTTConfig::new("test-api-key")
    }

    fn create_test_base_config() -> STTConfig {
        STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            provider: "gladia".to_string(),
            ..Default::default()
        }
    }

    // W1 keystone: advanced features set on the standardized config must survive through
    // `new_standard` into the nested Gladia provider config (previously dropped by the flat
    // factory).
    #[test]
    fn new_standard_propagates_advanced_features() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "gladia".into(),
                api_key: "test-api-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                word_timestamps: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Gladia".into()]),
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let stt = GladiaSTT::new_standard(&std).expect("new_standard should succeed");
        assert!(stt.gladia_config.realtime_processing.words_accurate_timestamps);
        assert_eq!(
            stt.gladia_config.realtime_processing.custom_vocabulary,
            vec!["WaaV", "Gladia"]
        );
    }

    // ===================== WIRE-LEVEL feature tests (session-init body) =====================
    //
    // These build the EXACT `POST /v2/live` JSON body via `build_init_request` (the same
    // builder `init_session` posts) and assert each newly-wired feature reaches the wire —
    // not merely the config struct. Construction goes through `new_standard`, the reachable
    // standardized path, so the assertion covers from_standard -> config -> body end-to-end.

    use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};

    fn std_with_extras(extras: serde_json::Value) -> StandardSTTConfig {
        StandardSTTConfig {
            base: STTConfig {
                provider: "gladia".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: Default::default(),
            extras: ProviderExtras(extras.as_object().unwrap().clone()),
        }
    }

    /// Build the session-init body JSON for a Gladia provider built from the given extras.
    fn init_body_json(extras: serde_json::Value) -> serde_json::Value {
        let stt = GladiaSTT::new_standard(&std_with_extras(extras)).expect("new_standard");
        serde_json::to_value(stt.build_init_request()).expect("serialize body")
    }

    #[test]
    fn wire_custom_spelling_dictionary_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "custom_spelling_dictionary": { "WaaV": ["wave", "wav"], "Gladia": ["gladiya"] }
        }));
        let rp = &body["realtime_processing"];
        assert_eq!(rp["custom_spelling"], true, "custom_spelling flag missing: {body}");
        assert_eq!(
            rp["custom_spelling_config"]["spelling_dictionary"]["WaaV"],
            serde_json::json!(["wave", "wav"]),
            "spelling_dictionary entry missing on wire: {body}"
        );
    }

    #[test]
    fn wire_custom_vocabulary_config_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "custom_vocabulary_config": {
                "vocabulary": [
                    { "value": "WaaV", "intensity": 0.8, "pronunciations": ["wave"], "language": "en" }
                ],
                "default_intensity": 0.5
            }
        }));
        let cvc = &body["realtime_processing"]["custom_vocabulary_config"];
        // f32 round-trips with widening, so compare numerics with a tolerance.
        let default_intensity = cvc["default_intensity"].as_f64().expect("default_intensity");
        assert!(
            (default_intensity - 0.5).abs() < 1e-6,
            "default_intensity missing on wire: {body}"
        );
        assert_eq!(cvc["vocabulary"][0]["value"], "WaaV");
        let intensity = cvc["vocabulary"][0]["intensity"].as_f64().expect("intensity");
        assert!((intensity - 0.8).abs() < 1e-6, "intensity missing on wire: {body}");
        assert_eq!(cvc["vocabulary"][0]["pronunciations"], serde_json::json!(["wave"]));
        assert_eq!(cvc["vocabulary"][0]["language"], "en");
    }

    #[test]
    fn wire_translation_config_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "translation_config": {
                "model": "enhanced",
                "match_original_utterances": true,
                "lipsync": true,
                "context_adaptation": true,
                "context": "medical",
                "informal": false
            }
        }));
        let tc = &body["realtime_processing"]["translation_config"];
        assert_eq!(tc["model"], "enhanced", "translation model missing: {body}");
        assert_eq!(tc["match_original_utterances"], true);
        assert_eq!(tc["lipsync"], true);
        assert_eq!(tc["context_adaptation"], true);
        assert_eq!(tc["context"], "medical");
        assert_eq!(tc["informal"], false);
    }

    #[test]
    fn wire_summarization_reaches_body() {
        let body = init_body_json(serde_json::json!({
            "summarization": true,
            "summarization_config": { "type": "bullet_points" }
        }));
        let pp = &body["post_processing"];
        assert_eq!(pp["summarization"], true, "summarization flag missing: {body}");
        assert_eq!(
            pp["summarization_config"]["type"], "bullet_points",
            "summarization type missing on wire: {body}"
        );
    }

    #[test]
    fn wire_chapterization_reaches_body() {
        let body = init_body_json(serde_json::json!({ "chapterization": true }));
        assert_eq!(
            body["post_processing"]["chapterization"], true,
            "chapterization missing on wire: {body}"
        );
    }

    #[test]
    fn wire_message_toggles_reach_body() {
        let body = init_body_json(serde_json::json!({
            "receive_acknowledgments": true,
            "receive_errors": true,
            "receive_lifecycle_events": true
        }));
        let mc = &body["messages_config"];
        assert_eq!(mc["receive_acknowledgments"], true, "acks missing: {body}");
        assert_eq!(mc["receive_errors"], true, "errors toggle missing: {body}");
        assert_eq!(
            mc["receive_lifecycle_events"], true,
            "lifecycle toggle missing: {body}"
        );
    }

    #[test]
    fn wire_defaults_omit_post_processing_and_leave_toggles_off() {
        // No extras: post_processing must be omitted entirely and message toggles default off.
        let body = init_body_json(serde_json::json!({}));
        assert!(
            body.get("post_processing").is_none() || body["post_processing"].is_null(),
            "post_processing must be omitted by default: {body}"
        );
        let mc = &body["messages_config"];
        assert_eq!(mc["receive_acknowledgments"], false);
        assert_eq!(mc["receive_errors"], false);
        assert_eq!(mc["receive_lifecycle_events"], false);
        let rp = &body["realtime_processing"];
        assert_eq!(rp["custom_spelling"], false);
        assert!(rp.get("custom_spelling_config").is_none() || rp["custom_spelling_config"].is_null());
        assert!(rp.get("translation_config").is_none() || rp["translation_config"].is_null());
        assert!(
            rp.get("custom_vocabulary_config").is_none() || rp["custom_vocabulary_config"].is_null()
        );
    }

    #[test]
    fn test_gladia_stt_with_config() {
        let config = create_test_gladia_config();
        let stt = GladiaSTT::with_config(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert_eq!(stt.get_provider_info(), "Gladia STT (solaria-1)");
    }

    #[test]
    fn test_gladia_stt_with_empty_api_key() {
        let config = GladiaSTTConfig::default();
        let stt = GladiaSTT::with_config(config);
        assert!(stt.is_err());
    }

    #[test]
    fn test_gladia_stt_new() {
        let config = create_test_base_config();
        let stt = GladiaSTT::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[test]
    fn test_gladia_stt_new_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };
        let stt = GladiaSTT::new(config);
        assert!(stt.is_err());
        if let Err(err) = stt {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    #[test]
    fn test_create_gladia_stt_factory() {
        let config = create_test_base_config();
        let result = create_gladia_stt(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert_eq!(stt.get_provider_info(), "Gladia STT (solaria-1)");
    }

    #[test]
    fn test_create_gladia_stt_factory_empty_api_key() {
        let config = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = create_gladia_stt(config);
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::AuthenticationFailed(_)));
        }
    }

    #[tokio::test]
    async fn test_gladia_stt_initial_state() {
        let config = create_test_base_config();
        let stt = GladiaSTT::new(config).unwrap();

        assert!(!stt.is_ready());
        assert!(stt.get_config().is_some());
    }

    #[tokio::test]
    async fn test_gladia_stt_send_audio_not_connected() {
        let config = create_test_base_config();
        let mut stt = GladiaSTT::new(config).unwrap();

        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_err());
        if let Err(err) = result {
            assert!(matches!(err, STTError::ConnectionFailed(_)));
        }
    }

    #[tokio::test]
    async fn test_gladia_stt_callbacks() {
        use std::sync::atomic::AtomicUsize;

        let config = create_test_base_config();
        let mut stt = GladiaSTT::new(config).unwrap();

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
    fn test_gladia_stt_get_config() {
        let config = create_test_base_config();
        let stt = GladiaSTT::new(config.clone()).unwrap();

        let stored_config = stt.get_config().unwrap();
        assert_eq!(stored_config.api_key, "test-api-key");
        assert_eq!(stored_config.language, "en");
        assert_eq!(stored_config.sample_rate, 16000);
    }
}
