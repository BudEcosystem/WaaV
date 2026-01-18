//! Speechmatics TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Speechmatics API,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use tracing::{debug, info};

use super::config::{SpeechmaticsGenerateRequest, SpeechmaticsTtsConfig};
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSResult};
#[cfg(test)]
use crate::core::tts::base::TTSError;
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Speechmatics TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for Speechmatics's generate API.
#[derive(Clone)]
pub struct SpeechmaticsRequestBuilder {
    /// Speechmatics-specific configuration
    config: SpeechmaticsTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl SpeechmaticsRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(speechmatics_config: SpeechmaticsTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: speechmatics_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the streaming endpoint URL (with voice and format)
    pub fn streaming_url(&self) -> String {
        self.config.full_url()
    }

    /// Get the Speechmatics configuration
    pub fn speechmatics_config(&self) -> &SpeechmaticsTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for SpeechmaticsRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = SpeechmaticsGenerateRequest::from_config(&self.config, text);

        // Build headers
        let mut headers = HeaderMap::new();

        // Speechmatics uses Bearer token authentication
        let auth_value = format!("Bearer {}", self.config.api_key);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let url = self.streaming_url();

        debug!(
            "Speechmatics TTS request: voice={}, output_format={}, url={}",
            self.config.voice,
            self.config.output_format,
            url
        );

        // Build and return the request
        client.post(&url).headers(headers).json(&request_body)
    }

    fn get_config(&self) -> &TTSConfig {
        &self.base_config
    }

    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Speechmatics TTS Provider
// =============================================================================

/// Speechmatics TTS Provider
///
/// Provides real-time text-to-speech synthesis using Speechmatics's HTTP streaming API.
///
/// # Features
///
/// - **4 English voices**: Sarah (UK), Theo (UK), Megan (US), Jack (US)
/// - **Audio formats**: WAV (16kHz), PCM (16kHz)
/// - **Natural prosody**: Expressive speech without SSML
/// - **Low latency**: <200ms time-to-first-audio
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::speechmatics::SpeechmaticsTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("sarah".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = SpeechmaticsTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct SpeechmaticsTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Speechmatics-specific configuration
    speechmatics_config: SpeechmaticsTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl SpeechmaticsTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> SpeechmaticsRequestBuilder {
        SpeechmaticsRequestBuilder::new(self.speechmatics_config.clone(), self.base_config.clone())
    }

    /// Get the Speechmatics-specific configuration
    pub fn speechmatics_config(&self) -> &SpeechmaticsTtsConfig {
        &self.speechmatics_config
    }

    /// Get the streaming endpoint URL being used
    pub fn streaming_url(&self) -> String {
        self.speechmatics_config.full_url()
    }
}

#[async_trait]
impl BaseTTS for SpeechmaticsTts {
    /// Create a new Speechmatics TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Speechmatics-specific configuration from base config
        let speechmatics_config = SpeechmaticsTtsConfig::from_base(&config)?;

        info!(
            "Creating Speechmatics TTS provider: voice={}, output_format={}",
            speechmatics_config.voice, speechmatics_config.output_format
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            speechmatics_config,
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
        let url = self.streaming_url();
        debug!("Connecting Speechmatics TTS provider to {}", url);
        self.provider
            .generic_connect_with_config(&url, &self.base_config)
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
        SpeechmaticsTtsConfig::validate_text(text)?;

        debug!(
            "Speechmatics TTS speak: text='{}', flush={}, voice={}, format={}",
            text, flush, self.speechmatics_config.voice, self.speechmatics_config.output_format
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
    use crate::core::tts::speechmatics::{SpeechmaticsOutputFormat, SpeechmaticsVoice};

    #[test]
    fn test_speechmatics_tts_creation() {
        // With API key
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("sarah".to_string()),
            ..Default::default()
        };

        let tts = SpeechmaticsTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.speechmatics_config().voice, SpeechmaticsVoice::Sarah);
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_speechmatics_tts_requires_api_key() {
        let config = TTSConfig::default();
        let result = SpeechmaticsTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("API") || msg.contains("api_key") || msg.contains("SPEECHMATICS"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_url() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("theo".to_string()),
            audio_format: Some("pcm".to_string()),
            ..Default::default()
        };

        let speechmatics_config = SpeechmaticsTtsConfig::from_base(&config).unwrap();
        let builder = SpeechmaticsRequestBuilder::new(speechmatics_config, config);

        let url = builder.streaming_url();
        assert!(url.contains("speechmatics.com"));
        assert!(url.contains("/generate/theo"));
        assert!(url.contains("output_format=pcm_16000"));
    }

    #[test]
    fn test_request_builder_with_different_voices() {
        for voice in SpeechmaticsVoice::all() {
            let config = TTSConfig {
                api_key: "test-key".to_string(),
                voice_id: Some(voice.as_str().to_string()),
                ..Default::default()
            };

            let speechmatics_config = SpeechmaticsTtsConfig::from_base(&config).unwrap();
            let builder = SpeechmaticsRequestBuilder::new(speechmatics_config, config);

            assert_eq!(builder.speechmatics_config().voice, *voice);
            assert!(builder.streaming_url().contains(voice.as_str()));
        }
    }

    #[test]
    fn test_generate_request_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("megan".to_string()),
            ..Default::default()
        };

        let speechmatics_config = SpeechmaticsTtsConfig::from_base(&config).unwrap();
        let request = SpeechmaticsGenerateRequest::from_config(&speechmatics_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Verify JSON structure
        assert!(json.contains("\"text\":\"Hello world\""));
    }

    #[test]
    fn test_pronunciation_replacer() {
        use crate::core::tts::base::Pronunciation;

        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("sarah".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "API".to_string(),
                    pronunciation: "A P I".to_string(),
                },
                Pronunciation {
                    word: "TTS".to_string(),
                    pronunciation: "text to speech".to_string(),
                },
            ],
            ..Default::default()
        };

        let speechmatics_config = SpeechmaticsTtsConfig::from_base(&config).unwrap();
        let builder = SpeechmaticsRequestBuilder::new(speechmatics_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_speechmatics_tts_connect_disconnect() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("sarah".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechmaticsTts::new(config).unwrap();
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
    async fn test_speechmatics_tts_speak_not_connected() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("sarah".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechmaticsTts::new(config).unwrap();

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
    async fn test_speechmatics_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("sarah".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechmaticsTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_speechmatics_tts_text_too_long() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("sarah".to_string()),
            ..Default::default()
        };

        let mut tts = SpeechmaticsTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Text over max length should fail
        let long_text = "a".repeat(5001);
        let result = tts.speak(&long_text, true).await;
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("5000"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }

        tts.disconnect().await.unwrap();
    }

    #[test]
    fn test_default_streaming_url() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("sarah".to_string()),
            ..Default::default()
        };

        let tts = SpeechmaticsTts::new(config).unwrap();
        let url = tts.streaming_url();
        
        assert!(url.contains("preview.tts.speechmatics.com"));
        assert!(url.contains("/generate/sarah"));
        assert!(url.contains("output_format=wav_16000"));
    }

    #[test]
    fn test_config_with_all_formats() {
        for format in &[SpeechmaticsOutputFormat::Wav16000, SpeechmaticsOutputFormat::Pcm16000] {
            let config = TTSConfig {
                api_key: "test-key".to_string(),
                audio_format: Some(format.as_str().to_string()),
                ..Default::default()
            };

            let tts = SpeechmaticsTts::new(config).unwrap();
            assert_eq!(tts.speechmatics_config().output_format, *format);
        }
    }

    #[test]
    fn test_voice_description() {
        assert_eq!(SpeechmaticsVoice::Sarah.description(), "Female UK English voice");
        assert_eq!(SpeechmaticsVoice::Theo.description(), "Male UK English voice");
        assert_eq!(SpeechmaticsVoice::Megan.description(), "Female US English voice");
        assert_eq!(SpeechmaticsVoice::Jack.description(), "Male US English voice");
    }
}
