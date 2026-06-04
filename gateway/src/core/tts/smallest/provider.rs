//! Smallest.ai TTS Provider Implementation
//!
//! This module provides the core TTS implementation for Smallest.ai Waves API,
//! using HTTP streaming for real-time audio synthesis.
//!
//! # Architecture
//!
//! The `SmallestRequestBuilder` constructs HTTP requests for the Smallest.ai Waves API:
//! - URL: `https://waves-api.smallest.ai/api/v1/lightning/get_speech`
//! - Authentication: Bearer token (`Authorization: Bearer <API_KEY>`)
//! - Content-Type: `application/json`
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::smallest::SmallestTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("emily".to_string()),
//!     model: "lightning".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut tts = SmallestTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use tracing::{debug, info};
use xxhash_rust::xxh3::xxh3_128;

use super::config::{SmallestLanguage, SmallestModel, SmallestOutputFormat, SmallestTtsConfig};
use super::messages::{SmallestTtsRequest, SmallestVoice, SmallestVoicesResponse, SmallestWsRequest};
use super::{MAX_TEXT_LENGTH, SMALLEST_TTS_URL, SMALLEST_TTS_WS_URL, SUPPORTED_SAMPLE_RATES, voices_url};
use crate::core::tts::base::{
    AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

// =============================================================================
// Request Builder
// =============================================================================

/// Request builder for Smallest.ai TTS API
///
/// Implements the `TTSRequestBuilder` trait to create HTTP requests
/// with the correct URL, headers, and body for Smallest.ai's REST API.
#[derive(Clone)]
pub struct SmallestRequestBuilder {
    /// Smallest-specific configuration
    config: SmallestTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precompiled pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl SmallestRequestBuilder {
    /// Create a new request builder from configurations
    pub fn new(smallest_config: SmallestTtsConfig, base_config: TTSConfig) -> Self {
        // Build pronunciation replacer from base config
        let pronunciation_replacer = if !base_config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&base_config.pronunciations))
        } else {
            None
        };

        Self {
            config: smallest_config,
            base_config,
            pronunciation_replacer,
        }
    }

    /// Get the REST API endpoint URL
    pub fn rest_url(&self) -> &str {
        SMALLEST_TTS_URL
    }

    /// Get the lightning-v2 streaming WebSocket endpoint URL.
    pub fn ws_url(&self) -> &str {
        SMALLEST_TTS_WS_URL
    }

    /// Get the Smallest configuration
    pub fn smallest_config(&self) -> &SmallestTtsConfig {
        &self.config
    }

    /// Build the lightning-v2 streaming WebSocket request body for the
    /// `wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream` endpoint.
    ///
    /// This is the wire body the streaming path sends; it carries the voice params plus the
    /// `max_buffer_flush_ms` buffer-flush latency knob (omitted when unset so the server default
    /// applies). Mirrors [`Self::build_http_request`] for the REST path.
    pub fn build_ws_request(&self, text: &str) -> SmallestWsRequest {
        let mut request = SmallestWsRequest::new(text, &self.config.voice_id)
            .with_language(self.config.language.as_code())
            .with_sample_rate(self.config.sample_rate)
            .with_speed(self.config.speed)
            .with_consistency(self.config.consistency)
            .with_similarity(self.config.similarity)
            .with_enhancement(self.config.enhancement);

        if let Some(ms) = self.config.max_buffer_flush_ms {
            request = request.with_max_buffer_flush_ms(ms);
        }

        request
    }
}

impl TTSRequestBuilder for SmallestRequestBuilder {
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build request body
        let request_body = SmallestTtsRequest::new(text, &self.config.voice_id)
            .with_sample_rate(self.config.sample_rate)
            .with_speed(self.config.speed)
            .with_language(self.config.language.as_code())
            .with_output_format(self.config.format.as_str());

        // Build headers
        let mut headers = HeaderMap::new();

        // Smallest.ai uses Bearer token authentication
        let auth_value = format!("Bearer {}", self.config.api_key);
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&auth_value).unwrap_or_else(|_| HeaderValue::from_static("")),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        debug!(
            "Smallest.ai TTS request: voice={}, model={}, format={}, sample_rate={}, speed={}",
            self.config.voice_id,
            self.config.model,
            self.config.format,
            self.config.sample_rate,
            self.config.speed
        );

        // Build and return the request
        client
            .post(crate::core::tts::standard::override_rest_endpoint(
                self.rest_url(),
                self.config.endpoint_override.as_deref(),
            ))
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
// Config Hash
// =============================================================================

/// Computes a hash of the TTS configuration for cache keying.
fn compute_smallest_tts_config_hash(
    config: &TTSConfig,
    smallest_config: &SmallestTtsConfig,
) -> String {
    let mut s = String::with_capacity(256);

    // Provider identifier
    s.push_str("smallest|");

    // Voice ID
    s.push_str(&smallest_config.voice_id);
    s.push('|');

    // Model
    s.push_str(smallest_config.model.as_str());
    s.push('|');

    // Language
    s.push_str(smallest_config.language.as_code());
    s.push('|');

    // Format
    s.push_str(smallest_config.format.as_str());
    s.push('|');
    s.push_str(&smallest_config.sample_rate.to_string());
    s.push('|');

    // Voice parameters
    s.push_str(&format!("{:.3}", smallest_config.speed));
    s.push('|');
    s.push_str(&format!("{:.3}", smallest_config.consistency));
    s.push('|');
    s.push_str(&format!("{:.3}", smallest_config.similarity));
    s.push('|');
    s.push_str(&smallest_config.enhancement.to_string());
    s.push('|');

    // Speaking rate from base config
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }

    // NOTE: `max_buffer_flush_ms` is deliberately NOT part of this key. It is a streaming
    // buffer-flush latency/chunking knob (lightning-v2 WS), not an audio-content parameter — the
    // synthesized samples are identical regardless of its value — so folding it in would only
    // fragment the cache without correctness benefit (the cache-collision/over-keying review note).

    // Compute xxHash3-128 and format as hex
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

// =============================================================================
// Smallest TTS Provider
// =============================================================================

/// Smallest.ai TTS Provider
///
/// Provides real-time text-to-speech synthesis using Smallest.ai's Waves API.
///
/// # Features
///
/// - **Lightning Model**: Ultra-low latency (~100ms)
/// - **Lightning-Large**: Voice cloning support (~300ms)
/// - **Lightning-V2**: WebSocket streaming (<200ms)
/// - **16+ Languages**: Global language support
/// - **Voice Cloning**: Quick voice cloning capability
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::smallest::SmallestTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("emily".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = SmallestTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// ```
pub struct SmallestTts {
    /// Generic HTTP TTS provider (handles connection pooling, queuing, callbacks)
    provider: TTSProvider,
    /// Smallest-specific configuration
    smallest_config: SmallestTtsConfig,
    /// Base TTS configuration
    base_config: TTSConfig,
    /// Precomputed configuration hash for cache keying
    config_hash: String,
}

impl SmallestTts {
    /// Create request builder for this provider
    fn create_request_builder(&self) -> SmallestRequestBuilder {
        SmallestRequestBuilder::new(self.smallest_config.clone(), self.base_config.clone())
    }

    /// Validate text length before synthesis
    pub fn validate_text(text: &str) -> TTSResult<()> {
        if text.len() > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text too long: {} characters (max: {})",
                text.len(),
                MAX_TEXT_LENGTH
            )));
        }
        Ok(())
    }

    /// List available voices from Smallest.ai API
    ///
    /// # Arguments
    /// * `api_key` - The API key for authentication
    /// * `model` - The model to list voices for
    ///
    /// # Returns
    /// * `TTSResult<Vec<SmallestVoice>>` - List of available voices
    pub async fn list_voices(
        api_key: &str,
        model: &super::config::SmallestModel,
    ) -> TTSResult<Vec<SmallestVoice>> {
        let client = reqwest::Client::new();
        let url = voices_url(model);

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

        let voices_response: SmallestVoicesResponse = response.json().await.map_err(|e| {
            TTSError::InternalError(format!("Failed to parse voices response: {}", e))
        })?;

        Ok(voices_response.voices)
    }

    /// Get the Smallest-specific configuration
    pub fn smallest_config(&self) -> &SmallestTtsConfig {
        &self.smallest_config
    }

    /// Get the REST endpoint URL being used
    pub fn rest_url(&self) -> &str {
        SMALLEST_TTS_URL
    }

    /// Sets the request manager for connection pooling.
    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }

    /// Sets the voice ID.
    pub fn set_voice_id(&mut self, voice_id: impl Into<String>) {
        self.smallest_config.voice_id = voice_id.into();
        self.recompute_config_hash();
    }

    /// Sets the model.
    pub fn set_model(&mut self, model: SmallestModel) {
        self.smallest_config.model = model;
        self.recompute_config_hash();
    }

    /// Sets the language.
    pub fn set_language(&mut self, language: SmallestLanguage) {
        self.smallest_config.language = language;
        self.recompute_config_hash();
    }

    /// Sets the speed.
    pub fn set_speed(&mut self, speed: f32) {
        let max_speed = self.smallest_config.model.max_speed();
        self.smallest_config.speed = speed.clamp(super::MIN_SPEED, max_speed);
        self.recompute_config_hash();
    }

    /// Sets the sample rate.
    pub fn set_sample_rate(&mut self, rate: u32) {
        self.smallest_config.sample_rate = if super::is_valid_sample_rate(rate) {
            rate
        } else {
            super::nearest_sample_rate(rate)
        };
        self.recompute_config_hash();
    }

    /// Sets the audio format.
    pub fn set_format(&mut self, format: SmallestOutputFormat) {
        self.smallest_config.format = format;
        self.recompute_config_hash();
    }

    /// Sets the consistency parameter.
    pub fn set_consistency(&mut self, consistency: f32) {
        self.smallest_config.consistency =
            consistency.clamp(super::MIN_CONSISTENCY, super::MAX_CONSISTENCY);
        self.recompute_config_hash();
    }

    /// Sets the similarity parameter.
    pub fn set_similarity(&mut self, similarity: f32) {
        self.smallest_config.similarity =
            similarity.clamp(super::MIN_SIMILARITY, super::MAX_SIMILARITY);
        self.recompute_config_hash();
    }

    /// Sets the enhancement level.
    pub fn set_enhancement(&mut self, enhancement: u8) {
        self.smallest_config.enhancement =
            enhancement.clamp(super::MIN_ENHANCEMENT, super::MAX_ENHANCEMENT);
        self.recompute_config_hash();
    }

    /// Recomputes the config hash after parameter changes.
    fn recompute_config_hash(&mut self) {
        self.config_hash =
            compute_smallest_tts_config_hash(&self.base_config, &self.smallest_config);
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// Mirrors `DeepgramTTS::from_standard`: maps the standardized features onto Smallest's
    /// provider config via [`SmallestTtsConfig::from_standard`] (speed, stability -> consistency,
    /// similarity_boost -> similarity, language, sample_rate + the enhancement extra), then
    /// constructs the provider mirroring [`BaseTTS::new`]. Capability gaps (pitch, volume, style,
    /// emotion, instructions, ssml, seed, …) have no Smallest field and stay at their defaults.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> TTSResult<Self> {
        let smallest_config = SmallestTtsConfig::from_standard(std)?;
        smallest_config.validate()?;

        let base_config = std.base.clone();
        let config_hash = compute_smallest_tts_config_hash(&base_config, &smallest_config);

        Ok(Self {
            provider: TTSProvider::new()?,
            smallest_config,
            base_config,
            config_hash,
        })
    }
}

#[async_trait]
impl BaseTTS for SmallestTts {
    /// Create a new Smallest.ai TTS provider
    fn new(config: TTSConfig) -> TTSResult<Self>
    where
        Self: Sized,
    {
        // Parse Smallest-specific configuration from base config
        let smallest_config = SmallestTtsConfig::from_base(config.clone())?;

        // Validate configuration
        smallest_config.validate()?;

        info!(
            "Creating Smallest.ai TTS provider: voice={}, model={}, format={}, sample_rate={}",
            smallest_config.voice_id,
            smallest_config.model,
            smallest_config.format,
            smallest_config.sample_rate
        );

        // Compute config hash for caching
        let config_hash = compute_smallest_tts_config_hash(&config, &smallest_config);

        Ok(Self {
            provider: TTSProvider::new()?,
            smallest_config,
            base_config: config,
            config_hash,
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
            "Connecting Smallest.ai TTS provider to {}",
            SMALLEST_TTS_URL
        );
        self.provider
            .generic_connect_with_config(SMALLEST_TTS_URL, &self.base_config)
            .await
    }

    /// Disconnect from the TTS provider.
    async fn disconnect(&mut self) -> TTSResult<()> {
        self.provider.generic_disconnect().await
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
        Self::validate_text(text)?;

        // Auto-reconnect if needed
        if !self.is_ready() {
            info!("Smallest.ai TTS not ready, attempting to connect...");
            self.connect().await?;
        }

        // Set config hash once on first speak (idempotent)
        self.provider
            .set_tts_config_hash(self.config_hash.clone())
            .await;

        debug!(
            "Smallest.ai TTS speak: text='{}', flush={}, voice={}, model={}",
            text, flush, self.smallest_config.voice_id, self.smallest_config.model
        );

        let request_builder = self.create_request_builder();
        self.provider
            .generic_speak(request_builder, text, flush)
            .await
    }

    /// Clear any queued text from the synthesis queue.
    async fn clear(&mut self) -> TTSResult<()> {
        self.provider.generic_clear().await
    }

    /// Flush any pending synthesis
    async fn flush(&self) -> TTSResult<()> {
        self.provider.generic_flush().await
    }

    /// Register an audio callback.
    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.provider.generic_on_audio(callback)
    }

    /// Remove the registered audio callback.
    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        self.provider.generic_remove_audio_callback()
    }

    /// Check if the provider is ready for synthesis
    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    /// Get the current connection state
    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }

    /// Get provider-specific information.
    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "smallest",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "endpoint": SMALLEST_TTS_URL,
            "voice_id": self.smallest_config.voice_id,
            "model": self.smallest_config.model.as_str(),
            "language": self.smallest_config.language.as_code(),
            "format": self.smallest_config.format.as_str(),
            "sample_rate": self.smallest_config.sample_rate,
            "speed": self.smallest_config.speed,
            "consistency": self.smallest_config.consistency,
            "similarity": self.smallest_config.similarity,
            "enhancement": self.smallest_config.enhancement,
            "supported_formats": ["pcm", "wav", "mp3", "mulaw"],
            "supported_sample_rates": SUPPORTED_SAMPLE_RATES,
            "features": {
                "voice_cloning": self.smallest_config.model.supports_voice_cloning(),
                "websocket_streaming": self.smallest_config.model.supports_websocket(),
                "max_text_length": MAX_TEXT_LENGTH,
                "speed_range": [super::MIN_SPEED, self.smallest_config.model.max_speed()],
                "ultra_low_latency": true,
                "multi_language": true,
            },
            "supported_languages": [
                "en", "hi", "mr", "kn", "ta", "bn", "gu", "de", "fr", "es", "it", "pl", "nl", "ru", "ar", "he"
            ],
            "models": {
                "lightning": {
                    "latency_ms": 100,
                    "api": "REST",
                    "voice_cloning": false,
                },
                "lightning-large": {
                    "latency_ms": 300,
                    "api": "REST",
                    "voice_cloning": true,
                },
                "lightning-v2": {
                    "latency_ms": 200,
                    "api": "WebSocket",
                    "voice_cloning": true,
                },
            },
            "documentation": "https://waves-docs.smallest.ai"
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::smallest::{SmallestModel, SmallestOutputFormat};

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "smallest".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("emily".to_string()),
            model: "lightning".to_string(),
            speaking_rate: Some(1.0),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(24000),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        }
    }

    // The provider struct's `from_standard` (mirroring `DeepgramTTS::from_standard`) maps a
    // standardized advanced feature (speed) all the way onto the provider config the request
    // builder reads — proving the standardized dispatch path reaches the struct, not just the
    // config-level method.
    #[test]
    fn from_standard_maps_speed_onto_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                speed: Some(1.5),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = SmallestTts::from_standard(&std).unwrap();
        assert_eq!(tts.smallest_config().speed, 1.5);
    }

    // WIRE-LEVEL: `max_buffer_flush_ms` (from the extras passthrough) must reach the actual
    // lightning-v2 streaming WS request BODY that is sent to
    // wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream — not merely the config
    // struct. Asserts on the serialized JSON the streaming endpoint receives, guarding the
    // recurring "config set but never serialized to the wire" bug class.
    #[test]
    fn max_buffer_flush_ms_reaches_streaming_ws_request_body() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("max_buffer_flush_ms".into(), serde_json::json!(250));
        let std = StandardTTSConfig {
            base: {
                let mut c = create_test_config();
                c.model = "lightning-v2".to_string();
                c
            },
            features: TtsFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let tts = SmallestTts::from_standard(&std).unwrap();
        assert_eq!(tts.smallest_config().max_buffer_flush_ms, Some(250));

        // Build the actual streaming WS request body and serialize it to the wire JSON.
        let builder = tts.create_request_builder();
        assert_eq!(
            builder.ws_url(),
            "wss://waves-api.smallest.ai/api/v1/lightning-v2/get_speech/stream"
        );
        let ws_request = builder.build_ws_request("Hello streaming");
        let json = serde_json::to_string(&ws_request).unwrap();
        // The exact wire param name with the configured value must be present in the body.
        assert!(
            json.contains("\"max_buffer_flush_ms\":250"),
            "max_buffer_flush_ms missing from streaming WS body: {json}"
        );
    }

    // WIRE-LEVEL counterpart: when unset, `max_buffer_flush_ms` must be OMITTED from the WS body
    // (serde skip_serializing_if) so the server default applies — no spurious value on the wire.
    #[test]
    fn max_buffer_flush_ms_omitted_from_ws_body_when_unset() {
        let mut config = create_test_config();
        config.model = "lightning-v2".to_string();
        let tts = SmallestTts::new(config).unwrap();

        let builder = tts.create_request_builder();
        let ws_request = builder.build_ws_request("Hello");
        let json = serde_json::to_string(&ws_request).unwrap();
        assert!(
            !json.contains("max_buffer_flush_ms"),
            "max_buffer_flush_ms must be omitted when unset: {json}"
        );
    }

    // =========================================================================
    // Provider Creation Tests
    // =========================================================================

    #[test]
    fn test_smallest_tts_creation() {
        let config = create_test_config();
        let tts = SmallestTts::new(config);
        assert!(tts.is_ok());

        let tts = tts.unwrap();
        assert_eq!(tts.smallest_config().voice_id, "emily");
        assert_eq!(tts.smallest_config().model, SmallestModel::Lightning);
        assert_eq!(tts.smallest_config().format, SmallestOutputFormat::Wav);
        assert!(!tts.is_ready());
    }

    #[test]
    fn test_smallest_tts_requires_api_key() {
        let mut config = create_test_config();
        config.api_key = String::new();

        let result = SmallestTts::new(config);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("API") || msg.contains("api_key") || msg.contains("required"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_smallest_tts_default_voice() {
        let mut config = create_test_config();
        config.voice_id = None;

        let tts = SmallestTts::new(config).unwrap();
        assert_eq!(
            tts.smallest_config().voice_id,
            super::super::DEFAULT_VOICE_ID
        );
    }

    // =========================================================================
    // Request Builder Tests
    // =========================================================================

    #[test]
    fn test_request_builder_creation() {
        let config = create_test_config();
        let smallest_config = SmallestTtsConfig::from_base(config.clone()).unwrap();
        let builder = SmallestRequestBuilder::new(smallest_config, config);

        assert!(builder.rest_url().contains("waves-api.smallest.ai"));
        assert!(builder.rest_url().contains("lightning"));
    }

    #[test]
    fn test_request_builder_with_model() {
        let mut config = create_test_config();
        config.model = "lightning-v2".to_string();

        let smallest_config = SmallestTtsConfig::from_base(config.clone()).unwrap();
        let builder = SmallestRequestBuilder::new(smallest_config, config);

        assert_eq!(builder.smallest_config().model, SmallestModel::LightningV2);
    }

    #[test]
    fn test_request_builder_with_pronunciations() {
        use crate::core::tts::base::Pronunciation;

        let mut config = create_test_config();
        config.pronunciations = vec![
            Pronunciation {
                word: "API".to_string(),
                pronunciation: "A P I".to_string(),
            },
            Pronunciation {
                word: "TTS".to_string(),
                pronunciation: "text to speech".to_string(),
            },
        ];

        let smallest_config = SmallestTtsConfig::from_base(config.clone()).unwrap();
        let builder = SmallestRequestBuilder::new(smallest_config, config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    // =========================================================================
    // Text Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_text_success() {
        let text = "Hello, world!";
        assert!(SmallestTts::validate_text(text).is_ok());
    }

    #[test]
    fn test_validate_text_too_long() {
        let text = "a".repeat(MAX_TEXT_LENGTH + 1);
        let result = SmallestTts::validate_text(&text);
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains("too long") || msg.contains(&MAX_TEXT_LENGTH.to_string()));
        } else {
            panic!("Expected InvalidConfiguration error");
        }
    }

    #[test]
    fn test_validate_text_at_limit() {
        let text = "a".repeat(MAX_TEXT_LENGTH);
        assert!(SmallestTts::validate_text(&text).is_ok());
    }

    // =========================================================================
    // Connection State Tests
    // =========================================================================

    #[tokio::test]
    async fn test_smallest_tts_connect_disconnect() {
        let config = create_test_config();
        let mut tts = SmallestTts::new(config).unwrap();

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
    async fn test_smallest_tts_auto_connect_on_speak() {
        let config = create_test_config();
        let mut tts = SmallestTts::new(config).unwrap();

        // Should auto-connect when not connected
        assert!(!tts.is_ready());

        // Speak should trigger auto-connect
        let result = tts.speak("Hello", true).await;
        assert!(result.is_ok());
        assert!(tts.is_ready());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_smallest_tts_empty_text() {
        let config = create_test_config();
        let mut tts = SmallestTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Empty text should succeed without making a request
        let result = tts.speak("", true).await;
        assert!(result.is_ok());

        let result = tts.speak("   ", true).await;
        assert!(result.is_ok());

        tts.disconnect().await.unwrap();
    }

    #[tokio::test]
    async fn test_smallest_tts_text_too_long() {
        let config = create_test_config();
        let mut tts = SmallestTts::new(config).unwrap();
        tts.connect().await.unwrap();

        // Text over MAX_TEXT_LENGTH characters should fail
        let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
        let result = tts.speak(&long_text, true).await;
        assert!(result.is_err());

        if let Err(TTSError::InvalidConfiguration(msg)) = result {
            assert!(msg.contains(&MAX_TEXT_LENGTH.to_string()) || msg.contains("too long"));
        } else {
            panic!("Expected InvalidConfiguration error");
        }

        tts.disconnect().await.unwrap();
    }

    // =========================================================================
    // URL Tests
    // =========================================================================

    #[test]
    fn test_rest_url() {
        let config = create_test_config();
        let tts = SmallestTts::new(config).unwrap();

        assert_eq!(
            tts.rest_url(),
            "https://waves-api.smallest.ai/api/v1/lightning/get_speech"
        );
    }

    // =========================================================================
    // Config Access Tests
    // =========================================================================

    #[test]
    fn test_smallest_config_access() {
        let config = create_test_config();
        let tts = SmallestTts::new(config).unwrap();

        let smallest_config = tts.smallest_config();
        assert_eq!(smallest_config.voice_id, "emily");
        assert_eq!(smallest_config.model, SmallestModel::Lightning);
        assert_eq!(smallest_config.format, SmallestOutputFormat::Wav);
        assert_eq!(smallest_config.sample_rate, 24000);
    }

    // =========================================================================
    // Request Serialization Tests
    // =========================================================================

    #[test]
    fn test_request_body_serialization() {
        let request = SmallestTtsRequest::new("Hello, world!", "emily")
            .with_sample_rate(24000)
            .with_speed(1.0)
            .with_language("en")
            .with_output_format("wav");

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"text\":\"Hello, world!\""));
        assert!(json.contains("\"voice_id\":\"emily\""));
        assert!(json.contains("\"sample_rate\":24000"));
        assert!(json.contains("\"speed\":1"));
        assert!(json.contains("\"language\":\"en\""));
        assert!(json.contains("\"output_format\":\"wav\""));
    }

    #[test]
    fn test_request_body_with_unicode() {
        let request = SmallestTtsRequest::new("नमस्ते दुनिया", "emily").with_language("hi");

        let json = serde_json::to_string(&request).unwrap();

        assert!(json.contains("\"text\":\"नमस्ते दुनिया\""));
        assert!(json.contains("\"language\":\"hi\""));
    }
}
