//! Prosa.ai STT client implementation.
//!
//! This module implements the BaseSTT trait for Prosa.ai's
//! Speech-to-Text service.

use super::config::{
    MIN_AUDIO_BUFFER_SIZE, PROSA_STT_BASE_URL, PROSA_STT_WS_ENDPOINT, ProsaAudioFormat,
    ProsaSttAudioConfig, ProsaSttConfig, ProsaSttModel, ProsaSttRequest, ProsaSttRequestConfig,
    ProsaSttRequestData, ProsaSttResponse, ProsaSttStreamConfig, ProsaSttWsMessage,
};
use crate::core::stt::base::{
    BaseSTT, STTConfig, STTError, STTErrorCallback, STTResult, STTResultCallback,
};

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info};

// =============================================================================
// Prosa STT Client
// =============================================================================

/// Prosa.ai STT client.
pub struct ProsaStt {
    /// Provider-specific configuration.
    config: ProsaSttConfig,

    /// Original base configuration.
    base_config: Option<STTConfig>,

    /// HTTP client for REST API.
    http_client: Client,

    /// WebSocket write sink for streaming.
    ws_sink: Arc<RwLock<Option<WsSink>>>,

    /// Audio buffer for batching.
    audio_buffer: Arc<RwLock<Vec<u8>>>,

    /// Connection state flag.
    is_connected: Arc<AtomicBool>,

    /// Result callback.
    result_callback: Arc<RwLock<Option<STTResultCallback>>>,

    /// Error callback.
    error_callback: Arc<RwLock<Option<STTErrorCallback>>>,

    /// Current job ID.
    current_job_id: Arc<RwLock<Option<String>>>,
}

/// Type alias for WebSocket sink.
type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

impl ProsaStt {
    /// Create a new Prosa.ai STT client.
    pub fn new(config: STTConfig) -> Result<Self, STTError> {
        let prosa_config = ProsaSttConfig::from_base(&config)?;

        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(
                prosa_config.request_timeout_secs,
            ))
            .build()
            .map_err(|e| {
                STTError::ConnectionFailed(format!("Failed to create HTTP client: {}", e))
            })?;

        Ok(Self {
            config: prosa_config,
            base_config: Some(config),
            http_client,
            ws_sink: Arc::new(RwLock::new(None)),
            audio_buffer: Arc::new(RwLock::new(Vec::new())),
            is_connected: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            current_job_id: Arc::new(RwLock::new(None)),
        })
    }

    /// Get the Prosa.ai specific configuration.
    pub fn get_prosa_config(&self) -> &ProsaSttConfig {
        &self.config
    }

    /// Set the STT model.
    pub fn set_model(&mut self, model: ProsaSttModel) {
        self.config.model = model;
    }

    /// Set the audio format.
    pub fn set_audio_format(&mut self, format: ProsaAudioFormat) {
        self.config.audio_format = format;
    }

    /// Enable or disable partial results.
    pub fn set_include_partial(&mut self, include: bool) {
        self.config.include_partial = include;
    }

    /// Set speaker count for diarization.
    pub fn set_speaker_count(&mut self, count: u32) {
        self.config.speaker_count = count;
    }

    /// Process audio using REST API (synchronous mode).
    async fn process_audio_rest(&self, audio_data: &[u8]) -> Result<String, STTError> {
        let url = PROSA_STT_BASE_URL;

        // Encode audio as base64
        let encoded_audio = BASE64.encode(audio_data);

        // Build request
        let request = ProsaSttRequest {
            config: ProsaSttRequestConfig {
                engine: self.config.model.as_str().to_string(),
                wait: Some(self.config.wait),
                speaker_count: if self.config.speaker_count > 0 {
                    Some(self.config.speaker_count)
                } else {
                    None
                },
                include_filler: Some(self.config.include_filler),
                auto_punctuation: Some(self.config.auto_punctuation),
                enable_spoken_numerals: Some(self.config.enable_spoken_numerals),
            },
            request: ProsaSttRequestData {
                label: self.config.label.clone(),
                data: Some(encoded_audio),
                uri: None,
            },
        };

        debug!("Sending STT request to Prosa.ai REST API");

        let response = self
            .http_client
            .post(url)
            .header("x-api-key", &self.config.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| STTError::ConnectionFailed(format!("Failed to send request: {}", e)))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| STTError::ProviderError(format!("Failed to read response: {}", e)))?;

        debug!(
            "Received response: status={}, body_len={}",
            status,
            body.len()
        );

        if !status.is_success() {
            return Err(self.parse_error_response(status.as_u16(), &body));
        }

        let prosa_response: ProsaSttResponse = serde_json::from_str(&body)
            .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

        // Store job ID
        if !prosa_response.job_id.is_empty() {
            let mut job_id = self.current_job_id.write().await;
            *job_id = Some(prosa_response.job_id.clone());
        }

        if prosa_response.has_error() {
            return Err(STTError::ProviderError(
                prosa_response.status_message().to_string(),
            ));
        }

        // If wait=false, we need to poll for results
        if !self.config.wait && prosa_response.is_in_progress() {
            return self.poll_job_result(&prosa_response.job_id).await;
        }

        prosa_response
            .transcription()
            .ok_or_else(|| STTError::ProviderError("No transcription in response".to_string()))
    }

    /// Poll for job result (async mode).
    async fn poll_job_result(&self, job_id: &str) -> Result<String, STTError> {
        let url = format!("{}/{}", PROSA_STT_BASE_URL, job_id);
        let max_attempts = 60; // Poll for up to 60 seconds
        let poll_interval = std::time::Duration::from_secs(1);

        for attempt in 0..max_attempts {
            debug!("Polling job {} (attempt {})", job_id, attempt + 1);

            let response = self
                .http_client
                .get(&url)
                .header("x-api-key", &self.config.api_key)
                .send()
                .await
                .map_err(|e| STTError::ConnectionFailed(format!("Failed to poll job: {}", e)))?;

            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| STTError::ProviderError(format!("Failed to read response: {}", e)))?;

            if !status.is_success() {
                return Err(self.parse_error_response(status.as_u16(), &body));
            }

            let prosa_response: ProsaSttResponse = serde_json::from_str(&body)
                .map_err(|e| STTError::ProviderError(format!("Failed to parse response: {}", e)))?;

            if prosa_response.is_complete() {
                return prosa_response.transcription().ok_or_else(|| {
                    STTError::ProviderError("No transcription in response".to_string())
                });
            }

            if prosa_response.has_error() {
                return Err(STTError::ProviderError(
                    prosa_response.status_message().to_string(),
                ));
            }

            tokio::time::sleep(poll_interval).await;
        }

        Err(STTError::ProviderError("Job polling timeout".to_string()))
    }

    /// Parse error response.
    fn parse_error_response(&self, status: u16, body: &str) -> STTError {
        // Try to parse as JSON error
        if let Ok(response) = serde_json::from_str::<ProsaSttResponse>(body) {
            if let Some(err) = response.error {
                return match err.code.as_str() {
                    "auth_invalid_api_key" | "auth_unauthorized" => {
                        STTError::AuthenticationFailed(err.message)
                    }
                    "forbidden" => STTError::AuthenticationFailed("Access forbidden".to_string()),
                    "quota_insufficient" | "quota_empty" => STTError::ProviderError(err.message),
                    _ => STTError::ProviderError(err.message),
                };
            }
        }

        // Fallback to status code based error
        match status {
            401 => STTError::AuthenticationFailed("Invalid API key".to_string()),
            403 => STTError::AuthenticationFailed("Access forbidden".to_string()),
            400 => STTError::ProviderError("Bad request".to_string()),
            404 => STTError::ProviderError("Not found".to_string()),
            422 => STTError::ProviderError("Validation error".to_string()),
            429 => STTError::ProviderError("Rate limited".to_string()),
            500..=599 => STTError::ProviderError("Server error".to_string()),
            _ => STTError::ProviderError(format!("HTTP error: {}", status)),
        }
    }

    /// Connect to WebSocket for streaming.
    async fn connect_websocket(&self) -> Result<(), STTError> {
        if !self.config.model.supports_streaming() {
            return Err(STTError::ConnectionFailed(
                "Selected model does not support streaming. Use stt-general-online for streaming."
                    .to_string(),
            ));
        }

        let url = format!(
            "{}?x-api-key={}",
            PROSA_STT_WS_ENDPOINT, self.config.api_key
        );

        debug!(
            "Connecting to Prosa.ai WebSocket: {}",
            PROSA_STT_WS_ENDPOINT
        );

        let (ws_stream, _) = connect_async(&url).await.map_err(|e| {
            STTError::ConnectionFailed(format!("WebSocket connection failed: {}", e))
        })?;

        let (write, mut read) = ws_stream.split();

        // Store write sink
        {
            let mut sink = self.ws_sink.write().await;
            *sink = Some(write);
        }

        // Send configuration
        let config = ProsaSttStreamConfig {
            model: self.config.model.as_str().to_string(),
            label: self.config.label.clone(),
            include_partial: Some(self.config.include_partial),
            audio: Some(ProsaSttAudioConfig {
                format: self.config.audio_format.as_str().to_string(),
                channels: Some(self.config.channels),
                sample_rate: Some(self.config.sample_rate),
            }),
        };

        let config_json = serde_json::to_string(&config).map_err(|e| {
            STTError::ConnectionFailed(format!("Failed to serialize config: {}", e))
        })?;

        {
            let mut sink = self.ws_sink.write().await;
            if let Some(ref mut ws) = *sink {
                ws.send(Message::Text(config_json.into()))
                    .await
                    .map_err(|e| {
                        STTError::ConnectionFailed(format!("Failed to send config: {}", e))
                    })?;
            }
        }

        // Spawn message handler
        let result_callback = self.result_callback.clone();
        let error_callback = self.error_callback.clone();
        let is_connected = self.is_connected.clone();
        let job_id = self.current_job_id.clone();

        tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        if let Ok(ws_msg) = serde_json::from_str::<ProsaSttWsMessage>(&text) {
                            match ws_msg {
                                ProsaSttWsMessage::Created { id } => {
                                    debug!("Streaming session created: {}", id);
                                    let mut job = job_id.write().await;
                                    *job = Some(id);
                                }
                                ProsaSttWsMessage::Partial { transcript } => {
                                    debug!("Partial transcript: {}", transcript);
                                    let callback = result_callback.read().await;
                                    if let Some(ref cb) = *callback {
                                        let result = STTResult::new(
                                            transcript.clone(),
                                            false, // is_final
                                            false, // is_speech_final
                                            0.0,   // confidence (not provided by partial)
                                        );
                                        cb(result).await;
                                    }
                                }
                                ProsaSttWsMessage::Result {
                                    transcript,
                                    time_start,
                                    time_end,
                                } => {
                                    debug!(
                                        "Final transcript: {} ({:.2}s - {:.2}s)",
                                        transcript, time_start, time_end
                                    );
                                    let callback = result_callback.read().await;
                                    if let Some(ref cb) = *callback {
                                        let result = STTResult::new(
                                            transcript.clone(),
                                            true, // is_final
                                            true, // is_speech_final (end of segment)
                                            1.0,  // confidence (Prosa doesn't provide, assume high)
                                        );
                                        cb(result).await;
                                    }
                                }
                                ProsaSttWsMessage::Status { status } => {
                                    debug!("Status update: {}", status);
                                }
                                ProsaSttWsMessage::Metadata {
                                    duration,
                                    quota_used,
                                } => {
                                    debug!(
                                        "Session metadata: duration={:?}, quota_used={:?}",
                                        duration, quota_used
                                    );
                                }
                                ProsaSttWsMessage::Error { message } => {
                                    error!("WebSocket error: {}", message);
                                    let callback = error_callback.read().await;
                                    if let Some(ref cb) = *callback {
                                        cb(STTError::ProviderError(message)).await;
                                    }
                                }
                            }
                        }
                    }
                    Ok(Message::Close(_)) => {
                        info!("WebSocket closed by server");
                        is_connected.store(false, Ordering::SeqCst);
                        break;
                    }
                    Err(e) => {
                        error!("WebSocket error: {}", e);
                        is_connected.store(false, Ordering::SeqCst);
                        break;
                    }
                    _ => {}
                }
            }
        });

        info!("Connected to Prosa.ai STT WebSocket");
        Ok(())
    }

    /// Send audio chunk via WebSocket.
    async fn send_audio_ws(&self, audio: &[u8]) -> Result<(), STTError> {
        let mut sink = self.ws_sink.write().await;
        if let Some(ref mut ws) = *sink {
            ws.send(Message::Binary(Bytes::copy_from_slice(audio)))
                .await
                .map_err(|e| STTError::ProviderError(format!("Failed to send audio: {}", e)))?;
        } else {
            return Err(STTError::ConnectionFailed(
                "WebSocket not connected".to_string(),
            ));
        }
        Ok(())
    }

    /// Signal end of audio stream.
    async fn end_audio_stream(&self) -> Result<(), STTError> {
        let mut sink = self.ws_sink.write().await;
        if let Some(ref mut ws) = *sink {
            ws.send(Message::Binary(Bytes::new())).await.map_err(|e| {
                STTError::ProviderError(format!("Failed to send end signal: {}", e))
            })?;
        }
        Ok(())
    }

    /// Flush the audio buffer and process the accumulated audio.
    /// For batch mode, this sends the buffered audio to the REST API.
    /// For streaming mode, this signals the end of the audio stream.
    pub async fn flush(&mut self) -> Result<String, STTError> {
        let audio_data = {
            let mut buffer = self.audio_buffer.write().await;
            let data = std::mem::take(&mut *buffer);
            data
        };

        if audio_data.is_empty() {
            return Ok(String::new());
        }

        if audio_data.len() < MIN_AUDIO_BUFFER_SIZE {
            debug!(
                "Audio buffer too small ({} bytes), skipping transcription",
                audio_data.len()
            );
            return Ok(String::new());
        }

        if self.config.model.supports_streaming() {
            // For streaming, signal end of stream
            self.end_audio_stream().await?;
            Ok(String::new()) // Results come via callback
        } else {
            // For batch mode, send to REST API
            let wav_data = self.wrap_in_wav(&audio_data);
            self.process_audio_rest(&wav_data).await
        }
    }

    /// Get the current buffer size.
    pub async fn buffer_size(&self) -> usize {
        let buffer = self.audio_buffer.read().await;
        buffer.len()
    }

    /// Wrap raw PCM audio in WAV header.
    fn wrap_in_wav(&self, pcm_data: &[u8]) -> Vec<u8> {
        let sample_rate = self.config.sample_rate;
        let channels = self.config.channels;
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
        let block_align = channels * bits_per_sample / 8;
        let data_size = pcm_data.len() as u32;
        let file_size = 36 + data_size;

        let mut wav = Vec::with_capacity(44 + pcm_data.len());

        // RIFF header
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&file_size.to_le_bytes());
        wav.extend_from_slice(b"WAVE");

        // fmt chunk
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // audio format (PCM)
        wav.extend_from_slice(&channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());

        // data chunk
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());
        wav.extend_from_slice(pcm_data);

        wav
    }
}

impl Default for ProsaStt {
    fn default() -> Self {
        Self {
            config: ProsaSttConfig::default(),
            base_config: None,
            http_client: Client::new(),
            ws_sink: Arc::new(RwLock::new(None)),
            audio_buffer: Arc::new(RwLock::new(Vec::new())),
            is_connected: Arc::new(AtomicBool::new(false)),
            result_callback: Arc::new(RwLock::new(None)),
            error_callback: Arc::new(RwLock::new(None)),
            current_job_id: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait]
impl BaseSTT for ProsaStt {
    fn new(config: STTConfig) -> Result<Self, STTError>
    where
        Self: Sized,
    {
        ProsaStt::new(config)
    }

    async fn connect(&mut self) -> Result<(), STTError> {
        if self.config.api_key.is_empty() {
            return Err(STTError::AuthenticationFailed(
                "Prosa.ai API key is required".to_string(),
            ));
        }

        // For streaming model, connect to WebSocket
        if self.config.model.supports_streaming() {
            self.connect_websocket().await?;
        }

        // Clear buffer
        {
            let mut buffer = self.audio_buffer.write().await;
            buffer.clear();
        }

        self.is_connected.store(true, Ordering::SeqCst);
        info!("Prosa.ai STT connected (model: {})", self.config.model);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), STTError> {
        // Send end signal for streaming
        if self.config.model.supports_streaming() {
            let _ = self.end_audio_stream().await;

            // Close WebSocket
            let mut sink = self.ws_sink.write().await;
            if let Some(ref mut ws) = *sink {
                let _ = ws.close().await;
            }
            *sink = None;
        }

        // Clear buffer
        {
            let mut buffer = self.audio_buffer.write().await;
            buffer.clear();
        }

        // Clear job ID
        {
            let mut job_id = self.current_job_id.write().await;
            *job_id = None;
        }

        self.is_connected.store(false, Ordering::SeqCst);
        info!("Prosa.ai STT disconnected");
        Ok(())
    }

    async fn send_audio(&mut self, audio: Bytes) -> Result<(), STTError> {
        if audio.is_empty() {
            return Ok(());
        }

        // Auto-connect if not connected
        if !self.is_ready() {
            self.connect().await?;
        }

        if self.config.model.supports_streaming() {
            // For streaming, send audio chunks directly
            self.send_audio_ws(&audio).await?;
        } else {
            // For batch mode, buffer the audio
            let mut buffer = self.audio_buffer.write().await;
            buffer.extend_from_slice(&audio);
        }

        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_connected.load(Ordering::SeqCst)
    }

    async fn on_result(&mut self, callback: STTResultCallback) -> Result<(), STTError> {
        let mut cb = self.result_callback.write().await;
        *cb = Some(callback);
        Ok(())
    }

    async fn on_error(&mut self, callback: STTErrorCallback) -> Result<(), STTError> {
        let mut cb = self.error_callback.write().await;
        *cb = Some(callback);
        Ok(())
    }

    fn get_provider_info(&self) -> &'static str {
        "Prosa.ai STT (Indonesian NLP)"
    }

    fn get_config(&self) -> Option<&STTConfig> {
        self.base_config.as_ref()
    }

    async fn update_config(&mut self, config: STTConfig) -> Result<(), STTError> {
        let new_config = ProsaSttConfig::from_base(&config)?;
        self.config = new_config;
        self.base_config = Some(config);
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> STTConfig {
        STTConfig {
            provider: "prosa-ai".to_string(),
            api_key: "test_key".to_string(),
            language: "id".to_string(),
            sample_rate: 16000,
            channels: 1,
            ..Default::default()
        }
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = ProsaStt::new(config);
        assert!(result.is_ok());

        let stt = result.unwrap();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = STTConfig {
            provider: "prosa-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = ProsaStt::new(config);
        assert!(result.is_err());

        match result {
            Err(STTError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("API key"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let stt = ProsaStt::new(make_test_config()).unwrap();
        let info = stt.get_provider_info();

        assert!(info.contains("Prosa.ai"));
        assert!(info.contains("Indonesian"));
    }

    #[test]
    fn test_default_state() {
        let stt = ProsaStt::default();
        assert!(!stt.is_ready());
    }

    #[test]
    fn test_get_config() {
        let config = make_test_config();
        let stt = ProsaStt::new(config.clone()).unwrap();

        let retrieved_config = stt.get_config();
        assert!(retrieved_config.is_some());
        assert_eq!(retrieved_config.unwrap().api_key, config.api_key);
    }

    #[test]
    fn test_get_prosa_config() {
        let stt = ProsaStt::new(make_test_config()).unwrap();
        let config = stt.get_prosa_config();

        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.channels, 1);
        assert_eq!(config.model, ProsaSttModel::General);
    }

    #[test]
    fn test_set_model() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert_eq!(stt.config.model, ProsaSttModel::General);

        stt.set_model(ProsaSttModel::GeneralOnline);
        assert_eq!(stt.config.model, ProsaSttModel::GeneralOnline);
    }

    #[test]
    fn test_set_audio_format() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert_eq!(stt.config.audio_format, ProsaAudioFormat::Wav);

        stt.set_audio_format(ProsaAudioFormat::Mp3);
        assert_eq!(stt.config.audio_format, ProsaAudioFormat::Mp3);
    }

    #[test]
    fn test_set_include_partial() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert!(stt.config.include_partial);

        stt.set_include_partial(false);
        assert!(!stt.config.include_partial);
    }

    #[test]
    fn test_set_speaker_count() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert_eq!(stt.config.speaker_count, 0);

        stt.set_speaker_count(2);
        assert_eq!(stt.config.speaker_count, 2);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        // REST mode doesn't need actual connection
        let result = stt.connect().await;

        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut stt = ProsaStt::default();
        let result = stt.connect().await;

        assert!(result.is_err());
        match result {
            Err(STTError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.disconnect().await;
        assert!(result.is_ok());
        assert!(!stt.is_ready());
    }

    #[tokio::test]
    async fn test_send_audio_empty() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.send_audio(Bytes::new()).await;
        assert!(result.is_ok());
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_send_audio_buffers() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let audio_data = Bytes::from(vec![0u8; 1000]);
        let result = stt.send_audio(audio_data).await;

        assert!(result.is_ok());
        assert_eq!(stt.buffer_size().await, 1000);
    }

    #[tokio::test]
    async fn test_send_audio_multiple() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();

        assert_eq!(stt.buffer_size().await, 1500);
    }

    #[tokio::test]
    async fn test_flush_empty_buffer() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        let result = stt.flush().await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_disconnect_clears_buffer() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
        assert!(stt.buffer_size().await > 0);

        stt.disconnect().await.unwrap();
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[tokio::test]
    async fn test_on_result_callback() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTResultCallback = Arc::new(move |_result: STTResult| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let result = stt.on_result(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_on_error_callback() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        let callback_count = Arc::new(AtomicUsize::new(0));
        let callback_count_clone = callback_count.clone();

        let callback: STTErrorCallback = Arc::new(move |_error: STTError| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        });

        let result = stt.on_error(callback).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_update_config() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();

        let new_config = STTConfig {
            provider: "prosa-ai".to_string(),
            api_key: "new_key".to_string(),
            sample_rate: 8000,
            channels: 2,
            ..Default::default()
        };

        let result = stt.update_config(new_config).await;
        assert!(result.is_ok());

        assert_eq!(stt.get_prosa_config().sample_rate, 8000);
        assert_eq!(stt.get_prosa_config().channels, 2);
        assert_eq!(stt.get_prosa_config().api_key, "new_key");
    }

    #[tokio::test]
    async fn test_auto_connect_on_send_audio() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        assert!(!stt.is_ready());

        // Should auto-connect
        let result = stt.send_audio(Bytes::from(vec![0u8; 100])).await;
        assert!(result.is_ok());
        assert!(stt.is_ready());
    }

    #[tokio::test]
    async fn test_buffer_size() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        assert_eq!(stt.buffer_size().await, 0);

        stt.send_audio(Bytes::from(vec![0u8; 500])).await.unwrap();
        assert_eq!(stt.buffer_size().await, 500);

        stt.send_audio(Bytes::from(vec![0u8; 300])).await.unwrap();
        assert_eq!(stt.buffer_size().await, 800);
    }

    #[tokio::test]
    async fn test_connect_clears_previous_buffer() {
        let mut stt = ProsaStt::new(make_test_config()).unwrap();
        stt.connect().await.unwrap();

        // Add some audio
        stt.send_audio(Bytes::from(vec![0u8; 1000])).await.unwrap();
        assert_eq!(stt.buffer_size().await, 1000);

        // Disconnect
        stt.disconnect().await.unwrap();
        assert_eq!(stt.buffer_size().await, 0);

        // Reconnect - buffer should be clear
        stt.connect().await.unwrap();
        assert_eq!(stt.buffer_size().await, 0);
    }

    #[test]
    fn test_wrap_in_wav() {
        let stt = ProsaStt::new(make_test_config()).unwrap();
        let pcm_data = vec![0u8; 1000];

        let wav_data = stt.wrap_in_wav(&pcm_data);

        // WAV header is 44 bytes
        assert_eq!(wav_data.len(), 44 + 1000);

        // Check RIFF header
        assert_eq!(&wav_data[0..4], b"RIFF");
        assert_eq!(&wav_data[8..12], b"WAVE");
        assert_eq!(&wav_data[12..16], b"fmt ");
        assert_eq!(&wav_data[36..40], b"data");
    }
}
