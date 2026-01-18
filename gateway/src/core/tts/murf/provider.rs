//! Murf.ai TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Murf.ai,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use tracing::{debug, info};

use super::config::{MurfStreamRequest, MurfTtsConfig, MurfVoice};
use super::MURF_VOICES_URL;
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Murf.ai TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for Murf.ai's streaming API.
#[derive(Clone)]
pub struct MurfRequestBuilder {
    /// Murf-specific configuration
    config: MurfTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl MurfRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(murf_config: MurfTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: murf_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the streaming endpoint URL
    pub fn streaming_url(&self) -> &str {
        self.config.streaming_url()
    }

    /// Get the Murf configuration
    pub fn murf_config(&self) -> &MurfTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for MurfRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = MurfStreamRequest::from_config(&self.config, text);

        // Build headers
        let mut headers = HeaderMap::new();

        // Murf uses 'api-key' header (NOT Authorization: Bearer)
        headers.insert(
            "api-key",
            HeaderValue::from_str(&self.config.api_key).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        debug!(
            "Murf TTS request: voice={}, model={}, format={}, sample_rate={}, url={}",
            request_body.voice_id,
            request_body.model,
            request_body.format,
            request_body.sample_rate,
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
// Murf TTS Provider
// =============================================================================

/// Murf.ai TTS Provider
///
/// Provides real-time text-to-speech synthesis using Murf.ai's streaming API.
///
/// # Features
///
/// - **Falcon Model**: Ultra-low latency (~130ms TTFA) for real-time applications
/// - **Gen2 Model**: Studio-quality with full customization (pitch, rate, style)
/// - **150+ Voices**: Support for 35+ languages with 20+ speaking styles
/// - **Regional Endpoints**: 12 global regions for data residency and latency optimization
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::murf::MurfTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("en-US-natalie".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = MurfTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct MurfTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Murf-specific configuration
    murf_config: MurfTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl MurfTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> MurfRequestBuilder {
        MurfRequestBuilder::new(self.murf_config.clone(), self.base_config.clone())
    }

    /// List available voices from Murf.ai API
    ///
    /// # Arguments
    /// * `api_key` - The API key for authentication
    ///
    /// # Returns
    /// * `TTSResult<Vec<MurfVoice>>` - List of available voices
    pub async fn list_voices(api_key: &str) -> TTSResult<Vec<MurfVoice>> {
        let client = reqwest::Client::new();

        let response = client
            .get(MURF_VOICES_URL)
            .header("api-key", api_key)
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

        let voices: Vec<MurfVoice> = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse voices response: {}", e))
        })?;

        Ok(voices)
    }

    /// Get the Murf-specific configuration
    pub fn murf_config(&self) -> &MurfTtsConfig {
        &self.murf_config
    }

    /// Get the streaming endpoint URL being used
    pub fn streaming_url(&self) -> &str {
        self.murf_config.streaming_url()
    }
}

#[async_trait]
impl BaseTTS for MurfTts {
    /// Create a new Murf.ai TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Murf-specific configuration from base config
        let murf_config = MurfTtsConfig::from_base(&config)?;

        info!(
            "Creating Murf TTS provider: voice={}, model={}, format={}, region={}",
            murf_config.voice_id,
            murf_config.model,
            murf_config.format,
            murf_config.region
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            murf_config,
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
        let streaming_url = self.murf_config.streaming_url();
        debug!("Connecting Murf TTS provider to {}", streaming_url);
        self.provider
            .generic_connect_with_config(streaming_url, &self.base_config)
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

        debug!(
            "Murf TTS speak: text='{}', flush={}, voice={}, model={}",
            text, flush, self.murf_config.voice_id, self.murf_config.model
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

    #[test]
    fn test_murf_tts_creation() {
        // With API key
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            ..Default::default()
        };

        let tts = MurfTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.murf_config().voice_id, "en-US-natalie");
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_murf_tts_requires_api_key() {
        let config = TTSConfig::default();
        let result = MurfTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("API") || msg.contains("api_key") || msg.contains("MURF_API_KEY"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_headers() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        };

        let murf_config = MurfTtsConfig::from_base(&config).unwrap();
        let builder = MurfRequestBuilder::new(murf_config, config);

        // Test that streaming URL is correct
        assert!(builder.streaming_url().contains("api.murf.ai"));
        assert!(builder.streaming_url().contains("/v1/speech/stream"));
    }

    #[test]
    fn test_request_builder_with_region() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            ..Default::default()
        };

        // Create config and set region via builder method
        let murf_config = MurfTtsConfig::from_base(&config)
            .unwrap()
            .with_region(crate::core::tts::murf::MurfRegion::UsEast);
        let builder = MurfRequestBuilder::new(murf_config, config);

        assert!(builder.streaming_url().contains("us-east"));
    }

    #[test]
    fn test_request_builder_gen2_params() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            model: "GEN2".to_string(),
            ..Default::default()
        };

        // Create config and set Gen2-specific params via builder methods
        let murf_config = MurfTtsConfig::from_base(&config)
            .unwrap()
            .with_rate(10)
            .with_pitch(-5)
            .with_style("Conversational");

        let builder = MurfRequestBuilder::new(murf_config, config);

        assert_eq!(builder.murf_config().rate, Some(10));
        assert_eq!(builder.murf_config().pitch, Some(-5));
        assert_eq!(
            builder.murf_config().style,
            Some("Conversational".to_string())
        );
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            audio_format: Some("pcm".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        };

        let murf_config = MurfTtsConfig::from_base(&config).unwrap();
        let request = MurfStreamRequest::from_config(&murf_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Verify JSON structure
        assert!(json.contains("\"text\":\"Hello world\""));
        assert!(json.contains("\"voiceId\":\"en-US-natalie\""));
        assert!(json.contains("\"model\":\"FALCON\""));
        assert!(json.contains("\"format\":\"PCM\""));
        assert!(json.contains("\"sampleRate\":24000"));
    }

    #[test]
    fn test_pronunciation_replacer() {
        use crate::core::tts::base::Pronunciation;

        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "API".to_string(),
                    pronunciation: "A P I".to_string(),
                },
                Pronunciation {
                    word: "Murf".to_string(),
                    pronunciation: "murf".to_string(),
                },
            ],
            ..Default::default()
        };

        let murf_config = MurfTtsConfig::from_base(&config).unwrap();
        let builder = MurfRequestBuilder::new(murf_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_murf_tts_connect_disconnect() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            ..Default::default()
        };

        let mut tts = MurfTts::new(config).unwrap();
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
    async fn test_murf_tts_speak_not_connected() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            ..Default::default()
        };

        let mut tts = MurfTts::new(config).unwrap();

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
    async fn test_murf_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            ..Default::default()
        };

        let mut tts = MurfTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[test]
    fn test_default_streaming_url() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("en-US-natalie".to_string()),
            ..Default::default()
        };

        let tts = MurfTts::new(config).unwrap();
        assert_eq!(
            tts.streaming_url(),
            "https://global.api.murf.ai/v1/speech/stream"
        );
    }
}
