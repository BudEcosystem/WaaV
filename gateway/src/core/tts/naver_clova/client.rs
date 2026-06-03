//! NAVER CLOVA Voice (TTS Premium) client implementation.
//!
//! This module provides a REST API-based TTS client for the NAVER Cloud Platform
//! CLOVA Voice service.
//!
//! # Protocol
//!
//! CLOVA Voice uses a REST API with form-encoded requests and binary audio response.
//! Each `speak()` call sends an HTTP POST request and receives audio data.

use super::config::{MAX_TEXT_LENGTH, NaverClovaTtsConfig, NaverClovaTtsFormat};
use crate::core::tts::base::{
    AudioCallback, AudioData, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use reqwest::Client;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// NAVER CLOVA Voice TTS client.
///
/// This client implements a REST API-based approach where each `speak()` call
/// sends an HTTP request and receives audio data.
pub struct NaverClovaTts {
    /// Provider configuration.
    config: NaverClovaTtsConfig,
    /// HTTP client for REST API calls.
    http_client: Client,
    /// Whether the client is ready to receive requests.
    is_ready: AtomicBool,
    /// Audio callback for streaming audio to caller.
    audio_callback: Arc<RwLock<Option<Arc<dyn AudioCallback>>>>,
    /// Current connection state.
    connection_state: Arc<RwLock<ConnectionState>>,
}

impl NaverClovaTts {
    /// Build from the standardized config (W1 keystone for TTS), mirroring
    /// `DeepgramTTS::from_standard`. Delegates to the config-level
    /// [`NaverClovaTtsConfig::from_standard`] (which maps `speed`/`pitch`/`volume` onto CLOVA's
    /// -5..=5 integer prosody deltas and reads the non-standard `custom_endpoint` extra) so the
    /// mapped prosody is honored end-to-end through the dispatch path, then assembles the REST
    /// client exactly like [`BaseTTS::new`].
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let naver_config = NaverClovaTtsConfig::from_standard(std)?;

        let timeout_secs = naver_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        Ok(Self {
            config: naver_config,
            http_client,
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        })
    }

    /// Synthesize text and return audio data.
    async fn synthesize(&self, text: &str) -> TTSResult<Vec<u8>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }

        // Truncate text if too long
        let text_to_send = if text.len() > MAX_TEXT_LENGTH {
            warn!(
                "NAVER CLOVA TTS: Text truncated from {} to {} characters",
                text.len(),
                MAX_TEXT_LENGTH
            );
            &text[..MAX_TEXT_LENGTH]
        } else {
            text
        };

        let request_body = self.config.build_request_body(text_to_send);
        let endpoint = self.config.get_endpoint_url();

        debug!(
            "NAVER CLOVA TTS: Synthesizing {} characters with voice '{}'",
            text_to_send.len(),
            self.config.voice.speaker_id()
        );

        let response = self
            .http_client
            .post(endpoint)
            .header("X-NCP-APIGW-API-KEY-ID", &self.config.client_id)
            .header("X-NCP-APIGW-API-KEY", &self.config.client_secret)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(request_body)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to send request: {e}")))?;

        let status = response.status();

        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "Unable to read error body".to_string());

            error!(
                "NAVER CLOVA TTS: API error (status {}): {}",
                status.as_u16(),
                body
            );

            return match status.as_u16() {
                401 => Err(TTSError::AuthenticationFailed(format!(
                    "Invalid credentials: {body}"
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

        // Get the audio data
        let audio_data = response
            .bytes()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to read response body: {e}")))?
            .to_vec();

        info!(
            "NAVER CLOVA TTS: Received {} bytes of audio",
            audio_data.len()
        );

        Ok(audio_data)
    }
}

#[async_trait::async_trait]
impl BaseTTS for NaverClovaTts {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        let naver_config = NaverClovaTtsConfig::from_base(config.clone())?;

        let timeout_secs = naver_config.request_timeout_secs;
        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                TTSError::InvalidConfiguration(format!("Failed to create HTTP client: {e}"))
            })?;

        info!(
            "NAVER CLOVA TTS: Initialized with voice='{}', format='{}'",
            naver_config.voice.speaker_id(),
            naver_config.format.api_format()
        );

        Ok(Self {
            config: naver_config,
            http_client,
            is_ready: AtomicBool::new(false),
            audio_callback: Arc::new(RwLock::new(None)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
        })
    }

    async fn connect(&mut self) -> TTSResult<()> {
        // Validate credentials
        if self.config.client_id.is_empty() || self.config.client_secret.is_empty() {
            return Err(TTSError::AuthenticationFailed(
                "Client ID and Client Secret are required".to_string(),
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

        info!("NAVER CLOVA TTS: Ready to synthesize");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.is_ready.store(false, Ordering::SeqCst);

        {
            let mut state = self.connection_state.write().await;
            *state = ConnectionState::Disconnected;
        }

        info!("NAVER CLOVA TTS: Disconnected");
        Ok(())
    }

    fn is_ready(&self) -> bool {
        self.is_ready.load(Ordering::SeqCst)
    }

    fn get_connection_state(&self) -> ConnectionState {
        // Since we can't await in a sync function, return a default
        // The actual state is managed internally
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
            let format_str = self.config.format.api_format().to_string();
            let sample_rate = match self.config.format {
                NaverClovaTtsFormat::Mp3 => 24000, // MP3 default
                NaverClovaTtsFormat::Wav => 24000, // WAV default
            };

            let data = AudioData {
                data: audio_data,
                sample_rate,
                format: format_str,
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
            "provider": "naver-clova",
            "name": "NAVER CLOVA Voice",
            "version": "1.0.0",
            "voice": self.config.voice.speaker_id(),
            "format": self.config.format.api_format(),
            "supported_formats": ["mp3", "wav"],
            "supported_languages": ["ko", "en", "ja", "zh", "es"],
            "max_text_length": MAX_TEXT_LENGTH,
            "features": {
                "volume_control": true,
                "speed_control": true,
                "pitch_control": true,
                "emotion_support": true
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;

    fn make_test_config() -> TTSConfig {
        TTSConfig {
            provider: "naver-clova".to_string(),
            api_key: "test_client_id|test_client_secret".to_string(),
            voice_id: Some("nara".to_string()),
            audio_format: Some("mp3".to_string()),
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

        fn get_audio_count(&self) -> usize {
            self.audio_count.load(Ordering::SeqCst)
        }

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
        let result = NaverClovaTts::new(config);
        assert!(result.is_ok());

        let tts = result.unwrap();
        assert!(!tts.is_ready());
    }

    // W1 keystone: the struct-level `from_standard` (the dispatch entry point, mirroring
    // `DeepgramTTS::from_standard`) must carry the standardized advanced features onto the live
    // client's CLOVA config — not just the config-level helper. Asserts speed/pitch/volume reach
    // the integer prosody deltas plus the `custom_endpoint` extra.
    #[test]
    fn from_standard_reaches_provider_config() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert(
            "custom_endpoint".into(),
            serde_json::json!("https://enterprise.example.com/tts"),
        );
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "naver-clova".into(),
                api_key: "client_id|client_secret".into(),
                voice_id: Some("clara".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(4.0), // max multiplier -> +5
                pitch: Some(3.0),
                volume: Some(-2.0),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };

        let tts = NaverClovaTts::from_standard(&std).unwrap();
        assert_eq!(tts.config.speed, 5); // 4.0x -> +5
        assert_eq!(tts.config.pitch, 3);
        assert_eq!(tts.config.volume, -2);
        assert_eq!(tts.config.voice.speaker_id(), "clara"); // base carried through
        assert_eq!(
            tts.config.custom_endpoint,
            Some("https://enterprise.example.com/tts".to_string())
        );
    }

    // WIRE-LEVEL (recurring bug class: assert the request body, not the config struct): the
    // standardized `sample_rate` feature must reach the `sampling-rate` form param in the body
    // POSTed to tts-premium/v1/tts — the exact form `synthesize` sends via `build_request_body`.
    // CLOVA Voice Premium honors `sampling-rate` only for WAV output, so the request uses WAV.
    #[test]
    fn from_standard_sample_rate_reaches_form_body_sampling_rate() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};

        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "naver-clova".into(),
                api_key: "client_id|client_secret".into(),
                voice_id: Some("clara".into()),
                audio_format: Some("wav".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(48000),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };

        let tts = NaverClovaTts::from_standard(&std).unwrap();
        // The EXACT form-encoded body the live `synthesize` path posts to the synth endpoint.
        let body = tts.config.build_request_body("Hello world");
        assert!(
            body.contains("sampling-rate=48000"),
            "sample_rate must reach the tts-premium/v1/tts `sampling-rate` form param, got: {body}"
        );
    }

    #[test]
    fn test_new_invalid_api_key() {
        let config = TTSConfig {
            api_key: "invalid_format".to_string(),
            ..make_test_config()
        };

        let result = NaverClovaTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_empty_api_key() {
        let config = TTSConfig {
            api_key: String::new(),
            ..make_test_config()
        };

        let result = NaverClovaTts::new(config);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connect_disconnect() {
        let config = make_test_config();
        let mut tts = NaverClovaTts::new(config).unwrap();

        assert!(!tts.is_ready());

        tts.connect().await.unwrap();
        assert!(tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);

        tts.disconnect().await.unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_callback_registration() {
        let config = make_test_config();
        let mut tts = NaverClovaTts::new(config).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        assert!(tts.on_audio(callback.clone()).is_ok());

        // Give the spawned task time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        // Verify callback is registered
        let guard = tts.audio_callback.read().await;
        assert!(guard.is_some());
    }

    #[tokio::test]
    async fn test_remove_callback() {
        let config = make_test_config();
        let mut tts = NaverClovaTts::new(config).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback).unwrap();

        // Give the spawned task time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        tts.remove_audio_callback().unwrap();

        // Give the spawned task time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        let guard = tts.audio_callback.read().await;
        assert!(guard.is_none());
    }

    #[test]
    fn test_provider_info() {
        let config = make_test_config();
        let tts = NaverClovaTts::new(config).unwrap();

        let info = tts.get_provider_info();
        assert_eq!(info["provider"], "naver-clova");
        assert_eq!(info["voice"], "nara");
        assert_eq!(info["format"], "mp3");
    }

    #[tokio::test]
    async fn test_clear_and_flush() {
        let config = make_test_config();
        let mut tts = NaverClovaTts::new(config).unwrap();

        tts.connect().await.unwrap();

        // These should not fail for REST API
        assert!(tts.clear().await.is_ok());
        assert!(tts.flush().await.is_ok());
    }

    #[tokio::test]
    async fn test_flush_triggers_complete() {
        let config = make_test_config();
        let mut tts = NaverClovaTts::new(config).unwrap();

        let callback = Arc::new(MockAudioCallback::new());
        tts.on_audio(callback.clone()).unwrap();

        // Give the spawned task time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

        tts.connect().await.unwrap();
        tts.flush().await.unwrap();

        // Flush should trigger on_complete
        assert_eq!(callback.get_complete_count(), 1);
    }

    #[test]
    fn test_config_voice_parsing() {
        let config = TTSConfig {
            api_key: "id|secret".to_string(),
            voice_id: Some("clara".to_string()),
            ..Default::default()
        };

        let tts = NaverClovaTts::new(config).unwrap();
        assert_eq!(tts.config.voice.speaker_id(), "clara");
    }

    #[test]
    fn test_config_format_parsing() {
        let config = TTSConfig {
            api_key: "id|secret".to_string(),
            audio_format: Some("wav".to_string()),
            ..Default::default()
        };

        let tts = NaverClovaTts::new(config).unwrap();
        assert_eq!(tts.config.format, NaverClovaTtsFormat::Wav);
    }
}
