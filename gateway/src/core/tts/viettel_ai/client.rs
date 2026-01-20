//! Viettel AI TTS client implementation.
//!
//! This module provides a REST API-based TTS client for Viettel Group's
//! AI Text-to-Speech service.
//!
//! # Protocol
//!
//! Viettel AI TTS uses REST API:
//! 1. POST JSON request with text, voice, speed parameters
//! 2. Receive WAV audio binary response directly
//!
//! # Audio Format
//!
//! Output is WAV format at 16kHz sample rate.

use super::config::{
    ViettelTtsConfig, ViettelTtsRequest, ViettelVoice, AUDIO_SAMPLE_RATE, MAX_SPEED, MIN_SPEED,
    VIETTEL_TTS_ENDPOINT,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use reqwest::Client;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Viettel AI TTS client.
///
/// This client implements a REST API-based approach where each `speak()` call:
/// 1. Sends an HTTP POST with JSON body containing text and parameters
/// 2. Receives WAV audio binary response directly
pub struct ViettelTts {
    /// Provider configuration.
    config: ViettelTtsConfig,
    /// HTTP client for REST API calls.
    http_client: Client,
    /// Whether the client is ready to receive requests.
    is_ready: AtomicBool,
    /// Audio callback for streaming audio to caller.
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Current connection state.
    connection_state: Arc<RwLock<ConnectionState>>,
    /// Request counter for generating unique IDs.
    request_counter: AtomicU64,
}

impl ViettelTts {
    /// Synthesize text and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // Validate text length
        ViettelTtsConfig::validate_text(text)?;

        debug!(
            "Viettel TTS: Synthesizing {} characters with voice '{}' (speed: {})",
            text.chars().count(),
            self.config.voice.display_name(),
            self.config.speed
        );

        // Generate unique request ID
        let request_id = self.request_counter.fetch_add(1, Ordering::SeqCst);

        // Build request body
        let request_body = ViettelTtsRequest {
            text: text.to_string(),
            voice: self.config.voice.voice_id().to_string(),
            id: request_id.to_string(),
            without_filter: self.config.without_filter,
            speed: self.config.speed,
            tts_return_option: self.config.tts_return_option,
        };

        // Send request
        let response = self
            .http_client
            .post(VIETTEL_TTS_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("token", &self.config.api_key)
            .json(&request_body)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to send TTS request: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!("Viettel TTS: API error (status {}): {}", status.as_u16(), body);

            return match status.as_u16() {
                401 => Err(TTSError::AuthenticationFailed(format!(
                    "Invalid or expired token: {body}"
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

        // Response is binary audio data (WAV)
        let audio_data = response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read audio data: {e}")))?
            .to_vec();

        // Verify we got actual audio data
        if audio_data.is_empty() {
            return Err(TTSError::ProviderError(
                "Received empty audio data".to_string(),
            ));
        }

        // Check if response is JSON error (not audio)
        if audio_data.starts_with(b"{") {
            // Try to parse as error JSON
            if let Ok(error_str) = std::str::from_utf8(&audio_data) {
                if error_str.contains("error") || error_str.contains("message") {
                    return Err(TTSError::ProviderError(format!(
                        "API returned error: {}",
                        error_str
                    )));
                }
            }
        }

        info!("Viettel TTS: Received {} bytes of audio", audio_data.len());

        Ok(audio_data)
    }

    /// Get the Viettel-specific configuration.
    pub fn get_viettel_config(&self) -> &ViettelTtsConfig {
        &self.config
    }

    /// Set the voice.
    pub fn set_voice(&mut self, voice: ViettelVoice) {
        self.config.voice = voice;
    }

    /// Set the speech speed.
    pub fn set_speed(&mut self, speed: f32) {
        self.config.speed = speed.clamp(MIN_SPEED, MAX_SPEED);
    }

    /// Set the without_filter option.
    pub fn set_without_filter(&mut self, without_filter: bool) {
        self.config.without_filter = without_filter;
    }
}

impl Default for ViettelTts {
    fn default() -> Self {
        Self {
            config: ViettelTtsConfig::default(),
            http_client: Client::new(),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            request_counter: AtomicU64::new(1),
        }
    }
}

#[async_trait::async_trait]
impl BaseTTS for ViettelTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let viettel_config = ViettelTtsConfig::from_base(config)?;

        let timeout_secs = viettel_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "Viettel TTS: Initialized with voice='{}', speed={}",
            viettel_config.voice.display_name(),
            viettel_config.speed
        );

        Ok(Self {
            config: viettel_config,
            http_client,
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            request_counter: AtomicU64::new(1),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        // Validate credentials
        if self.config.api_key.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "Viettel AI API token is required".to_string(),
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

        info!("Viettel TTS: Ready to synthesize");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        info!("Viettel TTS: Disconnected");
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
            "provider": "viettel-ai",
            "name": "Viettel AI TTS (Viettel Group)",
            "version": "1.0.0",
            "voice": self.config.voice.voice_id(),
            "voice_name": self.config.voice.display_name(),
            "speed": self.config.speed,
            "format": "wav",
            "supported_formats": ["wav"],
            "supported_languages": ["vi"],
            "sample_rate": AUDIO_SAMPLE_RATE,
            "features": {
                "speed_control": true,
                "regional_accents": true,
                "northern_accent": true,
                "southern_accent": true,
                "central_accent": true,
                "human_like_quality": "95%"
            },
            "voices": [
                {"id": "doanngocle", "name": "Doan Ngoc Le", "region": "northern", "gender": "female"},
                {"id": "hn_female_ngochuyen_news_48k-fhg", "name": "Ngoc Huyen", "region": "northern", "gender": "female"},
                {"id": "hn_female_thuthao_news_48k-fhg", "name": "Thu Thao", "region": "northern", "gender": "female"},
                {"id": "hn_female_phuongtrang_news_48k-fhg", "name": "Phuong Trang", "region": "northern", "gender": "female"},
                {"id": "hn_male_xuankien_news_48k-fhg", "name": "Xuan Kien", "region": "northern", "gender": "male"},
                {"id": "hn_male_quang_news_48k-fhg", "name": "Quang", "region": "northern", "gender": "male"},
                {"id": "sg_female_thuphuong_news_48k-fhg", "name": "Thu Phuong", "region": "southern", "gender": "female"},
                {"id": "sg_female_minhly_news_48k-fhg", "name": "Minh Ly", "region": "southern", "gender": "female"},
                {"id": "sg_female_huonggiang_news_48k-fhg", "name": "Huong Giang", "region": "southern", "gender": "female"},
                {"id": "sg_male_trongphuc_news_48k-fhg", "name": "Trong Phuc", "region": "southern", "gender": "male"},
                {"id": "hn_female_maiphuong_news_48k-fhg", "name": "Mai Phuong", "region": "central", "gender": "female"},
                {"id": "hn_male_thanhtung_news_48k-fhg", "name": "Thanh Tung", "region": "central", "gender": "male"}
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
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> TTSConfig {
        TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test_token".to_string(),
            voice_id: Some("doanngocle".to_string()),
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
        fn on_audio(&self, _audio_data: AudioData) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
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
        let result = ViettelTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = ViettelTts::new(config);
        assert!(result.is_err());

        match result {
            Err(TTSError::AuthenticationFailed(msg)) => {
                assert!(msg.contains("token"));
            }
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[test]
    fn test_provider_info() {
        let tts = ViettelTts::new(make_test_config()).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "viettel-ai");
        assert_eq!(info["name"], "Viettel AI TTS (Viettel Group)");
        assert_eq!(info["voices"].as_array().unwrap().len(), 12);
    }

    #[test]
    fn test_default_state() {
        let tts = ViettelTts::default();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_set_voice() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.set_voice(ViettelVoice::SouthernMale);

        assert_eq!(tts.config.voice, ViettelVoice::SouthernMale);
    }

    #[test]
    fn test_set_speed() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();

        // Normal speed
        tts.set_speed(1.5);
        assert!((tts.config.speed - 1.5).abs() < f32::EPSILON);

        // Below minimum - should clamp
        tts.set_speed(0.1);
        assert!((tts.config.speed - MIN_SPEED).abs() < f32::EPSILON);

        // Above maximum - should clamp
        tts.set_speed(5.0);
        assert!((tts.config.speed - MAX_SPEED).abs() < f32::EPSILON);
    }

    #[test]
    fn test_set_without_filter() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        assert!(!tts.config.without_filter);

        tts.set_without_filter(true);
        assert!(tts.config.without_filter);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        let result = tts.connect().await;

        assert!(result.is_ok());
        assert!(tts.is_ready());
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut tts = ViettelTts::default();
        let result = tts.connect().await;

        assert!(result.is_err());
        match result {
            Err(TTSError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
    }

    #[tokio::test]
    async fn test_speak_empty_text() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.speak("", false).await;
        assert!(result.is_ok()); // Empty text should succeed (no-op)
    }

    #[tokio::test]
    async fn test_flush() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
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
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.clear().await;
        assert!(result.is_ok()); // Clear is a no-op for REST API
    }

    #[test]
    fn test_voice_parsing() {
        let test_cases = vec![
            ("doanngocle", ViettelVoice::DoanNgocLe),
            ("doan_ngoc_le", ViettelVoice::DoanNgocLe),
            ("default", ViettelVoice::DoanNgocLe),
            ("female", ViettelVoice::DoanNgocLe),
            ("ngochuyen", ViettelVoice::NorthernFemale1),
            ("thuthao", ViettelVoice::NorthernFemale2),
            ("xuankien", ViettelVoice::NorthernMale1),
            ("quang", ViettelVoice::NorthernMale2),
            ("thuphuong", ViettelVoice::SouthernFemale1),
            ("minhly", ViettelVoice::SouthernFemale2),
            ("trongphuc", ViettelVoice::SouthernMale),
            ("maiphuong", ViettelVoice::CentralFemale),
            ("thanhtung", ViettelVoice::CentralMale),
        ];

        for (voice_id, expected_voice) in test_cases {
            let config = TTSConfig {
                provider: "viettel-ai".to_string(),
                api_key: "test".to_string(),
                voice_id: Some(voice_id.to_string()),
                ..Default::default()
            };

            let tts = ViettelTts::new(config).unwrap();
            assert_eq!(
                tts.config.voice, expected_voice,
                "Voice ID '{}' should map to {:?}",
                voice_id, expected_voice
            );
        }
    }

    #[test]
    fn test_custom_voice() {
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            voice_id: Some("some_unknown_voice".to_string()),
            ..Default::default()
        };

        let tts = ViettelTts::new(config).unwrap();
        match &tts.config.voice {
            ViettelVoice::Custom(id) => assert_eq!(id, "some_unknown_voice"),
            _ => panic!("Expected Custom variant"),
        }
    }

    #[test]
    fn test_connection_state() {
        let tts = ViettelTts::new(make_test_config()).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_connection_state_after_connect() {
        let mut tts = ViettelTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);
    }

    #[test]
    fn test_speed_from_speaking_rate() {
        // Normal speed
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(1.0),
            ..Default::default()
        };
        let tts = ViettelTts::new(config).unwrap();
        assert!((tts.config.speed - 1.0).abs() < f32::EPSILON);

        // Fast speed
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(1.8),
            ..Default::default()
        };
        let tts = ViettelTts::new(config).unwrap();
        assert!((tts.config.speed - 1.8).abs() < f32::EPSILON);

        // Slow speed
        let config = TTSConfig {
            provider: "viettel-ai".to_string(),
            api_key: "test".to_string(),
            speaking_rate: Some(0.7),
            ..Default::default()
        };
        let tts = ViettelTts::new(config).unwrap();
        assert!((tts.config.speed - 0.7).abs() < f32::EPSILON);
    }

    #[test]
    fn test_get_viettel_config() {
        let tts = ViettelTts::new(make_test_config()).unwrap();
        let config = tts.get_viettel_config();

        assert_eq!(config.api_key, "test_token");
        assert_eq!(config.voice, ViettelVoice::DoanNgocLe);
    }

    #[test]
    fn test_request_counter() {
        let tts = ViettelTts::new(make_test_config()).unwrap();

        let id1 = tts.request_counter.fetch_add(1, Ordering::SeqCst);
        let id2 = tts.request_counter.fetch_add(1, Ordering::SeqCst);

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
    }

    #[test]
    fn test_voice_regions() {
        assert_eq!(ViettelVoice::DoanNgocLe.region(), "Northern");
        assert_eq!(ViettelVoice::NorthernFemale1.region(), "Northern");
        assert_eq!(ViettelVoice::SouthernFemale1.region(), "Southern");
        assert_eq!(ViettelVoice::SouthernMale.region(), "Southern");
        assert_eq!(ViettelVoice::CentralFemale.region(), "Central");
        assert_eq!(ViettelVoice::CentralMale.region(), "Central");
    }

    #[test]
    fn test_voice_genders() {
        assert_eq!(ViettelVoice::DoanNgocLe.gender(), "female");
        assert_eq!(ViettelVoice::NorthernFemale1.gender(), "female");
        assert_eq!(ViettelVoice::NorthernMale1.gender(), "male");
        assert_eq!(ViettelVoice::SouthernFemale1.gender(), "female");
        assert_eq!(ViettelVoice::SouthernMale.gender(), "male");
        assert_eq!(ViettelVoice::CentralFemale.gender(), "female");
        assert_eq!(ViettelVoice::CentralMale.gender(), "male");
    }
}
