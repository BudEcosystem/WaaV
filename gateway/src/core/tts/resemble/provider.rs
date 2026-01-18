//! Resemble AI TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Resemble AI,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use tracing::{debug, info};

use super::config::{ResembleStreamRequest, ResembleTtsConfig, ResembleVoicesResponse};
use super::RESEMBLE_TTS_STREAM_URL;
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Resemble AI TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for Resemble's streaming API.
#[derive(Clone)]
pub struct ResembleRequestBuilder {
    /// Resemble-specific configuration
    config: ResembleTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl ResembleRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(resemble_config: ResembleTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: resemble_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the Resemble configuration
    pub fn resemble_config(&self) -> &ResembleTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for ResembleRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = ResembleStreamRequest::from_config(&self.config, text);

        // Build headers
        // Resemble uses Bearer token authentication
        let mut headers = HeaderMap::new();
        let auth_value = format!("Bearer {}", self.config.api_key);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        debug!(
            "Resemble AI TTS request: voice_uuid={}, model={}, text_len={}",
            request_body.voice_uuid,
            self.config.model,
            text.len()
        );

        // Build and return the request
        client
            .post(RESEMBLE_TTS_STREAM_URL)
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
// Resemble TTS Provider
// =============================================================================

/// Resemble AI TTS Provider
///
/// Provides real-time text-to-speech synthesis using Resemble AI's streaming API.
///
/// # Features
///
/// - **Multiple Models**: Chatterbox, Chatterbox Turbo (low-latency), Chatterbox Multilingual
/// - **149+ Languages**: Extensive language support with multilingual model
/// - **Voice Cloning**: Clone voices with just 10 seconds of audio
/// - **HD Mode**: Higher quality synthesis option
/// - **Paralinguistic Tags**: [cough], [laugh], [chuckle] with Turbo model
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::resemble::ResembleTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("voice-uuid".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = ResembleTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct ResembleTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Resemble-specific configuration
    resemble_config: ResembleTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl ResembleTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> ResembleRequestBuilder {
        ResembleRequestBuilder::new(self.resemble_config.clone(), self.base_config.clone())
    }

    /// List available voices from Resemble AI API
    ///
    /// # Arguments
    /// * `api_key` - The API key for authentication
    /// * `page` - Page number (1-based)
    /// * `page_size` - Results per page (10-1000)
    ///
    /// # Returns
    /// * `TTSResult<ResembleVoicesResponse>` - Paginated list of available voices
    pub async fn list_voices(
        api_key: &str,
        page: u32,
        page_size: u32,
    ) -> TTSResult<ResembleVoicesResponse> {
        let client = reqwest::Client::new();

        let url = format!(
            "{}?page={}&page_size={}",
            super::RESEMBLE_VOICES_URL,
            page,
            page_size
        );

        let response = client
            .get(&url)
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

        let voices_response: ResembleVoicesResponse = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse voices response: {}", e))
        })?;

        Ok(voices_response)
    }

    /// Get the Resemble-specific configuration
    pub fn resemble_config(&self) -> &ResembleTtsConfig {
        &self.resemble_config
    }
}

#[async_trait]
impl BaseTTS for ResembleTts {
    /// Create a new Resemble AI TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Resemble-specific configuration from base config
        let resemble_config = ResembleTtsConfig::from_base(&config)?;

        info!(
            "Creating Resemble AI TTS provider: voice_uuid={}, model={}",
            resemble_config.voice_uuid, resemble_config.model
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            resemble_config,
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
        debug!("Connecting Resemble AI TTS provider");
        self.provider
            .generic_connect_with_config(RESEMBLE_TTS_STREAM_URL, &self.base_config)
            .await
    }

    /// Synthesize text to speech
    ///
    /// # Arguments
    /// * `text` - The text to synthesize (max 2000 characters for streaming)
    /// * `flush` - Whether to interrupt current playback
    ///
    /// # Returns
    /// * `TTSResult<()>` - Success or error
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if text.is_empty() || text.trim().is_empty() {
            return Ok(());
        }

        // Validate text length
        ResembleTtsConfig::validate_text(text)?;

        debug!(
            "Resemble AI TTS speak: text='{}', flush={}, voice_uuid={}, model={}",
            text, flush, self.resemble_config.voice_uuid, self.resemble_config.model
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
    use crate::core::tts::base::Pronunciation;
    use crate::core::tts::resemble::config::ResembleModel;

    #[test]
    fn test_resemble_tts_creation() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("test-voice-uuid".to_string()),
            ..Default::default()
        };

        let tts = ResembleTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.resemble_config().voice_uuid, "test-voice-uuid");
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_resemble_tts_requires_api_key() {
        let config = TTSConfig {
            voice_id: Some("test-voice-uuid".to_string()),
            ..Default::default()
        };
        let result = ResembleTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(
                msg.contains("API")
                    || msg.contains("api_key")
                    || msg.contains("RESEMBLE_API_KEY")
            );
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_resemble_tts_requires_voice_uuid() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: None,
            ..Default::default()
        };
        let result = ResembleTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("voice_uuid") || msg.contains("voice_id"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_headers() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let builder = ResembleRequestBuilder::new(resemble_config, config);

        // Verify config
        assert_eq!(builder.resemble_config().voice_uuid, "voice-uuid");
        assert_eq!(builder.resemble_config().model, ResembleModel::Chatterbox);
    }

    #[test]
    fn test_request_builder_with_turbo() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            model: "chatterbox-turbo".to_string(),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let builder = ResembleRequestBuilder::new(resemble_config, config);

        assert_eq!(builder.resemble_config().model, ResembleModel::ChatterboxTurbo);
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let request = ResembleStreamRequest::from_config(&resemble_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Verify JSON structure
        assert!(json.contains("\"data\":\"Hello world\""));
        assert!(json.contains("\"voice_uuid\":\"voice-uuid\""));
        // Default model should not include model field
        assert!(!json.contains("\"model\""));
    }

    #[test]
    fn test_stream_request_turbo_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            model: "chatterbox-turbo".to_string(),
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let request = ResembleStreamRequest::from_config(&resemble_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Turbo model should be included
        assert!(json.contains("\"model\":\"chatterbox-turbo\""));
    }

    #[test]
    fn test_pronunciation_replacer() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "API".to_string(),
                    pronunciation: "A P I".to_string(),
                },
                Pronunciation {
                    word: "Resemble".to_string(),
                    pronunciation: "Re-zemble".to_string(),
                },
            ],
            ..Default::default()
        };

        let resemble_config = ResembleTtsConfig::from_base(&config).unwrap();
        let builder = ResembleRequestBuilder::new(resemble_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_resemble_tts_connect_disconnect() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();
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
    async fn test_resemble_tts_speak_not_connected() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();

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
    async fn test_resemble_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_resemble_tts_text_too_long() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("voice-uuid".to_string()),
            ..Default::default()
        };

        let mut tts = ResembleTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Text over 2000 characters should fail
        let long_text = "a".repeat(2001);
        let result = tts.speak(&long_text, true).await;
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("2000"));
        } else {
            panic!("Expected InvalidConfiguration error about text length");
        }

        tts.disconnect().await.unwrap();
    }
}
