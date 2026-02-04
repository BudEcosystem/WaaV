//! FPT.AI TTS client implementation.
//!
//! This module provides a REST API-based TTS client for FPT Corporation's
//! FPT.AI Text-to-Speech service.
//!
//! # Protocol
//!
//! FPT TTS uses a two-step REST API:
//! 1. POST plain text to `/hmi/tts/v5` with voice/speed headers
//! 2. Receive JSON with async audio URL
//! 3. Download audio from the URL (may need to wait/poll)
//!
//! # Audio Format
//!
//! Output is MP3 or WAV format depending on configuration.

use super::config::{
    AUDIO_SAMPLE_RATE, FPT_TTS_ENDPOINT, FptAudioFormat, FptTtsConfig, FptTtsResponse, FptVoice,
    MAX_SPEED, MIN_SPEED,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Maximum number of retries for downloading audio (FPT uses async processing).
const MAX_DOWNLOAD_RETRIES: u32 = 10;

/// Delay between download retry attempts in milliseconds.
const DOWNLOAD_RETRY_DELAY_MS: u64 = 500;

/// FPT.AI TTS client.
///
/// This client implements a REST API-based approach where each `speak()` call:
/// 1. Sends an HTTP POST with text in the body
/// 2. Receives JSON response with async audio URL
/// 3. Downloads the audio from the URL (with retries for async processing)
pub struct FptTts {
    /// Provider configuration.
    config: FptTtsConfig,
    /// HTTP client for REST API calls.
    http_client: Client,
    /// Whether the client is ready to receive requests.
    is_ready: AtomicBool,
    /// Audio callback for streaming audio to caller.
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Current connection state.
    connection_state: Arc<RwLock<ConnectionState>>,
}

impl FptTts {
    /// Synthesize text and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // Validate text length
        FptTtsConfig::validate_text(text)?;

        debug!(
            "FPT TTS: Synthesizing {} characters with voice '{}' (speed: {})",
            text.chars().count(),
            self.config.voice.display_name(),
            self.config.speed
        );

        // Step 1: Request audio URL
        let mut request_builder = self
            .http_client
            .post(FPT_TTS_ENDPOINT)
            .header("api_key", &self.config.api_key)
            .header("voice", self.config.voice.voice_id())
            .header("speed", self.config.speed.to_string())
            .header("format", self.config.format.as_str())
            .header("Content-Type", "text/plain; charset=utf-8");

        // Add callback URL if configured
        if let Some(callback_url) = &self.config.callback_url {
            request_builder = request_builder.header("callback_url", callback_url);
        }

        let response = request_builder
            .body(text.to_string())
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to send TTS request: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!("FPT TTS: API error (status {}): {}", status.as_u16(), body);

            return match status.as_u16() {
                401 => Err(TTSError::AuthenticationFailed(format!(
                    "Invalid API key: {body}"
                ))),
                429 => Err(TTSError::RateLimited {
                    retry_after_secs: None,
                    message: body,
                }),
                400 => Err(TTSError::InvalidConfiguration(format!(
                    "Bad request: {body}"
                ))),
                _ => Err(TTSError::ProviderError(format!(
                    "API error (status {}): {}",
                    status.as_u16(),
                    body
                ))),
            };
        }

        // Parse JSON response
        let api_response: FptTtsResponse = response
            .json()
            .await
            .map_err(|e| TTSError::ProviderError(format!("Failed to parse API response: {e}")))?;

        // Check for API-level errors
        if !api_response.is_success() {
            let error_msg = if api_response.message.is_empty() {
                format!("Error code: {}", api_response.error)
            } else {
                api_response.message.clone()
            };

            error!(
                "FPT TTS: API error code {}: {}",
                api_response.error, error_msg
            );

            return match api_response.error {
                401 => Err(TTSError::AuthenticationFailed(error_msg)),
                429 => Err(TTSError::RateLimited {
                    retry_after_secs: Some(1),
                    message: error_msg,
                }),
                _ => Err(TTSError::ProviderError(error_msg)),
            };
        }

        // Get audio URL
        let audio_url = api_response
            .audio_url()
            .ok_or_else(|| TTSError::ProviderError("No audio URL in response".to_string()))?;

        debug!(
            "FPT TTS: Got async URL, request_id={}, downloading from {}",
            api_response.request_id, audio_url
        );

        // Step 2: Download audio from URL with retries (FPT uses async processing)
        let audio_data = self.download_audio_with_retry(audio_url).await?;

        info!("FPT TTS: Received {} bytes of audio", audio_data.len());

        Ok(audio_data)
    }

    /// Download audio from URL with retry logic for async processing.
    async fn download_audio_with_retry(&self, url: &str) -> TTSResult<Vec<u8>> {
        let mut last_error = None;

        for attempt in 1..=MAX_DOWNLOAD_RETRIES {
            match self.try_download_audio(url).await {
                Ok(data) => return Ok(data),
                Err(e) => {
                    // Check if it's a temporary error (audio not ready yet)
                    if attempt < MAX_DOWNLOAD_RETRIES {
                        let is_temporary = matches!(
                            &e,
                            TTSError::NetworkError(msg) if msg.contains("404") || msg.contains("not ready")
                        );

                        if is_temporary {
                            debug!(
                                "FPT TTS: Audio not ready yet, attempt {}/{}, waiting {}ms",
                                attempt, MAX_DOWNLOAD_RETRIES, DOWNLOAD_RETRY_DELAY_MS
                            );
                            tokio::time::sleep(tokio::time::Duration::from_millis(
                                DOWNLOAD_RETRY_DELAY_MS,
                            ))
                            .await;
                            last_error = Some(e);
                            continue;
                        }
                    }
                    last_error = Some(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            TTSError::ProviderError("Failed to download audio after max retries".to_string())
        }))
    }

    /// Try to download audio from URL once.
    async fn try_download_audio(&self, url: &str) -> TTSResult<Vec<u8>> {
        let audio_response = self
            .http_client
            .get(url)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to download audio: {e}")))?;

        if !audio_response.status().is_success() {
            let status = audio_response.status().as_u16();
            return Err(TTSError::NetworkError(format!(
                "Failed to download audio: status {status} (not ready)"
            )));
        }

        let audio_data = audio_response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read audio data: {e}")))?
            .to_vec();

        // Verify we got actual audio data (not empty or error HTML)
        if audio_data.is_empty() {
            return Err(TTSError::ProviderError(
                "Received empty audio data".to_string(),
            ));
        }

        Ok(audio_data)
    }

    /// Get the FPT-specific configuration.
    pub fn get_fpt_config(&self) -> &FptTtsConfig {
        &self.config
    }

    /// Set the voice.
    pub fn set_voice(&mut self, voice: FptVoice) {
        self.config.voice = voice;
    }

    /// Set the speech speed.
    pub fn set_speed(&mut self, speed: i8) {
        self.config.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// Set the audio format.
    pub fn set_format(&mut self, format: FptAudioFormat) {
        self.config.format = format;
    }
}

impl Default for FptTts {
    fn default() -> Self {
        Self {
            config: FptTtsConfig::default(),
            http_client: Client::new(),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        }
    }
}

#[async_trait::async_trait]
impl BaseTTS for FptTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let fpt_config = FptTtsConfig::from_base(config)?;

        let timeout_secs = fpt_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "FPT TTS: Initialized with voice='{}', speed={}, format={}",
            fpt_config.voice.display_name(),
            fpt_config.speed,
            fpt_config.format.as_str()
        );

        Ok(Self {
            config: fpt_config,
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
                "FPT.AI API key is required".to_string(),
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

        info!("FPT TTS: Ready to synthesize");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        info!("FPT TTS: Disconnected");
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

        // Check minimum text length
        if text.chars().count() < super::config::MIN_TEXT_LENGTH {
            warn!(
                "FPT TTS: Text too short ({} chars), minimum is {}. Padding with spaces.",
                text.chars().count(),
                super::config::MIN_TEXT_LENGTH
            );
            // Pad with spaces if too short
            let padded = format!("{text}   ");
            return self.speak_internal(&padded, flush).await;
        }

        self.speak_internal(text, flush).await
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
        // Use tokio::spawn to avoid blocking in single-threaded runtime
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
            "provider": "fpt-ai",
            "name": "FPT.AI TTS (FPT Corporation)",
            "version": "1.0.0",
            "voice": self.config.voice.voice_id(),
            "voice_name": self.config.voice.display_name(),
            "speed": self.config.speed,
            "format": self.config.format.as_str(),
            "supported_formats": ["mp3", "wav"],
            "supported_languages": ["vi"],
            "sample_rate": AUDIO_SAMPLE_RATE,
            "features": {
                "speed_control": true,
                "northern_accent": true,
                "format_selection": true,
                "async_processing": true
            },
            "voices": [
                {"id": "banmai", "name": "Ban Mai", "accent": "northern", "gender": "female"},
                {"id": "lannhi", "name": "Lan Nhi", "accent": "northern", "gender": "female"},
                {"id": "leminh", "name": "Le Minh", "accent": "northern", "gender": "male"},
                {"id": "myan", "name": "My An", "accent": "standard", "gender": "female"},
                {"id": "thuminh", "name": "Thu Minh", "accent": "standard", "gender": "female"},
                {"id": "giahuy", "name": "Gia Huy", "accent": "standard", "gender": "male"},
                {"id": "linhsan", "name": "Linh San", "accent": "standard", "gender": "female"}
            ]
        })
    }
}

impl FptTts {
    /// Internal speak implementation after validation.
    async fn speak_internal(&mut self, text: &str, flush: bool) -> TTSResult<()> {
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
                sample_rate: AUDIO_SAMPLE_RATE,
                format: self.config.format.as_str().to_string(),
                duration_ms: None,
            };

            callback.on_audio(data).await;

            if flush {
                callback.on_complete().await;
            }
        }

        Ok(())
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
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> TTSConfig {
        TTSConfig {
            provider: "fpt-ai".to_string(),
            api_key: "test_api_key".to_string(),
            voice_id: Some("banmai".to_string()),
            ..Default::default()
        }
    }

    struct MockAudioCallback {
        audio_count: AtomicUsize,
        complete_count: AtomicUsize,
    }

    impl MockAudioCallback {
        fn new() -> Self {
            Self {
                audio_count: AtomicUsize::new(0),
                complete_count: AtomicUsize::new(0),
            }
        }

        #[allow(dead_code)]
        fn get_audio_count(&self) -> usize {
            self.audio_count.load(Ordering::SeqCst)
        }

        #[allow(dead_code)]
        fn get_complete_count(&self) -> usize {
            self.complete_count.load(Ordering::SeqCst)
        }
    }

    impl AudioCallback for MockAudioCallback {
        fn on_audio(
            &self,
            _audio_data: AudioData,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.audio_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }

        fn on_error(&self, _error: TTSError) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(async {})
        }

        fn on_complete(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            self.complete_count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = FptTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = TTSConfig {
            provider: "fpt-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = FptTts::new(config);
        assert!(result.is_err());

        match result {
            Err(TTSError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("API key"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let tts = FptTts::new(make_test_config()).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "fpt-ai");
        assert_eq!(info["name"], "FPT.AI TTS (FPT Corporation)");
        assert!(info["voices"].as_array().unwrap().len() == 7);
    }

    #[test]
    fn test_default_state() {
        let tts = FptTts::default();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_set_voice() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        tts.set_voice(FptVoice::LeMinh);

        assert_eq!(tts.config.voice, FptVoice::LeMinh);
    }

    #[test]
    fn test_set_speed() {
        let mut tts = FptTts::new(make_test_config()).unwrap();

        // Normal speed
        tts.set_speed(2);
        assert_eq!(tts.config.speed, 2);

        // Below minimum - should clamp
        tts.set_speed(-5);
        assert_eq!(tts.config.speed, MIN_SPEED);

        // Above maximum - should clamp
        tts.set_speed(5);
        assert_eq!(tts.config.speed, MAX_SPEED);
    }

    #[test]
    fn test_set_format() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        tts.set_format(FptAudioFormat::Wav);

        assert_eq!(tts.config.format, FptAudioFormat::Wav);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        let result = tts.connect().await;

        assert!(result.is_ok());
        assert!(tts.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut tts = FptTts::default();
        let result = tts.connect().await;

        assert!(result.is_err());
        match result {
            Err(TTSError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_speak_empty_text() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.speak("", false).await;
        assert!(result.is_ok()); // Empty text should succeed (no-op)
    }

    #[tokio::test]
    async fn test_flush() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback.clone()).unwrap();

        // Give time for the async callback registration
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let result = tts.flush().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.clear().await;
        assert!(result.is_ok()); // Clear is a no-op for REST API
    }

    #[test]
    fn test_voice_parsing() {
        let test_cases = vec![
            ("banmai", FptVoice::BanMai),
            ("lannhi", FptVoice::LanNhi),
            ("leminh", FptVoice::LeMinh),
            ("myan", FptVoice::MyAn),
            ("thuminh", FptVoice::ThuMinh),
            ("giahuy", FptVoice::GiaHuy),
            ("linhsan", FptVoice::LinhSan),
            ("ban_mai", FptVoice::BanMai),
            ("le_minh", FptVoice::LeMinh),
        ];

        for (voice_id, expected_voice) in test_cases {
            let config = TTSConfig {
                provider: "fpt-ai".to_string(),
                api_key: "test".to_string(),
                voice_id: Some(voice_id.to_string()),
                ..Default::default()
            };

            let tts = FptTts::new(config).unwrap();
            assert_eq!(
                tts.config.voice, expected_voice,
                "Voice ID '{}' should map to {:?}",
                voice_id, expected_voice
            );
        }
    }

    #[test]
    fn test_connection_state() {
        let tts = FptTts::new(make_test_config()).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_connection_state_after_connect() {
        let mut tts = FptTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);
    }

    #[test]
    fn test_format_parsing() {
        let config = TTSConfig {
            provider: "fpt-ai".to_string(),
            api_key: "test".to_string(),
            audio_format: Some("wav".to_string()),
            ..Default::default()
        };

        let tts = FptTts::new(config).unwrap();
        assert_eq!(tts.config.format, FptAudioFormat::Wav);
    }

    #[test]
    fn test_speed_conversion_from_speaking_rate() {
        // 1.0 -> 0 (normal)
        let config = TTSConfig {
            provider: "fpt-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(1.0),
            ..Default::default()
        };
        let tts = FptTts::new(config).unwrap();
        assert_eq!(tts.config.speed, 0);

        // 1.5 -> +3 (fast)
        let config = TTSConfig {
            provider: "fpt-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(1.5),
            ..Default::default()
        };
        let tts = FptTts::new(config).unwrap();
        assert_eq!(tts.config.speed, 3);

        // 0.5 -> -3 (slow)
        let config = TTSConfig {
            provider: "fpt-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(0.5),
            ..Default::default()
        };
        let tts = FptTts::new(config).unwrap();
        assert_eq!(tts.config.speed, -3);
    }
}
