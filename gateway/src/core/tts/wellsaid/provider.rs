//! WellSaid Labs TTS Provider Implementation
//!
//! This module provides the core TTS implementation for WellSaid Labs,
//! using HTTP streaming for real-time audio synthesis.

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderValue};
use tracing::{debug, info};

use super::WELLSAID_AVATARS_URL;
use super::config::{WellSaidAvatar, WellSaidStreamRequest, WellSaidTtsConfig};
use crate::core::tts::base::{BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for WellSaid Labs TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for WellSaid's streaming API.
#[derive(Clone)]
pub struct WellSaidRequestBuilder {
    /// WellSaid-specific configuration
    config: WellSaidTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl WellSaidRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(wellsaid_config: WellSaidTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: wellsaid_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the WellSaid configuration
    pub fn wellsaid_config(&self) -> &WellSaidTtsConfig {
        &self.config
    }
}

impl TTSRequestBuilder for WellSaidRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = WellSaidStreamRequest::from_config(&self.config, text);

        // Build headers
        // WellSaid uses X-Api-Key header (NOT Bearer token)
        let mut headers = HeaderMap::new();
        headers.insert(
            "X-Api-Key",
            HeaderValue::from_str(&self.config.api_key)
                .unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("audio/mpeg"));

        // SSML opt-in: when enabled, tell WellSaid to parse the body's `text` as SSML markup.
        if self.config.ssml {
            headers.insert("X-Enable-SSML", HeaderValue::from_static("true"));
        }

        debug!(
            "WellSaid TTS request: speaker_id={}, model={}, ssml={}, text_len={}",
            request_body.speaker_id,
            self.config.model,
            self.config.ssml,
            text.len()
        );

        // Build and return the request
        client
            .post(super::WELLSAID_TTS_STREAM_URL)
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
// WellSaid TTS Provider
// =============================================================================

/// WellSaid Labs TTS Provider
///
/// Provides real-time text-to-speech synthesis using WellSaid Labs' streaming API.
///
/// # Features
///
/// - **200+ Voice Avatars**: Professional AI voices across multiple styles
/// - **20+ Languages**: Including English, Spanish, German, French, Japanese, Korean, Chinese
/// - **Two Models**:
///   - Legacy: All languages, natural-sounding speech
///   - Caruso: English only, AI Director capabilities (pitch, tempo, loudness)
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::wellsaid::WellSaidTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("3".to_string()), // Alana B.
///     ..Default::default()
/// };
///
/// let mut tts = WellSaidTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct WellSaidTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// WellSaid-specific configuration
    wellsaid_config: WellSaidTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
}

impl WellSaidTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> WellSaidRequestBuilder {
        WellSaidRequestBuilder::new(self.wellsaid_config.clone(), self.base_config.clone())
    }

    /// List available voice avatars from WellSaid Labs API
    ///
    /// # Arguments
    /// * `api_key` - The API key for authentication
    ///
    /// # Returns
    /// * `TTSResult<Vec<WellSaidAvatar>>` - List of available voice avatars
    pub async fn list_avatars(api_key: &str) -> TTSResult<Vec<WellSaidAvatar>> {
        let client = reqwest::Client::new();

        let response = client
            .get(WELLSAID_AVATARS_URL)
            .header("X-Api-Key", api_key)
            .send()
            .await
            .map_err(|e| TTSError::NetworkError(format!("Failed to fetch avatars: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_body = response.text().await.unwrap_or_default();
            return Err(TTSError::ProviderError(format!(
                "Failed to list avatars ({}): {}",
                status, error_body
            )));
        }

        let avatars: Vec<WellSaidAvatar> = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse avatars response: {}", e))
        })?;

        Ok(avatars)
    }

    /// Get the WellSaid-specific configuration
    pub fn wellsaid_config(&self) -> &WellSaidTtsConfig {
        &self.wellsaid_config
    }

    /// Build from the standardized TTS config (W1 keystone). Mirrors [`BaseTTS::new`] but derives
    /// the WellSaid config via [`WellSaidTtsConfig::from_standard`] and keeps the standardized base
    /// config as `base_config`. WellSaid's config only carries voice (`speaker_id`) and model
    /// selection; every standardized prosody/voice feature is a capability gap and is skipped (the
    /// Caruso AI Director is not represented as config state here).
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let wellsaid_config = WellSaidTtsConfig::from_standard(std)?;

        info!(
            "Creating WellSaid TTS provider (standardized): speaker_id={}, model={}",
            wellsaid_config.speaker_id, wellsaid_config.model
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            wellsaid_config,
            base_config: std.base.clone(),
        })
    }
}

#[async_trait]
impl BaseTTS for WellSaidTts {
    /// Create a new WellSaid Labs TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse WellSaid-specific configuration from base config
        let wellsaid_config = WellSaidTtsConfig::from_base(&config)?;

        info!(
            "Creating WellSaid TTS provider: speaker_id={}, model={}",
            wellsaid_config.speaker_id, wellsaid_config.model
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            wellsaid_config,
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
        debug!("Connecting WellSaid TTS provider");
        self.provider
            .generic_connect_with_config(super::WELLSAID_TTS_STREAM_URL, &self.base_config)
            .await
    }

    /// Synthesize text to speech
    ///
    /// # Arguments
    /// * `text` - The text to synthesize (max 1000 characters)
    /// * `flush` - Whether to interrupt current playback
    ///
    /// # Returns
    /// * `TTSResult<()>` - Success or error
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        if text.is_empty() || text.trim().is_empty() {
            return Ok(());
        }

        // Validate text length
        WellSaidTtsConfig::validate_text(text)?;

        debug!(
            "WellSaid TTS speak: text='{}', flush={}, speaker_id={}, model={}",
            text, flush, self.wellsaid_config.speaker_id, self.wellsaid_config.model
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
    use crate::core::tts::wellsaid::config::WellSaidModel;

    #[test]
    fn test_wellsaid_tts_creation() {
        let config = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("26".to_string()),
            ..Default::default()
        };

        let tts = WellSaidTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.wellsaid_config().speaker_id, 26);
        assert!(!tts.is_ready());
    }

    // W1 keystone (TTS): the struct-level `from_standard` builds a real `WellSaidTts` through the
    // standardized path. WellSaid's config holds only voice/model selection (no prosody fields), so
    // this is a passthrough: speaker_id (voice_id) and the Caruso model reach the provider, while
    // standardized prosody features are capability gaps. Mirrors `DeepgramTTS::from_standard`.
    #[test]
    fn from_standard_builds_provider_with_voice_and_model() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        use crate::core::tts::wellsaid::config::WellSaidModel;
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "wellsaid".into(),
                api_key: "test-key".into(),
                voice_id: Some("26".into()),
                model: "caruso".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5), // capability gap: no prosody field, must be ignored
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = WellSaidTts::from_standard(&std).unwrap();
        assert_eq!(tts.wellsaid_config().speaker_id, 26);
        assert_eq!(tts.wellsaid_config().model, WellSaidModel::Caruso);
        assert_eq!(tts.wellsaid_config().api_key, "test-key");
    }

    #[test]
    fn test_wellsaid_tts_requires_api_key() {
        let config = TTSConfig::default();
        let result = WellSaidTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(
                msg.contains("API") || msg.contains("api_key") || msg.contains("WELLSAID_API_KEY")
            );
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_request_builder_headers() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };

        let wellsaid_config = WellSaidTtsConfig::from_base(&config).unwrap();
        let builder = WellSaidRequestBuilder::new(wellsaid_config, config);

        // Verify config
        assert_eq!(builder.wellsaid_config().speaker_id, 3);
        assert_eq!(builder.wellsaid_config().model, WellSaidModel::Legacy);
    }

    // WIRE-LEVEL guard: assert the `X-Enable-SSML` header actually reaches the built HTTP request
    // when (and only when) the standardized `ssml` feature is set. The recurring bug class is
    // asserting only the config struct, so this builds the real `reqwest::Request` and inspects its
    // headers.
    #[test]
    fn ssml_feature_sets_x_enable_ssml_header_on_request() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};

        let client = reqwest::Client::new();
        let mk = |ssml: Option<bool>| {
            let std = StandardTTSConfig {
                base: TTSConfig {
                    provider: "wellsaid".into(),
                    api_key: "test-key".into(),
                    voice_id: Some("26".into()),
                    ..Default::default()
                },
                features: TtsFeatures {
                    ssml,
                    ..Default::default()
                },
                extras: Default::default(),
            };
            let cfg = WellSaidTtsConfig::from_standard(&std).unwrap();
            let builder = WellSaidRequestBuilder::new(cfg, std.base.clone());
            builder
                .build_http_request(&client, "<speak>hi</speak>")
                .build()
                .unwrap()
        };

        // ssml = true: header present and equal to "true".
        let req = mk(Some(true));
        assert_eq!(
            req.headers().get("X-Enable-SSML").map(|v| v.to_str().unwrap()),
            Some("true"),
            "X-Enable-SSML must be set when the ssml feature is on"
        );

        // ssml unset: header must be absent (API default = plain text).
        let req_off = mk(None);
        assert!(
            req_off.headers().get("X-Enable-SSML").is_none(),
            "X-Enable-SSML must be absent when the ssml feature is off"
        );
    }

    #[test]
    fn test_request_builder_with_caruso() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("26".to_string()),
            model: "caruso".to_string(),
            ..Default::default()
        };

        let wellsaid_config = WellSaidTtsConfig::from_base(&config).unwrap();
        let builder = WellSaidRequestBuilder::new(wellsaid_config, config);

        assert_eq!(builder.wellsaid_config().model, WellSaidModel::Caruso);
    }

    #[test]
    fn test_stream_request_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };

        let wellsaid_config = WellSaidTtsConfig::from_base(&config).unwrap();
        let request = WellSaidStreamRequest::from_config(&wellsaid_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Verify JSON structure
        assert!(json.contains("\"text\":\"Hello world\""));
        assert!(json.contains("\"speaker_id\":3"));
        // Legacy model should not include model field
        assert!(!json.contains("\"model\""));
    }

    #[test]
    fn test_stream_request_caruso_serialization() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("26".to_string()),
            model: "caruso".to_string(),
            ..Default::default()
        };

        let wellsaid_config = WellSaidTtsConfig::from_base(&config).unwrap();
        let request = WellSaidStreamRequest::from_config(&wellsaid_config, "Hello world");

        let json = serde_json::to_string(&request).unwrap();

        // Caruso model should be included
        assert!(json.contains("\"model\":\"caruso\""));
    }

    #[test]
    fn test_pronunciation_replacer() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("3".to_string()),
            pronunciations: vec![
                Pronunciation {
                    word: "API".to_string(),
                    pronunciation: "A P I".to_string(),
                },
                Pronunciation {
                    word: "WellSaid".to_string(),
                    pronunciation: "Well Said".to_string(),
                },
            ],
            ..Default::default()
        };

        let wellsaid_config = WellSaidTtsConfig::from_base(&config).unwrap();
        let builder = WellSaidRequestBuilder::new(wellsaid_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    #[tokio::test]
    async fn test_wellsaid_tts_connect_disconnect() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };

        let mut tts = WellSaidTts::new(config).unwrap();
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
    async fn test_wellsaid_tts_speak_not_connected() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };

        let mut tts = WellSaidTts::new(config).unwrap();

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
    async fn test_wellsaid_tts_empty_text() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };

        let mut tts = WellSaidTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_wellsaid_tts_text_too_long() {
        let config = TTSConfig {
            api_key: "test-key".to_string(),
            voice_id: Some("3".to_string()),
            ..Default::default()
        };

        let mut tts = WellSaidTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Text over 1000 characters should fail
        let long_text = "a".repeat(1001);
        let result = tts.speak(&long_text, true).await;
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("1000"));
        } else {
            panic!("Expected InvalidConfiguration error about text length");
        }

        tts.disconnect().await.unwrap();
    }
}
