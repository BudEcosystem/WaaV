//! Unreal Speech TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Unreal Speech API,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use tracing::{debug, info};

use super::config::{UnrealSpeechStreamRequest, UnrealSpeechTtsConfig};
#[cfg(test)]
use crate::core::tts::base::TTSError;
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Unreal Speech TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for Unreal Speech's streaming API.
#[derive(Clone)]
pub struct UnrealSpeechRequestBuilder {
    /// Unreal Speech-specific configuration
    config: UnrealSpeechTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl UnrealSpeechRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(unrealspeech_config: UnrealSpeechTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: unrealspeech_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the streaming endpoint URL
    pub fn streaming_url(&self) -> &str {
        super::UNREALSPEECH_STREAM_URL
    }

    /// Get the Unreal Speech configuration
    pub fn unrealspeech_config(&self) -> &UnrealSpeechTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for UnrealSpeechRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = UnrealSpeechStreamRequest::from_config(&self.config, text);

        // Build headers
        let mut headers = HeaderMap::new();

        // Unreal Speech uses Bearer token authentication
        let auth_value = format!("Bearer {}", self.config.api_key);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        debug!(
            "Unreal Speech TTS request: voice={}, bitrate={}, speed={}, pitch={}, codec={}, url={}",
            request_body.voice_id,
            request_body.bitrate,
            request_body.speed,
            request_body.pitch,
            request_body.codec,
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
// Unreal Speech TTS Provider
// =============================================================================

/// Unreal Speech TTS Provider
///
/// Provides real-time text-to-speech synthesis using Unreal Speech's streaming API.
///
/// # Features
///
/// - **Ultra-low latency**: ~300ms time-to-first-audio
/// - **Standard voices**: Scarlett, Liv, Amy, Dan, Will
/// - **Kokoro V8 voices**: af, af_bella, am_adam, bf_emma, bm_george, etc.
/// - **Speed control**: -1.0 to 1.0
/// - **Pitch control**: 0.5 to 1.5
/// - **Bitrate options**: 16k to 320k
/// - **Cost-effective**: Up to 90% cheaper than competitors
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::unrealspeech::UnrealSpeechTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("Scarlett".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = UnrealSpeechTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct UnrealSpeechTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Unreal Speech-specific configuration
    unrealspeech_config: UnrealSpeechTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl UnrealSpeechTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> UnrealSpeechRequestBuilder {
        UnrealSpeechRequestBuilder::new(self.unrealspeech_config.clone(), self.base_config.clone())
    }

    /// Build from the standardized config (W1 keystone for TTS), mirroring
    /// `DeepgramTTS::from_standard`. Delegates the feature mapping to the config-level
    /// [`UnrealSpeechTtsConfig::from_standard`] (which maps `speed`→`speed` offset, `pitch`→`pitch`
    /// multiplier, and the non-standard `bitrate` extra), so the mapped Unreal Speech settings are
    /// honored end-to-end through the dispatch path. The matching flat base config is kept for the
    /// generic provider's connect layer.
    ///
    /// [`UnrealSpeechTtsConfig::from_standard`]: super::config::UnrealSpeechTtsConfig::from_standard
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let unrealspeech_config = UnrealSpeechTtsConfig::from_standard(std)?;

        info!(
            "Creating Unreal Speech TTS provider: voice={}, bitrate={}, speed={}, pitch={}",
            unrealspeech_config.voice,
            unrealspeech_config.bitrate,
            unrealspeech_config.speed,
            unrealspeech_config.pitch
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            unrealspeech_config,
            base_config: std.base.clone(),
        })
    }

    /// Get the Unreal Speech-specific configuration
    pub fn unrealspeech_config(&self) -> &UnrealSpeechTtsConfig {
        &self.unrealspeech_config
    }

    /// Get the streaming endpoint URL being used
    pub fn streaming_url(&self) -> &str {
        super::UNREALSPEECH_STREAM_URL
    }
}

#[async_trait]
impl BaseTTS for UnrealSpeechTts {
    /// Create a new Unreal Speech TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Unreal Speech-specific configuration from base config
        let unrealspeech_config = UnrealSpeechTtsConfig::from_base(&config)?;

        info!(
            "Creating Unreal Speech TTS provider: voice={}, bitrate={}, speed={}, pitch={}",
            unrealspeech_config.voice,
            unrealspeech_config.bitrate,
            unrealspeech_config.speed,
            unrealspeech_config.pitch
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            unrealspeech_config,
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
            "Connecting Unreal Speech TTS provider to {}",
            super::UNREALSPEECH_STREAM_URL
        );
        self.provider
            .generic_connect_with_config(super::UNREALSPEECH_STREAM_URL, &self.base_config)
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

        // Validate text length for stream endpoint
        UnrealSpeechTtsConfig::validate_stream_text(text)?;

        debug!(
            "Unreal Speech TTS speak: text='{}', flush={}, voice={}, bitrate={}",
            text, flush, self.unrealspeech_config.voice, self.unrealspeech_config.bitrate
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
    use crate::core::tts::unrealspeech::{UnrealSpeechBitrate, UnrealSpeechVoice};

    // W1 keystone: the standardized dispatch path reaches the provider STRUCT's `from_standard`,
    // building the real provider with the mapped speed offset + pitch multiplier (plus the bitrate
    // extra), all previously unreachable through the flat factory.
    #[test]
    fn from_standard_builds_provider_with_mapped_speed_pitch_bitrate() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("bitrate".into(), serde_json::json!(320));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "unrealspeech".into(),
                api_key: "test-key".into(),
                voice_id: Some("Dan".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(0.5),
                pitch: Some(1.2),
                ssml: Some(true), // capability gap: ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = UnrealSpeechTts::from_standard(&std).unwrap();
        assert_eq!(tts.unrealspeech_config().speed, 0.5);
        assert_eq!(tts.unrealspeech_config().pitch, 1.2);
        assert_eq!(
            tts.unrealspeech_config().bitrate,
            UnrealSpeechBitrate::Bitrate320k
        );
        assert_eq!(tts.unrealspeech_config().voice, UnrealSpeechVoice::Dan);
    }

    #[test]
    fn test_unrealspeech_tts_creation() {
        // With API key
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("Scarlett".to_string()),
            ..Default::default()
        };

        let tts = UnrealSpeechTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.unrealspeech_config().voice, UnrealSpeechVoice::Scarlett);
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_unrealspeech_tts_requires_api_key() {
        let config = TTSConfig::default();
        let result = UnrealSpeechTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("API") || msg.contains("api_key") || msg.contains("UNREALSPEECH"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_headers() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Dan".to_string()),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let unrealspeech_config = UnrealSpeechTtsConfig::from_base(&config).unwrap();
        let builder = UnrealSpeechRequestBuilder::new(unrealspeech_config, config);

        // Test that streaming URL is correct
        assert!(builder.streaming_url().contains("api.v8.unrealspeech.com"));
        assert!(builder.streaming_url().contains("/stream"));
    }

    #[test]
    fn test_request_builder_with_kokoro_voice() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("af_bella".to_string()),
            ..Default::default()
        };

        let unrealspeech_config = UnrealSpeechTtsConfig::from_base(&config).unwrap();
        let builder = UnrealSpeechRequestBuilder::new(unrealspeech_config, config);

        assert_eq!(
            builder.unrealspeech_config().voice,
            UnrealSpeechVoice::AfBella
        );
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Will".to_string()),
            ..Default::default()
        };

        let unrealspeech_config = UnrealSpeechTtsConfig::from_base(&config).unwrap();
        let request = UnrealSpeechStreamRequest::from_config(&unrealspeech_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Verify JSON structure (PascalCase field names for Unreal Speech API)
        assert!(json.contains("\"Text\":\"Hello world\""));
        assert!(json.contains("\"VoiceId\":\"Will\""));
        assert!(json.contains("\"Bitrate\":\"192k\""));
        assert!(json.contains("\"Speed\":0"));
        assert!(json.contains("\"Pitch\":1"));
        assert!(json.contains("\"Codec\":\"libmp3lame\""));
    }

    #[test]
    fn test_pronunciation_replacer() {
        use crate::core::tts::base::Pronunciation;

        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Scarlett".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "API".to_string(),
                    pronunciation: "A P I".to_string(),
                },
                Pronunciation {
                    word: "Unreal".to_string(),
                    pronunciation: "un-real".to_string(),
                },
            ],
            ..Default::default()
        };

        let unrealspeech_config = UnrealSpeechTtsConfig::from_base(&config).unwrap();
        let builder = UnrealSpeechRequestBuilder::new(unrealspeech_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_unrealspeech_tts_connect_disconnect() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Scarlett".to_string()),
            ..Default::default()
        };

        let mut tts = UnrealSpeechTts::new(config).unwrap();
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
    async fn test_unrealspeech_tts_speak_not_connected() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Scarlett".to_string()),
            ..Default::default()
        };

        let mut tts = UnrealSpeechTts::new(config).unwrap();

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
    async fn test_unrealspeech_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Scarlett".to_string()),
            ..Default::default()
        };

        let mut tts = UnrealSpeechTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_unrealspeech_tts_text_too_long() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Scarlett".to_string()),
            ..Default::default()
        };

        let mut tts = UnrealSpeechTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Text over 1000 characters should fail for /stream endpoint
        let long_text = "a".repeat(1001);
        let result = tts.speak(&long_text, true).await;
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("1000"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }

        tts.disconnect().await.unwrap();
    }

    #[test]
    fn test_default_streaming_url() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("Scarlett".to_string()),
            ..Default::default()
        };

        let tts = UnrealSpeechTts::new(config).unwrap();
        assert_eq!(
            tts.streaming_url(),
            "https://api.v8.unrealspeech.com/stream"
        );
    }

    #[test]
    fn test_config_with_custom_settings() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("am_adam".to_string()),
            ..Default::default()
        };

        let tts = UnrealSpeechTts::new(config).unwrap();
        assert_eq!(tts.unrealspeech_config().voice, UnrealSpeechVoice::AmAdam);
        assert_eq!(
            tts.unrealspeech_config().bitrate,
            UnrealSpeechBitrate::Bitrate192k
        );
    }
}
