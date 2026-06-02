//! Speechmatics STT WebSocket Client
//!
//! Real-time speech-to-text using Speechmatics WebSocket streaming API.

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info};

use super::config::SpeechmaticsSTTConfig;
use super::messages::{
    AddPartialTranscriptMessage, AddTranscriptMessage, AudioFormat, EndOfStreamMessage,
    ErrorMessage, StartRecognitionMessage, TranscriptionConfig,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Speechmatics STT WebSocket client
pub struct SpeechmaticsSTT {
    /// Speechmatics-specific configuration
    config: SpeechmaticsSTTConfig,
    /// Base STT configuration
    base_config: Option<STTConfig>,
    /// WebSocket connection
    ws: Arc<RwLock<Option<WsStream>>>,
    /// Connection state
    is_connected: Arc<AtomicBool>,
    /// Session started flag
    is_session_started: Arc<AtomicBool>,
    /// Audio sequence number
    seq_no: Arc<AtomicU64>,
    /// Result callback
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,
    /// Error callback
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
}

impl SpeechmaticsSTT {
    /// W1 keystone — construct directly from the standardized config so Speechmatics' rich feature
    /// surface (diarization, interim partials, entity detection, custom vocabulary) is honored
    /// END-TO-END. The flat `BaseSTT::new` path uses `from_base`, which hardcodes those off; this
    /// is the reachable standardized path.
    pub fn new_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let speechmatics_config = SpeechmaticsSTTConfig::from_standard(std)?;
        speechmatics_config.validate()?;

        info!(
            "Creating Speechmatics STT client (standardized): region={}, language={}, operating_point={}",
            speechmatics_config.region,
            speechmatics_config.language,
            speechmatics_config.operating_point
        );

        Ok(Self {
            config: speechmatics_config,
            base_config: Some(std.base.clone()),
            ws: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(AtomicBool::new(false)),
            is_session_started: Arc::new(AtomicBool::new(false)),
            seq_no: Arc::new(AtomicU64::new(0)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        })
    }

    /// Build the WebSocket URL with authentication
    fn build_ws_url(&self) -> String {
        self.config.ws_url().to_string()
    }

    /// Build the StartRecognition message
    fn build_start_recognition(&self) -> StartRecognitionMessage {
        let audio_format = AudioFormat::raw(self.config.encoding, self.config.sample_rate);

        let mut transcription_config = TranscriptionConfig::new(self.config.language)
            .with_operating_point(self.config.operating_point)
            .with_partials(self.config.enable_partials)
            .with_max_delay(self.config.max_delay);

        if self.config.enable_diarization {
            transcription_config = transcription_config.with_diarization(self.config.max_speakers);
        }

        if !self.config.additional_vocab.is_empty() {
            transcription_config =
                transcription_config.with_vocab(self.config.additional_vocab.clone());
        }

        StartRecognitionMessage::with_config(audio_format, transcription_config)
    }

    /// Start the message receiving loop
    fn start_receive_loop(
        ws: Arc<RwLock<Option<WsStream>>>,
        is_connected: Arc<AtomicBool>,
        is_session_started: Arc<AtomicBool>,
        result_callback: Arc<RwLock<Option<STTResultCallback>>>,
        error_callback: Arc<RwLock<Option<STTErrorCallback>>>,
    ) {
        tokio::spawn(async move {
            loop {
                let msg = {
                    let mut ws_guard = ws.write().await;
                    if let Some(ws_stream) = ws_guard.as_mut() {
                        ws_stream.next().await
                    } else {
                        break;
                    }
                };

                match msg {
                    Some(Ok(Message::Text(text))) => {
                        // Parse and handle the message inline
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                            let message_type =
                                value.get("message").and_then(|v| v.as_str()).unwrap_or("");

                            match message_type {
                                "RecognitionStarted" => {
                                    info!("Speechmatics session started");
                                    is_session_started.store(true, Ordering::SeqCst);
                                }
                                "AddPartialTranscript" => {
                                    if let Ok(msg) =
                                        serde_json::from_str::<AddPartialTranscriptMessage>(&text)
                                    {
                                        let transcript = msg.transcript();
                                        if !transcript.is_empty()
                                            && let Some(callback) =
                                                result_callback.read().await.as_ref()
                                            {
                                                let result = STTResult::new(
                                                    transcript.to_string(),
                                                    false,
                                                    false,
                                                    0.0,
                                                );
                                                callback(result).await;
                                            }
                                    }
                                }
                                "AddTranscript" => {
                                    if let Ok(msg) =
                                        serde_json::from_str::<AddTranscriptMessage>(&text)
                                    {
                                        let transcript = msg.transcript();
                                        if !transcript.is_empty() {
                                            let words: Vec<_> = msg.words().collect();
                                            let confidence = if !words.is_empty() {
                                                words
                                                    .iter()
                                                    .map(|w| w.confidence() as f32)
                                                    .sum::<f32>()
                                                    / words.len() as f32
                                            } else {
                                                0.9
                                            };

                                            if let Some(callback) =
                                                result_callback.read().await.as_ref()
                                            {
                                                let result = STTResult::new(
                                                    transcript.to_string(),
                                                    true,
                                                    false,
                                                    confidence,
                                                );
                                                callback(result).await;
                                            }
                                        }
                                    }
                                }
                                "EndOfTranscript" => {
                                    info!("Speechmatics session ended");
                                    is_session_started.store(false, Ordering::SeqCst);
                                }
                                "EndOfUtterance" => {
                                    if let Some(callback) = result_callback.read().await.as_ref() {
                                        let result = STTResult::new(String::new(), true, true, 1.0);
                                        callback(result).await;
                                    }
                                }
                                "Error" => {
                                    if let Ok(msg) = serde_json::from_str::<ErrorMessage>(&text) {
                                        error!("Speechmatics error: {}", msg);
                                        if let Some(callback) = error_callback.read().await.as_ref()
                                        {
                                            callback(STTError::ProviderError(msg.to_string()))
                                                .await;
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        debug!("Received unexpected binary message from server");
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let mut ws_guard = ws.write().await;
                        if let Some(ws_stream) = ws_guard.as_mut() {
                            let _ = ws_stream.send(Message::Pong(data)).await;
                        }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) => {
                        info!("WebSocket closed by server");
                        is_connected.store(false, Ordering::SeqCst);
                        is_session_started.store(false, Ordering::SeqCst);
                        break;
                    }
                    Some(Err(e)) => {
                        error!("WebSocket error: {}", e);
                        is_connected.store(false, Ordering::SeqCst);
                        is_session_started.store(false, Ordering::SeqCst);
                        if let Some(callback) = error_callback.read().await.as_ref() {
                            callback(STTError::ConnectionFailed(e.to_string())).await;
                        }
                        break;
                    }
                    None => {
                        info!("WebSocket stream ended");
                        is_connected.store(false, Ordering::SeqCst);
                        is_session_started.store(false, Ordering::SeqCst);
                        break;
                    }
                    _ => {}
                }
            }
        });
    }
}

#[async_trait]
impl BaseSTT for SpeechmaticsSTT {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        let speechmatics_config = SpeechmaticsSTTConfig::from_base(&config)?;
        speechmatics_config.validate()?;

        info!(
            "Creating Speechmatics STT client: region={}, language={}, operating_point={}",
            speechmatics_config.region,
            speechmatics_config.language,
            speechmatics_config.operating_point
        );

        Ok(Self {
            config: speechmatics_config,
            base_config: Some(config),
            ws: Arc::new(RwLock::new(None)),
            is_connected: Arc::new(AtomicBool::new(false)),
            is_session_started: Arc::new(AtomicBool::new(false)),
            seq_no: Arc::new(AtomicU64::new(0)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
        })
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.is_connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        let ws_url = self.build_ws_url();
        info!("Connecting to Speechmatics: {}", ws_url);

        // Build request with authorization header
        let request = http::Request::builder()
            .uri(&ws_url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Sec-WebSocket-Protocol", "json")
            .header("Host", "eu.rt.speechmatics.com")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to build request: {}", e)))?;

        let (ws_stream, _) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| STTError::ConnectionFailed(format!("WebSocket connect failed: {}", e)))?;

        *self.ws.write().await = Some(ws_stream);
        self.is_connected.store(true, Ordering::SeqCst);
        self.seq_no.store(0, Ordering::SeqCst);

        // Send StartRecognition message
        let start_msg = self.build_start_recognition();
        let json = serde_json::to_string(&start_msg)
            .map_err(|e| STTError::ProviderError(format!("Failed to serialize: {}", e)))?;

        {
            let mut ws_guard = self.ws.write().await;
            if let Some(ws) = ws_guard.as_mut() {
                ws.send(Message::Text(json.into())).await.map_err(|e| {
                    STTError::ConnectionFailed(format!("Failed to send start: {}", e))
                })?;
            }
        }

        info!("Speechmatics connected, sent StartRecognition");

        // Start receive loop
        Self::start_receive_loop(
            self.ws.clone(),
            self.is_connected.clone(),
            self.is_session_started.clone(),
            self.result_callback.clone(),
            self.error_callback.clone(),
        );

        // Wait briefly for RecognitionStarted
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        if !self.is_connected.load(Ordering::SeqCst) {
            return Ok(());
        }

        // Send EndOfStream
        let end_msg = EndOfStreamMessage::new(self.seq_no.load(Ordering::SeqCst));
        let json = serde_json::to_string(&end_msg)
            .map_err(|e| STTError::ProviderError(format!("Failed to serialize: {}", e)))?;

        {
            let mut ws_guard = self.ws.write().await;
            if let Some(ws) = ws_guard.as_mut() {
                let _ = ws.send(Message::Text(json.into())).await;
                let _ = ws.close(None).await;
            }
        }

        *self.ws.write().await = None;
        self.is_connected.store(false, Ordering::SeqCst);
        self.is_session_started.store(false, Ordering::SeqCst);

        info!("Speechmatics disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst) && self.is_session_started.load(Ordering::SeqCst)
    }

    async fn send_audio(&mut self, audio_data: Bytes) -> Result<(), STTError> {
        if !self.is_connected.load(Ordering::SeqCst) {
            return Err(STTError::ConnectionFailed("Not connected".to_string()));
        }

        // Send binary audio data
        let mut ws_guard = self.ws.write().await;
        if let Some(ws) = ws_guard.as_mut() {
            ws.send(Message::Binary(audio_data.to_vec().into()))
                .await
                .map_err(|e| STTError::NetworkError(format!("Failed to send audio: {}", e)))?;
        }

        self.seq_no.fetch_add(1, Ordering::SeqCst);
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
        self.base_config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let was_connected = self.is_connected.load(Ordering::SeqCst);

        if was_connected {
            self.disconnect().await?;
        }

        self.config = SpeechmaticsSTTConfig::from_base(&config)?;
        self.config.validate()?;
        self.base_config = Some(config);

        if was_connected {
            self.connect().await?;
        }

        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Speechmatics Real-time STT (55+ languages, WebSocket streaming)"
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_speechmatics_stt_creation() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            sample_rate: 16000,
            encoding: "pcm_s16le".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config);
        assert!(stt.is_ok());

        let stt = stt.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_speechmatics_stt_requires_api_key() {
        let config = STTConfig::default();
        let result = SpeechmaticsSTT::new(config);
        assert!(result.is_err());
    }

    // W1 keystone: Speechmatics' rich advanced features (diarization, interim partials, entity
    // detection, custom vocabulary) must survive THROUGH `new_standard` into the provider's
    // config — not just the config-level `from_standard`. The flat `new` path leaves them off.
    #[test]
    fn test_new_standard_unlocks_advanced_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "speechmatics".into(),
                api_key: "test-api-key".into(),
                language: "en".into(),
                sample_rate: 16000,
                encoding: "pcm_s16le".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                interim_results: Some(true),
                entity_detection: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Speechmatics".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let stt = SpeechmaticsSTT::new_standard(&std).unwrap();
        assert!(stt.config.enable_diarization);
        assert!(stt.config.enable_partials);
        assert!(stt.config.enable_entities);
        assert_eq!(stt.config.additional_vocab, vec!["WaaV", "Speechmatics"]);

        // Missing api_key is rejected through the standardized path too.
        let bad = StandardSTTConfig::from_base(STTConfig::default());
        assert!(SpeechmaticsSTT::new_standard(&bad).is_err());
    }

    #[test]
    fn test_build_start_recognition() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "fr".to_string(),
            sample_rate: 44100,
            encoding: "pcm_f32le".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config).unwrap();
        let msg = stt.build_start_recognition();

        assert_eq!(msg.message, "StartRecognition");
        assert_eq!(msg.audio_format.format_type, "raw");
        assert_eq!(msg.audio_format.sample_rate, Some(44100));
        assert_eq!(msg.transcription_config.language, "fr");
    }

    #[test]
    fn test_build_ws_url() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config).unwrap();
        let url = stt.build_ws_url();

        assert!(url.starts_with("wss://"));
        assert!(url.contains("speechmatics.com"));
    }

    #[tokio::test]
    async fn test_speechmatics_stt_send_audio_not_connected() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let mut stt = SpeechmaticsSTT::new(config).unwrap();
        let result = stt.send_audio(Bytes::from_static(b"test")).await;

        assert!(result.is_err());
    }

    #[test]
    fn test_get_provider_info() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Speechmatics"));
    }

    #[test]
    fn test_get_config() {
        let config = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let stt = SpeechmaticsSTT::new(config.clone()).unwrap();
        let retrieved_config = stt.get_config();

        assert!(retrieved_config.is_some());
        assert_eq!(retrieved_config.unwrap().language, "en");
    }
}
