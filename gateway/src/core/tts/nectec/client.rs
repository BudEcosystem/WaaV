//! NECTEC (AI for Thai) TTS client implementation.
//!
//! This module provides a REST API-based TTS client for Thailand's NECTEC
//! AI for Thai Text-to-Speech service (VAJA9).
//!
//! # Protocol
//!
//! NECTEC TTS uses a two-step HTTP process:
//! 1. POST JSON with text and voice selection to get audio URL
//! 2. GET the audio file from the returned URL
//!
//! # Audio Output
//!
//! - Format: WAV
//! - Sample Rate: Typically 22050 Hz
//! - Encoding: PCM 16-bit

use super::config::{
    API_KEY_HEADER, DEFAULT_SAMPLE_RATE, LIB_HEADER, LIB_VALUE, MAX_TEXT_LENGTH, NectecTtsConfig,
    NectecVoice, Vaja9Request, Vaja9Response, chunk_text,
};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// NECTEC TTS client.
///
/// This client implements a REST API-based approach where each `speak()` call:
/// 1. Sends an HTTP POST to get an audio URL
/// 2. Downloads the audio from the URL
/// 3. Sends audio data via callback
pub struct NectecTts {
    /// Provider configuration.
    config: NectecTtsConfig,
    /// HTTP client for REST API calls.
    http_client: Client,
    /// Whether the client is ready to receive requests.
    is_ready: AtomicBool,
    /// Audio callback for streaming audio to caller.
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Current connection state.
    connection_state: Arc<RwLock<ConnectionState>>,
}

impl NectecTts {
    /// Synthesize text and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // Handle text chunking if needed
        let chunks = if self.config.auto_chunk && text.len() > MAX_TEXT_LENGTH {
            chunk_text(text, MAX_TEXT_LENGTH)
        } else if text.len() > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text too long ({} chars), max is {} chars",
                text.len(),
                MAX_TEXT_LENGTH
            )));
        } else {
            vec![text.to_string()]
        };

        let mut all_audio = Vec::new();

        for (i, chunk) in chunks.iter().enumerate() {
            debug!(
                "NECTEC TTS: Processing chunk {}/{}: {} chars",
                i + 1,
                chunks.len(),
                chunk.len()
            );

            let audio = self.synthesize_chunk(chunk).await?;
            all_audio.extend(audio);
        }

        Ok(all_audio)
    }

    /// Synthesize a single chunk of text.
    async fn synthesize_chunk(&self, text: &str) -> TTSResult<Vec<u8>> {
        // Step 1: Request synthesis
        let request = Vaja9Request::new(text, self.config.voice);

        debug!(
            "NECTEC TTS: Synthesizing {} characters with voice '{}'",
            text.len(),
            self.config.voice
        );

        let response = self
            .http_client
            .post(self.config.endpoint())
            .header(API_KEY_HEADER, &self.config.api_key)
            .header(LIB_HEADER, LIB_VALUE)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to send TTS request: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!(
                "NECTEC TTS: API error (status {}): {}",
                status.as_u16(),
                body
            );

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

        // Parse response to get audio URL
        let api_response: Vaja9Response = response
            .json()
            .await
            .map_err(|e| TTSError::ProviderError(format!("Failed to parse API response: {e}")))?;

        if api_response.has_error() {
            let error_msg = api_response.error.as_deref().unwrap_or("Unknown error");
            error!("NECTEC TTS: API error: {}", error_msg);
            return Err(TTSError::ProviderError(error_msg.to_string()));
        }

        let audio_url = api_response
            .audio_url()
            .ok_or_else(|| TTSError::ProviderError("No audio URL in response".to_string()))?;

        debug!("NECTEC TTS: Got audio URL: {}", audio_url);

        // Step 2: Download audio
        self.download_audio(audio_url).await
    }

    /// Download audio from the given URL.
    async fn download_audio(&self, url: &str) -> TTSResult<Vec<u8>> {
        let response = self
            .http_client
            .get(url)
            .header(API_KEY_HEADER, &self.config.api_key)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to download audio: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!(
                "NECTEC TTS: Audio download error (status {}): {}",
                status.as_u16(),
                body
            );

            return Err(TTSError::ProviderError(format!(
                "Audio download failed (status {}): {}",
                status.as_u16(),
                body
            )));
        }

        let audio_data = response
            .bytes()
            .await
            .map_err(|e| TTSError::ProviderError(format!("Failed to read audio data: {e}")))?
            .to_vec();

        info!("NECTEC TTS: Downloaded {} bytes of audio", audio_data.len());

        Ok(audio_data)
    }

    /// Get the NECTEC-specific configuration.
    pub fn get_nectec_config(&self) -> &NectecTtsConfig {
        &self.config
    }

    /// Set the voice.
    pub fn set_voice(&mut self, voice: NectecVoice) {
        self.config.voice = voice;
    }
}

impl Default for NectecTts {
    fn default() -> Self {
        Self {
            config: NectecTtsConfig::default(),
            http_client: Client::new(),
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        }
    }
}

#[async_trait::async_trait]
impl BaseTTS for NectecTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let nectec_config =
            NectecTtsConfig::from_base(&config).map_err(|e| TTSError::InvalidConfiguration(e))?;

        let timeout_secs = nectec_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "NECTEC TTS: Initialized with voice='{}'",
            nectec_config.voice
        );

        Ok(Self {
            config: nectec_config,
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
                "NECTEC API key is required".to_string(),
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

        info!("NECTEC TTS: Ready (voice: {})", self.config.voice);
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        info!("NECTEC TTS: Disconnected");
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
            "provider": "nectec",
            "name": "NECTEC AI for Thai (VAJA9)",
            "version": "1.0.0",
            "description": "Thailand government-backed Thai language text-to-speech",
            "voice": self.config.voice.speaker_id(),
            "voice_name": self.config.voice.as_str(),
            "supported_formats": ["wav"],
            "supported_languages": ["th"],
            "sample_rate": DEFAULT_SAMPLE_RATE,
            "max_text_length": MAX_TEXT_LENGTH,
            "auto_chunk": self.config.auto_chunk,
            "features": {
                "auto_chunking": true,
                "male_voice": true,
                "female_voice": true
            },
            "voices": [
                {"id": "0", "name": "Male Thai", "gender": "male"},
                {"id": "1", "name": "Female Thai", "gender": "female"}
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
            provider: "nectec".to_string(),
            api_key: "test_api_key".to_string(),
            voice_id: Some("female".to_string()),
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
    fn test_default() {
        let tts = NectecTts::default();
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_new_valid_config() {
        let config = make_test_config();
        let result = NectecTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.config.voice, NectecVoice::Female);
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = TTSConfig {
            provider: "nectec".to_string(),
            api_key: String::new(),
            ..Default::default()
        };

        let result = NectecTts::new(config);
        assert!(result.is_err());

        match result {
            Err(TTSError::InvalidConfiguration(msg)) => {
                assert!(msg.contains("API key"));
            }
            _ => panic!("Expected InvalidConfiguration error"),
        }
    }

    #[test]
    fn test_new_with_male_voice() {
        let config = TTSConfig {
            provider: "nectec".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("male".to_string()),
            ..Default::default()
        };

        let result = NectecTts::new(config);
        assert!(result.is_ok());
        let tts = result.unwrap();
        assert_eq!(tts.config.voice, NectecVoice::Male);
    }

    #[test]
    fn test_provider_info() {
        let tts = NectecTts::new(make_test_config()).unwrap();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "nectec");
        assert_eq!(info["name"], "NECTEC AI for Thai (VAJA9)");
        assert!(info["voices"].as_array().unwrap().len() == 2);
        assert!(info.get("max_text_length").is_some());
        assert_eq!(info["supported_languages"][0], "th");
    }

    #[test]
    fn test_default_state() {
        let tts = NectecTts::default();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_set_voice() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();
        assert_eq!(tts.config.voice, NectecVoice::Female);

        tts.set_voice(NectecVoice::Male);
        assert_eq!(tts.config.voice, NectecVoice::Male);
    }

    #[test]
    fn test_get_nectec_config() {
        let tts = NectecTts::new(make_test_config()).unwrap();
        let config = tts.get_nectec_config();

        assert_eq!(config.api_key, "test_api_key");
        assert_eq!(config.voice, NectecVoice::Female);
    }

    #[tokio::test]
    async fn test_connect_success() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();
        let result = tts.connect().await;

        assert!(result.is_ok());
        assert!(tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);
    }

    #[tokio::test]
    async fn test_connect_empty_api_key() {
        let mut tts = NectecTts::default();
        let result = tts.connect().await;

        assert!(result.is_err());
        match result {
            Err(TTSError::AuthenticationFailed(_)) => {}
            _ => panic!("Expected AuthenticationFailed error"),
        }
    }

    #[tokio::test]
    async fn test_disconnect() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();
        assert!(tts.is_ready());

        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_speak_empty_text() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.speak("", false).await;
        assert!(result.is_ok()); // Empty text should succeed (no-op)
    }

    #[tokio::test]
    async fn test_flush() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();
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
        let mut tts = NectecTts::new(make_test_config()).unwrap();
        tts.connect().await.unwrap();

        let result = tts.clear().await;
        assert!(result.is_ok()); // Clear is a no-op for REST API
    }

    #[test]
    fn test_voice_parsing() {
        let test_cases = vec![
            ("male", NectecVoice::Male),
            ("female", NectecVoice::Female),
            ("0", NectecVoice::Male),
            ("1", NectecVoice::Female),
            ("m", NectecVoice::Male),
            ("f", NectecVoice::Female),
        ];

        for (voice_id, expected_voice) in test_cases {
            let config = TTSConfig {
                provider: "nectec".to_string(),
                api_key: "test".to_string(),
                voice_id: Some(voice_id.to_string()),
                ..Default::default()
            };

            let tts = NectecTts::new(config).unwrap();
            assert_eq!(
                tts.config.voice, expected_voice,
                "Voice ID '{}' should map to {:?}",
                voice_id, expected_voice
            );
        }
    }

    #[tokio::test]
    async fn test_on_audio_callback_registration() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        let result = tts.on_audio(callback);
        assert!(result.is_ok());

        // Give time for the async callback registration
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify callback was registered
        let guard = tts.audio_callback.read().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_remove_audio_callback() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback).unwrap();

        // Give time for the async callback registration
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let result = tts.remove_audio_callback();
        assert!(result.is_ok());

        // Give time for the async callback removal
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify callback was removed
        let guard = tts.audio_callback.read().await;
        assert!(guard.is_none());
    }

    #[tokio::test]
    async fn test_auto_connect_on_speak() {
        let mut tts = NectecTts::new(make_test_config()).unwrap();
        assert!(!tts.is_ready());

        // speak() should auto-connect
        // Note: This will fail network call, but will connect first
        let result = tts.speak("test", false).await;
        // The request will fail (no real API key) but connect should have happened
        assert!(tts.is_ready()); // Should be ready after auto-connect
        assert!(result.is_err()); // Network call fails
    }

    #[test]
    fn test_connection_state_initial() {
        let tts = NectecTts::new(make_test_config()).unwrap();
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }
}
