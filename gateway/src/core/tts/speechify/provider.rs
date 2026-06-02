//! Speechify TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Speechify API,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use tracing::{debug, info};

use super::SPEECHIFY_VOICES_URL;
use super::config::{
    SpeechifyStreamRequest, SpeechifyTtsConfig, SpeechifyVoice, SpeechifyVoicesResponse,
};
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Speechify TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for Speechify's streaming API.
#[derive(Clone)]
pub struct SpeechifyRequestBuilder {
    /// Speechify-specific configuration
    config: SpeechifyTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl SpeechifyRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(speechify_config: SpeechifyTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: speechify_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the streaming endpoint URL
    pub fn streaming_url(&self) -> &str {
        super::SPEECHIFY_TTS_STREAM_URL
    }

    /// Get the Speechify configuration
    pub fn speechify_config(&self) -> &SpeechifyTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for SpeechifyRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = SpeechifyStreamRequest::from_config(&self.config, text);

        // Build headers
        let mut headers = HeaderMap::new();

        // Speechify uses Bearer token authentication
        let auth_value = format!("Bearer {}", self.config.api_key);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        debug!(
            "Speechify TTS request: voice={}, model={}, format={}, url={}",
            request_body.voice_id,
            request_body.model.as_deref().unwrap_or("default"),
            request_body.audio_format.as_deref().unwrap_or("wav_48000"),
            self.streaming_url()
        );

        // Build and return the request
        client
            .post(self.streaming_url())
            .headers(headers)
            .json(&request_body)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.base_config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Speechify TTS Provider
// =============================================================================

/// Speechify TTS Provider
///
/// Provides real-time text-to-speech synthesis using Speechify's streaming API.
///
/// # Features
///
/// - **Simba English**: Standard English model, clear and natural
/// - **Simba Turbo**: Faster processing with emotion control
/// - **Simba Multilingual**: 50+ languages support
/// - **1000+ Voices**: Preset voices across multiple languages
/// - **Voice Cloning**: Instant cloning from 10-30s audio sample
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::speechify::SpeechifyTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("george".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = SpeechifyTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct SpeechifyTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Speechify-specific configuration
    speechify_config: SpeechifyTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl SpeechifyTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> SpeechifyRequestBuilder {
        SpeechifyRequestBuilder::new(self.speechify_config.clone(), self.base_config.clone())
    }

    /// List available voices from Speechify API
    ///
    /// # Arguments
    /// * `api_key` - The API key for authentication
    ///
    /// # Returns
    /// * `TTSResult<Vec<SpeechifyVoice>>` - List of available voices
    pub async fn list_voices(api_key: &str) -> TTSResult<Vec<SpeechifyVoice>> {
        let client = reqwest::Client::new();

        let response = client
            .get(SPEECHIFY_VOICES_URL)
            .header(AUTHORIZATION, format!("Bearer {}", api_key))
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to fetch voices: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(TTSError::ProviderError(format!(
                "Failed to list voices ({}): {}",
                status, error_body
            )));
        }

        let voices_response: SpeechifyVoicesResponse = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse voices response: {}", e))
        })?;

        Ok(voices_response.voices)
    }

    /// Get the Speechify-specific configuration
    pub fn speechify_config(&self) -> &SpeechifyTtsConfig {
        &self.speechify_config
    }

    /// Get the streaming endpoint URL being used
    pub fn streaming_url(&self) -> &str {
        super::SPEECHIFY_TTS_STREAM_URL
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// Mirrors `DeepgramTTS::from_standard`: maps the standardized features onto Speechify's
    /// provider config via [`SpeechifyTtsConfig::from_standard`] (language + the normalization
    /// extras), then constructs the provider mirroring [`BaseTTS::new`]. Speechify is a minimal
    /// surface, so speed/pitch/volume/voice-settings/emotion/instructions/ssml/seed/sample_rate
    /// are capability gaps and stay at their defaults — never fabricated.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let speechify_config = SpeechifyTtsConfig::from_standard(std)?;

        Ok(Self {
            provider: TTSProvider::new()?,
            speechify_config,
            base_config: std.base.clone(),
        })
    }
}

#[async_trait]
impl BaseTTS for SpeechifyTts {
    /// Create a new Speechify TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Speechify-specific configuration from base config
        let speechify_config = SpeechifyTtsConfig::from_base(&config)?;

        info!(
            "Creating Speechify TTS provider: voice={}, model={}, format={}",
            speechify_config.voice_id, speechify_config.model, speechify_config.audio_format
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            speechify_config,
            base_config: config,
        })
    }

    /// Get the underlying TTSProvider for HTTP-based providers
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the TTS provider
    ///
    /// Initializes the HTTP connection pool and prepares for synthesis.
    async fn connect(&mut self) -> TTSResult<()> {
        debug!(
            "Connecting Speechify TTS provider to {}",
            super::SPEECHIFY_TTS_STREAM_URL
        );
        self.provider
            .generic_connect_with_config(super::SPEECHIFY_TTS_STREAM_URL, &self.base_config)
            .await
    }

    /// Synthesize text to speech
    ///
    /// # Arguments
    /// * `text` - The text to synthesize
    /// * `flush` - Whether to interrupt current playback
    ///
    /// # Returns
    /// * `TTSResult<()>` - Success or error
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if text.is_empty() || text.trim().is_empty() {
            return Ok(());
        }

        // Validate text length
        SpeechifyTtsConfig::validate_text(text)?;

        debug!(
            "Speechify TTS speak: text='{}', flush={}, voice={}, model={}",
            text, flush, self.speechify_config.voice_id, self.speechify_config.model
        );

        let request_builder = self.create_request_builder();
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
    use crate::core::tts::speechify::SpeechifyModel;

    // The provider struct's `from_standard` (mirroring `DeepgramTTS::from_standard`) maps a
    // standardized advanced feature (language override) all the way onto the provider config the
    // request builder reads — proving the standardized dispatch path reaches the struct, not just
    // the config-level method.
    #[test]
    fn from_standard_maps_language_onto_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "speechify".to_string(),
                api_key: "test-api-key".to_string(),
                voice_id: Some("george".to_string()),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("es-ES".to_string()),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = SpeechifyTts::from_standard(&std).unwrap();
        assert_eq!(tts.speechify_config().language.as_deref(), Some("es-ES"));
    }

    #[test]
    fn test_speechify_tts_creation() {
        // With API key
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        };

        let tts = SpeechifyTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.speechify_config().voice_id, "george");
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_speechify_tts_requires_api_key() {
        let config = TTSConfig::default();
        let result = SpeechifyTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(
                msg.contains("API") || msg.contains("api_key") || msg.contains("SPEECHIFY_API_KEY")
            );
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_headers() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let speechify_config = SpeechifyTtsConfig::from_base(&config).unwrap();
        let builder = SpeechifyRequestBuilder::new(speechify_config, config);

        // Test that streaming URL is correct
        assert!(builder.streaming_url().contains("api.sws.speechify.com"));
        assert!(builder.streaming_url().contains("/v1/audio/stream"));
    }

    #[test]
    fn test_request_builder_with_turbo() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            model: "simba-turbo".to_string(),
            ..Default::default()
        };

        let speechify_config = SpeechifyTtsConfig::from_base(&config).unwrap();
        let builder = SpeechifyRequestBuilder::new(speechify_config, config);

        assert_eq!(builder.speechify_config().model, SpeechifyModel::SimbaTurbo);
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            model: "simba-turbo".to_string(),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let speechify_config = SpeechifyTtsConfig::from_base(&config).unwrap();
        let request = SpeechifyStreamRequest::from_config(&speechify_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Verify JSON structure
        assert!(json.contains("\"input\":\"Hello world\""));
        assert!(json.contains("\"voice_id\":\"george\""));
        assert!(json.contains("\"model\":\"simba-turbo\""));
        assert!(json.contains("\"audio_format\":\"mp3_24000\""));
        // Language is not in base TTSConfig so won't be in the request
    }

    #[test]
    fn test_stream_request_turbo_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("henry".to_string()),
            model: "simba-turbo".to_string(),
            ..Default::default()
        };

        let speechify_config = SpeechifyTtsConfig::from_base(&config).unwrap();
        let request = SpeechifyStreamRequest::from_config(&speechify_config, "Test");

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"simba-turbo\""));
    }

    #[test]
    fn test_pronunciation_replacer() {
        use crate::core::tts::base::Pronunciation;

        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "API".to_string(),
                    pronunciation: "A P I".to_string(),
                },
                Pronunciation {
                    word: "Speechify".to_string(),
                    pronunciation: "speechify".to_string(),
                },
            ],
            ..Default::default()
        };

        let speechify_config = SpeechifyTtsConfig::from_base(&config).unwrap();
        let builder = SpeechifyRequestBuilder::new(speechify_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_speechify_tts_connect_disconnect() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechifyTts::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);

        // Connect
        let result = tts.connect().await;
        assert!(result.is_ok());
        assert!(tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Connected);

        // Disconnect
        let result = tts.disconnect().await;
        assert!(result.is_ok());
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_speechify_tts_speak_not_connected() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechifyTts::new(config).unwrap();

        // Should fail when not connected
        let result = tts.speak("Hello", true).await;
        assert!(result.is_err());

        if let Err(TTSError::ProviderNotReady(_)) = result {
            // Expected
        } else {
            panic!("Expected ProviderNotReady error");
        }
    }

    #[tokio::test]
    async fn test_speechify_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechifyTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_speechify_tts_text_too_long() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechifyTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Text over 20000 characters should fail
        let long_text = "a".repeat(20001);
        let result = tts.speak(&long_text, true).await;
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("20000"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }

        tts.disconnect().await.unwrap();
    }

    #[test]
    fn test_default_streaming_url() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("george".to_string()),
            ..Default::default()
        };

        let tts = SpeechifyTts::new(config).unwrap();
        assert_eq!(
            tts.streaming_url(),
            "https://api.sws.speechify.com/v1/audio/stream"
        );
    }
}
