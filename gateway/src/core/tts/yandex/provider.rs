//! Yandex SpeechKit TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Yandex SpeechKit API v1,
//! using HTTP POST requests for speech synthesis.

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use tracing::{debug, info};

use super::config::YandexTtsConfig;
use super::messages::{YandexApiError, YandexStatusCode, YandexSynthesisResponse};
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Yandex SpeechKit TTS API
#[derive(Clone)]
pub struct YandexRequestBuilder {
    /// Yandex-specific configuration
    yandex_config: YandexTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Pronunciation replacer (precompiled regex patterns)
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl YandexRequestBuilder {
    /// Create a new request builder
    pub fn new(yandex_config: YandexTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config if pronunciations are defined
        let pronunciation_replacer = if base_config.pronunciations.is_empty() {
            None
        } else {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        };

        Self {
            yandex_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the Yandex configuration
    pub fn yandex_config(&self) -> &YandexTtsConfig {
        &self.yandex_config
    }
}

impl TTSRequestBuilder for YandexRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build headers
        let mut headers = HeaderMap::new();

        // Authorization header
        if let Ok(auth) = HeaderValue::from_str(&self.yandex_config.auth_header_value()) {
            headers.insert(AUTHORIZATION, auth);
        }

        // Content-Type for form data
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );

        // Build form parameters
        let form_params = self.yandex_config.build_form_params(text);

        debug!(
            "Yandex TTS request: voice={}, lang={}, format={}, text_len={}",
            self.yandex_config.voice,
            self.yandex_config.language,
            self.yandex_config.audio_format,
            text.len()
        );

        // Build and return the request
        client
            .post(crate::core::tts::standard::override_rest_endpoint(
                super::YANDEX_TTS_SYNTHESIZE_URL,
                self.yandex_config.endpoint_override.as_deref(),
            ))
            .headers(headers)
            .form(&form_params)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.base_config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Yandex TTS Provider
// =============================================================================

/// Yandex SpeechKit TTS provider
///
/// Implements text-to-speech synthesis using the Yandex SpeechKit API v1.
/// Uses HTTP POST requests with form-urlencoded parameters.
pub struct YandexTts {
    /// Generic TTS provider for HTTP connection management
    provider: TTSProvider,
    /// Yandex-specific configuration
    yandex_config: YandexTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl YandexTts {
    /// Create a new Yandex TTS provider from TTSConfig
    pub fn create(config: TTSConfig) -> TTSResult<Self> {
        let yandex_config = YandexTtsConfig::from_base(&config)?;

        info!(
            "Creating Yandex TTS provider: voice={}, lang={}, format={}",
            yandex_config.voice, yandex_config.language, yandex_config.audio_format
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            yandex_config,
            base_config: config,
        })
    }

    /// Build from the standardized TTS config (W1 keystone). Mirrors [`Self::create`] but derives
    /// the Yandex config via [`YandexTtsConfig::from_standard`] (which honors speed, emotion,
    /// language, output sample rate, and the folder_id / is_iam_token extras) and keeps the
    /// standardized base config as `base_config`. Features without a Yandex field stay at provider
    /// defaults (capability gaps).
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let yandex_config = YandexTtsConfig::from_standard(std)?;

        info!(
            "Creating Yandex TTS provider (standardized): voice={}, lang={}, format={}",
            yandex_config.voice, yandex_config.language, yandex_config.audio_format
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            yandex_config,
            base_config: std.base.clone(),
        })
    }

    /// Create a request builder
    fn create_request_builder(&self) -> YandexRequestBuilder {
        YandexRequestBuilder::new(self.yandex_config.clone(), self.base_config.clone())
    }

    /// Synthesize text to audio using direct HTTP request
    ///
    /// This method provides direct HTTP synthesis without going through the generic provider.
    /// Useful for testing or when direct control over the request/response is needed.
    #[allow(dead_code)]
    async fn synthesize_http(&self, text: &str) -> TTSResult<YandexSynthesisResponse> {
        let client = reqwest::Client::new();
        let request_builder = self.create_request_builder();

        let response = request_builder
            .build_http_request(&client, text)
            .send()
            .await
            .map_err(|e| TTSError::ConnectionFailed(format!("Failed to send request: {}", e)))?;

        let status = response.status();
        let status_code = YandexStatusCode::from_http_status(status.as_u16());

        if status_code.is_success() {
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("audio/ogg")
                .to_string();

            let audio_data = response
                .bytes()
                .await
                .map_err(|e| TTSError::NetworkError(format!("Failed to read response: {}", e)))?
                .to_vec();

            if audio_data.is_empty() {
                return Err(TTSError::AudioGenerationFailed(
                    "Empty audio response".to_string(),
                ));
            }

            Ok(YandexSynthesisResponse::new(audio_data, content_type))
        } else {
            let body = response.text().await.unwrap_or_default();
            let error = YandexApiError::from_response(status.as_u16(), &body);

            if error.is_auth_error() {
                Err(TTSError::AuthenticationFailed(error.display_message()))
            } else if error.is_rate_limit_error() {
                Err(TTSError::RateLimited {
                    retry_after_secs: None,
                    message: error.display_message(),
                })
            } else {
                Err(TTSError::ProviderError(error.display_message()))
            }
        }
    }
}

// =============================================================================
// BaseTTS Implementation
// =============================================================================

#[async_trait]
impl BaseTTS for YandexTts {
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        Self::create(config)
    }

    /// Expose the inner generic provider so the default `on_audio`/`remove_audio_callback`
    /// trait methods operate on it. Without this override `get_provider` defaults to `None`, so
    /// audio-callback registration silently fails and synthesized audio is never delivered.
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the Yandex SpeechKit API
    async fn connect(&mut self) -> TTSResult<()> {
        debug!("Connecting to Yandex SpeechKit TTS API");

        // Initialize HTTP connection pool
        self.provider
            .generic_connect_with_config(super::YANDEX_TTS_SYNTHESIZE_URL, &self.base_config)
            .await
    }

    /// Disconnect from the Yandex SpeechKit API
    async fn disconnect(&mut self) -> TTSResult<()> {
        debug!("Disconnecting from Yandex SpeechKit TTS API");
        self.provider.generic_disconnect().await
    }

    /// Synthesize text to speech
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if text.is_empty() || text.trim().is_empty() {
            return Ok(());
        }

        // Check if provider is ready (connected)
        if !self.is_ready() {
            return Err(TTSError::ProviderNotReady(
                "Provider not connected. Call connect() first.".to_string(),
            ));
        }

        // Validate text length
        self.yandex_config.validate_text(text)?;

        debug!(
            "Yandex TTS speak: text='{}...', flush={}, voice={}, format={}",
            &text[..text.len().min(50)],
            flush,
            self.yandex_config.voice,
            self.yandex_config.audio_format
        );

        // Create request builder
        let request_builder = self.create_request_builder();

        // Execute request through generic provider
        self.provider
            .generic_speak(request_builder, text, flush)
            .await
    }

    /// Flush any pending synthesis
    async fn flush(&self) -> TTSResult<()> {
        self.provider.generic_flush().await
    }

    /// Check if the provider is ready for synthesis
    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    /// Get the current connection state
    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::yandex::YandexAudioFormat;

    #[test]
    fn test_yandex_tts_creation() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("alena".to_string()),
            audio_format: None, // Explicitly test Yandex default (OggOpus)
            ..Default::default()
        };

        let tts = YandexTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.yandex_config.voice.as_str(), "alena");
        assert_eq!(tts.yandex_config.audio_format, YandexAudioFormat::OggOpus);
    }

    // W1 keystone (TTS): the struct-level `from_standard` builds a real `YandexTts` through the
    // standardized path, carrying the speed/emotion features Yandex can express onto the provider
    // config the request builder reads. Mirrors `DeepgramTTS::from_standard`.
    #[test]
    fn from_standard_builds_provider_with_speed_and_emotion() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        use crate::core::tts::yandex::YandexEmotion;
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "yandex".into(),
                api_key: "AQVN1234567890".into(),
                voice_id: Some("alena".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                emotion: Some("cheerful".into()),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = YandexTts::from_standard(&std).unwrap();
        assert_eq!(tts.yandex_config.speed, 1.5);
        assert_eq!(tts.yandex_config.emotion, YandexEmotion::Good);
        assert_eq!(tts.yandex_config.api_key, "AQVN1234567890");
    }

    #[test]
    fn test_yandex_tts_requires_api_key() {
        let config = TTSConfig {
            api_key: String::new(),
            voice_id: Some("alena".to_string()),
            ..Default::default()
        };

        let result = YandexTts::new(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_yandex_tts_with_folder_id() {
        let config = TTSConfig {
            api_key: "b1g12345:AQVN1234567890".to_string(),
            voice_id: Some("john".to_string()),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let tts = YandexTts::new(config).unwrap();
        assert_eq!(tts.yandex_config.folder_id, Some("b1g12345".to_string()));
        assert_eq!(tts.yandex_config.api_key, "AQVN1234567890");
        assert_eq!(tts.yandex_config.voice.as_str(), "john");
        assert_eq!(tts.yandex_config.audio_format, YandexAudioFormat::Mp3);
    }

    #[tokio::test]
    async fn test_yandex_tts_not_connected() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("alena".to_string()),
            ..Default::default()
        };

        let mut tts = YandexTts::new(config).unwrap();

        // Should not be ready initially
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);

        // Should fail when not connected
        let result = tts.speak("Hello", true).await;
        assert!(result.is_err());

        if let Err(TTSError::ProviderNotReady(_)) = result {
            // Expected
        } else {
            panic!("Expected ProviderNotReady error, got {:?}", result);
        }
    }

    #[tokio::test]
    async fn test_yandex_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("alena".to_string()),
            ..Default::default()
        };

        let mut tts = YandexTts::new(config).unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_request_builder() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("alena".to_string()),
            ..Default::default()
        };

        let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
        let builder = YandexRequestBuilder::new(yandex_config, config.clone());

        // Verify get_config works
        assert_eq!(builder.get_config().api_key, config.api_key);
    }

    #[test]
    fn test_request_builder_with_pronunciations() {
        use crate::core::tts::base::Pronunciation;

        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("alena".to_string()),
            pronunciations: vec![Pronunciation {
                word: "API".to_string(),
                pronunciation: "A P I".to_string(),
            }],
            ..Default::default()
        };

        let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
        let builder = YandexRequestBuilder::new(yandex_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[test]
    fn test_config_with_all_audio_formats() {
        for (format_str, expected_format) in [
            ("lpcm", YandexAudioFormat::Lpcm),
            ("oggopus", YandexAudioFormat::OggOpus),
            ("mp3", YandexAudioFormat::Mp3),
        ] {
            let config = TTSConfig {
                api_key: "test-api-key".to_string(),
                audio_format: Some(format_str.to_string()),
                voice_id: None,
                sample_rate: None,
                ..Default::default()
            };

            let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
            assert_eq!(yandex_config.audio_format, expected_format);
        }
    }

    #[test]
    fn test_config_with_speed() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            speaking_rate: Some(2.0),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };

        let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
        assert_eq!(yandex_config.speed, 2.0);
    }

    #[test]
    fn test_config_speed_clamping() {
        // Test minimum clamping
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            speaking_rate: Some(0.01),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };
        let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
        assert_eq!(yandex_config.speed, 0.1);

        // Test maximum clamping
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            speaking_rate: Some(5.0),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };
        let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
        assert_eq!(yandex_config.speed, 3.0);
    }

    #[test]
    fn test_auth_header_api_key() {
        let config = TTSConfig {
            api_key: "AQVN1234567890".to_string(),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };

        let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
        assert_eq!(yandex_config.auth_header_value(), "Api-Key AQVN1234567890");
    }

    #[test]
    fn test_form_params_contain_required_fields() {
        let config = TTSConfig {
            api_key: "b1g12345:AQVN1234567890".to_string(),
            voice_id: Some("john".to_string()),
            audio_format: Some("mp3".to_string()),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let yandex_config = YandexTtsConfig::from_base(&config).unwrap();
        let params = yandex_config.build_form_params("Hello world");

        // Check required parameters
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "text" && v == "Hello world")
        );
        assert!(params.iter().any(|(k, v)| *k == "voice" && v == "john"));
        assert!(params.iter().any(|(k, v)| *k == "format" && v == "mp3"));
        assert!(params.iter().any(|(k, v)| *k == "lang" && v == "en-US"));
        assert!(params.iter().any(|(k, v)| *k == "speed" && v == "1.5"));
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "folderId" && v == "b1g12345")
        );
    }
}
