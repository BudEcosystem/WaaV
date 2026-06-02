//! Zalo AI TTS client implementation.
//!
//! This module provides a REST API-based TTS client for VNG Corporation's
//! Zalo AI Text-to-Speech service.
//!
//! # Protocol
//!
//! Zalo TTS uses a two-step REST API:
//! 1. POST to `/v1/tts/synthesize` with text and parameters
//! 2. Receive JSON with audio URL
//! 3. Download audio from the URL
//!
//! # Audio Format
//!
//! Output is WAV format:
//! - Sample Rate: 16kHz
//! - Channels: Mono
//! - Sample Width: 16-bit

use super::config::{AUDIO_SAMPLE_RATE, ZALO_TTS_ENDPOINT, ZaloTtsConfig, ZaloTtsResponse};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Zalo AI TTS client.
///
/// This client implements a REST API-based approach where each `speak()` call:
/// 1. Sends an HTTP POST to get an audio URL
/// 2. Downloads the audio from the URL
/// 3. Returns the audio data
pub struct ZaloTts {
    /// Provider configuration.
    config: ZaloTtsConfig,
    /// HTTP client for REST API calls.
    http_client: Client,
    /// Whether the client is ready to receive requests.
    is_ready: AtomicBool,
    /// Audio callback for streaming audio to caller.
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Current connection state.
    connection_state: Arc<RwLock<ConnectionState>>,
}

impl ZaloTts {
    /// Synthesize text and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        let request_body = self.config.build_request_body(text);

        debug!(
            "Zalo TTS: Synthesizing {} characters with voice '{}' (speed: {})",
            text.len(),
            self.config.voice.display_name(),
            self.config.speed
        );

        // Step 1: Request audio URL
        let response = self
            .http_client
            .post(ZALO_TTS_ENDPOINT)
            .header("apikey", &self.config.api_key)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(request_body)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to send TTS request: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!("Zalo TTS: API error (status {}): {}", status.as_u16(), body);

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
        let api_response: ZaloTtsResponse = response
            .json()
            .await
            .map_err(|e| TTSError::ProviderError(format!("Failed to parse API response: {e}")))?;

        // Check for API-level errors
        if !api_response.is_success() {
            let error_msg = if api_response.error_message.is_empty() {
                format!("Error code: {}", api_response.error_code)
            } else {
                api_response.error_message.clone()
            };

            error!(
                "Zalo TTS: API error code {}: {}",
                api_response.error_code, error_msg
            );

            return match api_response.error_code {
                401 => Err(TTSError::AuthenticationFailed(error_msg)),
                155 => Err(TTSError::RateLimited {
                    retry_after_secs: Some(1),
                    message: error_msg,
                }),
                500 => Err(TTSError::ProviderError(format!(
                    "Server error: {error_msg}"
                ))),
                _ => Err(TTSError::ProviderError(error_msg)),
            };
        }

        // Get audio URL
        let audio_url = api_response
            .audio_url()
            .ok_or_else(|| TTSError::ProviderError("No audio URL in response".to_string()))?;

        debug!("Zalo TTS: Downloading audio from {}", audio_url);

        // Step 2: Download audio from URL
        let audio_response = self
            .http_client
            .get(audio_url)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to download audio: {e}")))?;

        if !audio_response.status().is_success() {
            return Err(TTSError::ProviderError(format!(
                "Failed to download audio: status {}",
                audio_response.status()
            )));
        }

        let audio_data = audio_response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read audio data: {e}")))?
            .to_vec();

        info!("Zalo TTS: Received {} bytes of audio", audio_data.len());

        Ok(audio_data)
    }

    /// Build from the standardized TTS config (W1 keystone). Mirrors [`BaseTTS::new`] but maps the
    /// standardized features via [`ZaloTtsConfig::from_standard`] (which honors `speed` and the
    /// `request_timeout_secs` extra) before constructing the timeout-bounded HTTP client. Features
    /// Zalo cannot express stay at provider defaults (capability gaps).
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let zalo_config = ZaloTtsConfig::from_standard(std)?;

        let timeout_secs = zalo_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "Zalo TTS: Initialized (standardized) with voice='{}', speed={}",
            zalo_config.voice.display_name(),
            zalo_config.speed
        );

        Ok(Self {
            config: zalo_config,
            http_client,
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        })
    }

    /// Get the Zalo-specific configuration.
    pub fn get_zalo_config(&self) -> &ZaloTtsConfig {
        &self.config
    }

    /// Set the voice.
    pub fn set_voice(&mut self, voice: super::config::ZaloVoice) {
        self.config.voice = voice;
    }

    /// Set the speech speed.
    pub fn set_speed(&mut self, speed: f32) {
        self.config.speed = speed.clamp(super::config::MIN_SPEED, super::config::MAX_SPEED);
    }
}

impl Default for ZaloTts {
    fn default() -> Self {
        Self {
            config: ZaloTtsConfig::default(),
            http_client: Client::new(),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        }
    }
}

#[async_trait::async_trait]
impl BaseTTS for ZaloTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let zalo_config = ZaloTtsConfig::from_base(config)?;

        let timeout_secs = zalo_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "Zalo TTS: Initialized with voice='{}', speed={}",
            zalo_config.voice.display_name(),
            zalo_config.speed
        );

        Ok(Self {
            config: zalo_config,
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
                "Zalo API key is required".to_string(),
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

        info!("Zalo TTS: Ready to synthesize");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        info!("Zalo TTS: Disconnected");
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
                sample_rate: AUDIO_SAMPLE_RATE,
                format: "wav".to_string(),
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
            "provider": "zalo-ai",
            "name": "Zalo AI TTS (VNG Corporation)",
            "version": "1.0.0",
            "voice": self.config.voice.speaker_id(),
            "voice_name": self.config.voice.display_name(),
            "speed": self.config.speed,
            "supported_formats": ["wav"],
            "supported_languages": ["vi"],
            "sample_rate": AUDIO_SAMPLE_RATE,
            "features": {
                "speed_control": true,
                "northern_accent": true,
                "southern_accent": true
            },
            "voices": [
                {"id": "1", "name": "Female Southern", "accent": "southern", "gender": "female"},
                {"id": "2", "name": "Female Northern", "accent": "northern", "gender": "female"},
                {"id": "3", "name": "Male Southern", "accent": "southern", "gender": "male"},
                {"id": "4", "name": "Male Northern", "accent": "northern", "gender": "male"}
            ]
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::zalo_ai::config::ZaloVoice;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> TTSConfig {
        TTSConfig {
            provider: "zalo-ai".to_string(),
            api_key: "test_api_key".to_string(),
            voice_id: Some("2".to_string()), // Female Northern
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
        let result = ZaloTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
    }

    // W1 keystone (TTS): the struct-level `from_standard` builds a real `ZaloTts` through the
    // standardized path, carrying the `speed` feature (Zalo's only prosody knob) onto the provider
    // config the request builder reads. Mirrors `DeepgramTTS::from_standard`.
    #[test]
    fn from_standard_builds_provider_with_speed() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "zalo_ai".into(),
                api_key: "test_key".into(),
                voice_id: Some("male_north".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.1),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = ZaloTts::from_standard(&std).unwrap();
        assert!((tts.config.speed - 1.1).abs() < f32::EPSILON);
        assert_eq!(tts.config.voice, ZaloVoice::MaleNorth);
        assert_eq!(tts.config.api_key, "test_key");
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = TTSConfig {
            provider: "zalo-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = ZaloTts::new(config);
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
        let tts = ZaloTts::new(make_test_config()).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "zalo-ai");
        assert_eq!(info["name"], "Zalo AI TTS (VNG Corporation)");
        assert!(info["voices"].as_array().unwrap().len() == 4);
    }

    #[test]
    fn test_default_state() {
        let tts = ZaloTts::default();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_set_voice() {
        let mut tts = ZaloTts::new(make_test_config()).unwrap();
        tts.set_voice(ZaloVoice::MaleNorth);

        assert_eq!(tts.config.voice, ZaloVoice::MaleNorth);
    }

    #[test]
    fn test_set_speed() {
        let mut tts = ZaloTts::new(make_test_config()).unwrap();

        // Normal speed
        tts.set_speed(1.1);
        assert_eq!(tts.config.speed, 1.1);

        // Below minimum - should clamp
        tts.set_speed(0.5);
        assert_eq!(tts.config.speed, super::super::config::MIN_SPEED);

        // Above maximum - should clamp
        tts.set_speed(2.0);
        assert_eq!(tts.config.speed, super::super::config::MAX_SPEED);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut tts = ZaloTts::new(make_test_config()).unwrap();
        let result = tts.connect().await;

        assert!(result.is_ok());
        assert!(tts.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut tts = ZaloTts::default();
        let result = tts.connect().await;

        assert!(result.is_err());
        match result {
            Err(TTSError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut tts = ZaloTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_speak_empty_text() {
        let mut tts = ZaloTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.speak("", false).await;
        assert!(result.is_ok()); // Empty text should succeed (no-op)
    }

    #[tokio::test]
    async fn test_flush() {
        let mut tts = ZaloTts::new(make_test_config()).unwrap();
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
        let mut tts = ZaloTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.clear().await;
        assert!(result.is_ok()); // Clear is a no-op for REST API
    }

    #[test]
    fn test_voice_parsing() {
        let test_cases = vec![
            ("1", ZaloVoice::FemaleSouth),
            ("2", ZaloVoice::FemaleNorth),
            ("3", ZaloVoice::MaleSouth),
            ("4", ZaloVoice::MaleNorth),
            ("female_south", ZaloVoice::FemaleSouth),
            ("male_north", ZaloVoice::MaleNorth),
        ];

        for (voice_id, expected_voice) in test_cases {
            let config = TTSConfig {
                provider: "zalo-ai".to_string(),
                api_key: "test".to_string(),
                voice_id: Some(voice_id.to_string()),
                ..Default::default()
            };

            let tts = ZaloTts::new(config).unwrap();
            assert_eq!(
                tts.config.voice, expected_voice,
                "Voice ID '{}' should map to {:?}",
                voice_id, expected_voice
            );
        }
    }

    #[test]
    fn test_connection_state() {
        let tts = ZaloTts::new(make_test_config()).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_connection_state_after_connect() {
        let mut tts = ZaloTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);
    }
}
