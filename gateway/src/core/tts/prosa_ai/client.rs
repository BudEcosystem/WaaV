//! Prosa.ai TTS client implementation.
//!
//! This module provides a REST API-based TTS client for Prosa.ai's
//! Text-to-Speech service.
//!
//! # Protocol
//!
//! Prosa.ai TTS uses REST API:
//! 1. POST JSON request with text, model, and parameters
//! 2. Receive JSON response with base64-encoded audio or job ID
//!
//! # Audio Format
//!
//! Output is available in opus (default), mp3, or wav format at 48kHz.

use super::config::{
    DEFAULT_SAMPLE_RATE, MAX_PITCH, MAX_TEMPO, MIN_PITCH, MIN_TEMPO, PROSA_TTS_BASE_URL,
    ProsaTtsAudioFormat, ProsaTtsConfig, ProsaTtsRequest, ProsaTtsRequestConfig,
    ProsaTtsRequestData, ProsaTtsResponse, ProsaTtsVoice,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Prosa.ai TTS client.
///
/// This client implements a REST API-based approach where each `speak()` call:
/// 1. Sends an HTTP POST with JSON body containing text and parameters
/// 2. Receives JSON response with base64-encoded audio
/// 3. Decodes and delivers audio via callback
pub struct ProsaTts {
    /// Provider configuration.
    config: ProsaTtsConfig,
    /// HTTP client for REST API calls.
    http_client: Client,
    /// Whether the client is ready to receive requests.
    is_ready: AtomicBool,
    /// Audio callback for streaming audio to caller.
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Current connection state.
    connection_state: Arc<RwLock<ConnectionState>>,
}

impl ProsaTts {
    /// Synthesize text and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // Validate text length
        self.config.validate_text(text)?;

        debug!(
            "Prosa TTS: Synthesizing {} characters with voice '{}' (tempo: {}, pitch: {})",
            text.chars().count(),
            self.config.voice.display_name(),
            self.config.tempo,
            self.config.pitch
        );

        // Build request body
        let request_body = ProsaTtsRequest {
            config: ProsaTtsRequestConfig {
                model: self.config.voice.model_id().to_string(),
                wait: Some(self.config.wait),
                pitch: Some(self.config.pitch),
                tempo: Some(self.config.tempo),
                audio_format: Some(self.config.audio_format.as_str().to_string()),
                as_signed_url: Some(self.config.as_signed_url),
            },
            request: ProsaTtsRequestData {
                label: self.config.label.clone(),
                text: text.to_string(),
            },
        };

        // Send request
        let response = self
            .http_client
            .post(PROSA_TTS_BASE_URL)
            .header("Content-Type", "application/json")
            .header("x-api-key", &self.config.api_key)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to send TTS request: {e}")))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read response: {e}")))?;

        debug!("Prosa TTS: Received response (status: {})", status.as_u16());

        if !status.is_success() {
            error!(
                "Prosa TTS: API error (status {}): {}",
                status.as_u16(),
                body
            );
            return Err(self.parse_error_response(status.as_u16(), &body));
        }

        // Parse response
        let prosa_response: ProsaTtsResponse = serde_json::from_str(&body)
            .map_err(|e| TTSError::ProviderError(format!("Failed to parse response: {e}")))?;

        if prosa_response.has_error() {
            return Err(TTSError::ProviderError(
                prosa_response.status_message().to_string(),
            ));
        }

        // If wait=false, we need to poll for results
        if !self.config.wait && prosa_response.is_in_progress() {
            return self.poll_job_result(&prosa_response.job_id).await;
        }

        // Get audio data
        let audio_data = if let Some(data) = prosa_response.audio_data() {
            if data.is_empty() {
                return Err(TTSError::ProviderError("Empty audio data".to_string()));
            }
            BASE64
                .decode(data)
                .map_err(|e| TTSError::ProviderError(format!("Failed to decode audio: {e}")))?
        } else if let Some(url) = prosa_response.audio_url() {
            // Download audio from URL
            self.download_audio(url).await?
        } else {
            return Err(TTSError::ProviderError("No audio in response".to_string()));
        };

        info!(
            "Prosa TTS: Received {} bytes of audio (duration: {:.2}s)",
            audio_data.len(),
            prosa_response.duration().unwrap_or(0.0)
        );

        Ok(audio_data)
    }

    /// Poll for job result (async mode).
    async fn poll_job_result(&self, job_id: &str) -> TTSResult<Vec<u8>> {
        let url = format!("{}/{}", PROSA_TTS_BASE_URL, job_id);
        let max_attempts = 60;
        let poll_interval = std::time::Duration::from_secs(1);

        for attempt in 0..max_attempts {
            debug!(
                "Prosa TTS: Polling job {} (attempt {})",
                job_id,
                attempt + 1
            );

            let response = self
                .http_client
                .get(&url)
                .header("x-api-key", &self.config.api_key)
                .send()
                .await
                .map_err(|e| TTSError::NetworkError(format!("Failed to poll job: {e}")))?;

            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|e| TTSError::NetworkError(format!("Failed to read response: {e}")))?;

            if !status.is_success() {
                return Err(self.parse_error_response(status.as_u16(), &body));
            }

            let prosa_response: ProsaTtsResponse = serde_json::from_str(&body)
                .map_err(|e| TTSError::ProviderError(format!("Failed to parse response: {e}")))?;

            if prosa_response.is_complete() {
                // Get audio data
                if let Some(data) = prosa_response.audio_data() {
                    return BASE64.decode(data).map_err(|e| {
                        TTSError::ProviderError(format!("Failed to decode audio: {e}"))
                    });
                } else if let Some(url) = prosa_response.audio_url() {
                    return self.download_audio(url).await;
                }
                return Err(TTSError::ProviderError("No audio in response".to_string()));
            }

            if prosa_response.has_error() {
                return Err(TTSError::ProviderError(
                    prosa_response.status_message().to_string(),
                ));
            }

            tokio::time::sleep(poll_interval).await;
        }

        Err(TTSError::TimeoutError("Job polling timeout".to_string()))
    }

    /// Download audio from URL.
    async fn download_audio(&self, url: &str) -> TTSResult<Vec<u8>> {
        debug!("Prosa TTS: Downloading audio from URL");

        let response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to download audio: {e}")))?;

        if !response.status().is_success() {
            return Err(TTSError::ProviderError(format!(
                "Failed to download audio: HTTP {}",
                response.status().as_u16()
            )));
        }

        let audio_data = response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read audio data: {e}")))?
            .to_vec();

        Ok(audio_data)
    }

    /// Parse error response.
    fn parse_error_response(&self, status: u16, body: &str) -> TTSError {
        // Try to parse as JSON error
        if let Ok(response) = serde_json::from_str::<ProsaTtsResponse>(body) {
            if let Some(err) = response.error {
                return match err.code.as_str() {
                    "auth_invalid_api_key" | "auth_unauthorized" => {
                        TTSError::AuthenticationFailed(err.message)
                    }
                    "forbidden" => TTSError::AuthenticationFailed("Access forbidden".to_string()),
                    "quota_insufficient" | "quota_empty" => {
                        TTSError::ProviderError(format!("Quota exceeded: {}", err.message))
                    }
                    _ => TTSError::ProviderError(err.message),
                };
            }
        }

        // Fallback to status code based error
        match status {
            401 => TTSError::AuthenticationFailed("Invalid API key".to_string()),
            403 => TTSError::AuthenticationFailed("Access forbidden".to_string()),
            400 => TTSError::InvalidConfiguration("Bad request".to_string()),
            404 => TTSError::ProviderError("Not found".to_string()),
            422 => TTSError::InvalidConfiguration("Validation error".to_string()),
            429 => TTSError::RateLimited {
                retry_after_secs: None,
                message: "Rate limited".to_string(),
            },
            500..=599 => TTSError::ProviderError("Server error".to_string()),
            _ => TTSError::ProviderError(format!("HTTP error: {}", status)),
        }
    }

    /// Get the Prosa-specific configuration.
    pub fn get_prosa_config(&self) -> &ProsaTtsConfig {
        &self.config
    }

    /// Set the voice.
    pub fn set_voice(&mut self, voice: ProsaTtsVoice) {
        self.config.voice = voice;
    }

    /// Set the audio format.
    pub fn set_audio_format(&mut self, format: ProsaTtsAudioFormat) {
        self.config.audio_format = format;
    }

    /// Set the pitch offset.
    pub fn set_pitch(&mut self, pitch: i32) {
        self.config.pitch = pitch.clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Set the tempo/speed.
    pub fn set_tempo(&mut self, tempo: f32) {
        self.config.tempo = tempo.clamp(MIN_TEMPO, MAX_TEMPO);
    }
}

impl Default for ProsaTts {
    fn default() -> Self {
        Self {
            config: ProsaTtsConfig::default(),
            http_client: Client::new(),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        }
    }
}

#[async_trait::async_trait]
impl BaseTTS for ProsaTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let prosa_config = ProsaTtsConfig::from_base(config)?;

        let timeout_secs = prosa_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "Prosa TTS: Initialized with voice='{}', format={}",
            prosa_config.voice.display_name(),
            prosa_config.audio_format
        );

        Ok(Self {
            config: prosa_config,
            http_client,
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        // Validate credentials
        if self.config.api_key.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "Prosa.ai API key is required".to_string(),
            ));
        }

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Connecting;
        }

        // REST API doesn't need a persistent connection
        // Just mark as ready
        self.is_ready.store(true, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Connected;
        }

        info!("Prosa TTS: Ready to synthesize");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        info!("Prosa TTS: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }

    fn get_connection_state(&self) -> ConnectionState {
        if self.is_ready() {
            ConnectionState::Connected
        } else {
            ConnectionState::Disconnected
        }
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if !self.is_ready() {
            // Auto-connect if not ready
            self.connect().await?;
        }

        if text.is_empty() {
            return Ok(());
        }

        // Synthesize the audio
        let audio_data = self.synthesize(text).await?;

        if audio_data.is_empty() {
            return Ok(());
        }

        // Send audio via callback
        let callback_guard = self.audio_callback.read().await;
        if let Some(callback) = callback_guard.as_ref() {
            let data = AudioData {
                data: audio_data,
                sample_rate: DEFAULT_SAMPLE_RATE,
                format: self.config.audio_format.as_str().to_string(),
                duration_ms: None,
            };

            callback.on_audio(data).await;

            if flush {
                callback.on_complete().await;
            }
        }

        Ok(())
    }

    async fn clear(&mut self) -> TTSResult<()> {
        // No queue to clear for REST API
        Ok(())
    }

    async fn flush(&self) -> TTSResult<()> {
        // REST API sends immediately, nothing to flush
        // Just trigger complete callback if registered
        let callback_guard = self.audio_callback.read().await;
        if let Some(callback) = callback_guard.as_ref() {
            callback.on_complete().await;
        }
        Ok(())
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        let audio_callback = Arc::clone(&self.audio_callback);
        tokio::spawn(async move {
            let mut guard = audio_callback.write().await;
            *guard = Some(callback);
        });
        Ok(())
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        let audio_callback = Arc::clone(&self.audio_callback);
        tokio::spawn(async move {
            let mut guard = audio_callback.write().await;
            *guard = None;
        });
        Ok(())
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "prosa-ai",
            "name": "Prosa.ai TTS (Indonesian NLP)",
            "version": "1.0.0",
            "voice": self.config.voice.model_id(),
            "voice_name": self.config.voice.display_name(),
            "pitch": self.config.pitch,
            "tempo": self.config.tempo,
            "format": self.config.audio_format.as_str(),
            "supported_formats": ["opus", "mp3", "wav"],
            "supported_languages": ["id", "en"],
            "sample_rate": DEFAULT_SAMPLE_RATE,
            "features": {
                "pitch_control": true,
                "tempo_control": true,
                "multiple_voices": true,
                "async_mode": true,
            }
        })
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

    fn make_test_config() -> TTSConfig {
        TTSConfig {
            provider: "prosa-ai".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("dimas".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = ProsaTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = TTSConfig {
            provider: "prosa-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = ProsaTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_provider_info() {
        let tts = ProsaTts::new(make_test_config()).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "prosa-ai");
        assert!(info["voice"].as_str().unwrap().contains("dimas"));
    }

    #[test]
    fn test_default_state() {
        let tts = ProsaTts::default();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_get_prosa_config() {
        let tts = ProsaTts::new(make_test_config()).unwrap();
        let config = tts.get_prosa_config();

        assert_eq!(config.api_key, "test_key");
        assert_eq!(config.voice, ProsaTtsVoice::DimasFormal);
    }

    #[test]
    fn test_set_voice() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();
        assert_eq!(tts.config.voice, ProsaTtsVoice::DimasFormal);

        tts.set_voice(ProsaTtsVoice::OchaFriendly);
        assert_eq!(tts.config.voice, ProsaTtsVoice::OchaFriendly);
    }

    #[test]
    fn test_set_audio_format() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();

        tts.set_audio_format(ProsaTtsAudioFormat::Mp3);
        assert_eq!(tts.config.audio_format, ProsaTtsAudioFormat::Mp3);
    }

    #[test]
    fn test_set_pitch() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();

        tts.set_pitch(5);
        assert_eq!(tts.config.pitch, 5);

        // Test clamping
        tts.set_pitch(100);
        assert_eq!(tts.config.pitch, MAX_PITCH);

        tts.set_pitch(-100);
        assert_eq!(tts.config.pitch, MIN_PITCH);
    }

    #[test]
    fn test_set_tempo() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();

        tts.set_tempo(1.5);
        assert!((tts.config.tempo - 1.5).abs() < f32::EPSILON);

        // Test clamping
        tts.set_tempo(10.0);
        assert!((tts.config.tempo - MAX_TEMPO).abs() < f32::EPSILON);

        tts.set_tempo(0.1);
        assert!((tts.config.tempo - MIN_TEMPO).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();
        let result = tts.connect().await;

        assert!(result.is_ok());
        assert!(tts.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut tts = ProsaTts::default();
        let result = tts.connect().await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_clear() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();
        let result = tts.clear().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_flush() {
        let tts = ProsaTts::new(make_test_config()).unwrap();
        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    struct TestCallback;

    impl AudioCallback for TestCallback {
        fn on_audio(&self, _data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }

        fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }

        fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn test_on_audio_callback() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();
        let callback = Arc::new(TestCallback);

        let result = tts.on_audio(callback);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_remove_audio_callback() {
        let mut tts = ProsaTts::new(make_test_config()).unwrap();
        let callback = Arc::new(TestCallback);
        tts.on_audio(callback).unwrap();

        let result = tts.remove_audio_callback();
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_error_response_auth() {
        let tts = ProsaTts::new(make_test_config()).unwrap();
        let error = tts.parse_error_response(401, "Unauthorized");

        match error {
            TTSError::AuthenticationFailed(_) => {}
            _ => panic!("Expected AuthenticationFailed"),
        }
    }

    #[test]
    fn test_parse_error_response_rate_limit() {
        let tts = ProsaTts::new(make_test_config()).unwrap();
        let error = tts.parse_error_response(429, "Rate limited");

        match error {
            TTSError::RateLimited { .. } => {}
            _ => panic!("Expected RateLimited"),
        }
    }

    #[test]
    fn test_connection_state() {
        let tts = ProsaTts::new(make_test_config()).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }
}
