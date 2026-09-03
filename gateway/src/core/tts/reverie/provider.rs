//! Reverie TTS Request Builder and Provider Implementation
//!
//! This module implements the `TTSRequestBuilder` trait for Reverie TTS, which builds
//! HTTP POST requests with proper headers, authentication, and JSON body.
//!
//! # Architecture
//!
//! The `ReverieRequestBuilder` constructs HTTP requests for the Reverie TTS API:
//! - URL: `https://revapi.reverieinc.com/`
//! - Authentication: Custom headers (REV-API-KEY, REV-APP-ID, REV-APPNAME, speaker)
//! - Content-Type: `application/json`
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::reverie::ReverieTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-api-key".to_string(),
//!     voice_id: Some("hi_female".to_string()),
//!     model: "your-app-id".to_string(),
//!     ..Default::default()
//! };
//!
//! let mut tts = ReverieTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("नमस्ते, दुनिया!", true).await?;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, info};
use xxhash_rust::xxh3::xxh3_128;

use super::config::ReverieTtsConfig;
use super::{DEFAULT_PITCH, DEFAULT_SPEED, MAX_TEXT_LENGTH, REVERIE_TTS_URL, TTS_APP_NAME};
use crate::core::tts::base::{
    AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

// =============================================================================
// ReverieRequestBuilder
// =============================================================================

/// Reverie-specific request builder for constructing TTS API requests.
///
/// Implements `TTSRequestBuilder` to construct HTTP POST requests for the
/// Reverie Text-to-Speech REST API with proper headers and JSON body.
#[derive(Clone)]
pub struct ReverieRequestBuilder {
    /// Base TTS configuration.
    config: TTSConfig,

    /// Reverie-specific configuration.
    reverie_config: ReverieTtsConfig,

    /// Pre-compiled pronunciation replacement patterns.
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl ReverieRequestBuilder {
    /// Creates a new Reverie request builder.
    pub fn new(config: TTSConfig, reverie_config: ReverieTtsConfig) -> Self {
        // Pre-compile pronunciation replacer if pronunciations are configured
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        Self {
            config,
            reverie_config,
            pronunciation_replacer,
        }
    }

    /// Builds the request body as JSON.
    fn build_request_body(&self, text: &str) -> serde_json::Value {
        // SSML input mode routes the input under the `ssml` body key (Reverie's documented SSML
        // input), otherwise the input is the plain-text `text` array.
        let mut body = if self.reverie_config.ssml_input {
            json!({ "ssml": text })
        } else {
            json!({ "text": [text] })
        };

        // Add speed if non-default
        if (self.reverie_config.speed - DEFAULT_SPEED).abs() > 0.001 {
            body["speed"] = json!(self.reverie_config.speed);
        }

        // Add pitch if non-default
        if (self.reverie_config.pitch - DEFAULT_PITCH).abs() > 0.001 {
            body["pitch"] = json!(self.reverie_config.pitch);
        }

        // Add sample rate
        body["sample_rate"] = json!(self.reverie_config.sample_rate);

        // Add format
        body["format"] = json!(self.reverie_config.format.as_str());

        body
    }
}

impl TTSRequestBuilder for ReverieRequestBuilder {
    /// Build the Reverie TTS HTTP request with URL, headers, and JSON body.
    ///
    /// # Request Format
    ///
    /// **URL**: `https://revapi.reverieinc.com/`
    /// **Method**: POST
    ///
    /// **Headers**:
    /// | Header | Value | Purpose |
    /// |--------|-------|---------|
    /// | REV-API-KEY | {api_key} | Authentication |
    /// | REV-APP-ID | {app_id} | Application identification |
    /// | REV-APPNAME | tts | Service type |
    /// | speaker | {speaker_code} | Voice selection |
    /// | Content-Type | application/json | Request body format |
    ///
    /// **Body**:
    /// ```json
    /// {
    ///   "text": ["नमस्ते, दुनिया!"],
    ///   "speed": 1.0,
    ///   "pitch": 0,
    ///   "sample_rate": 22050,
    ///   "format": "WAV"
    /// }
    /// ```
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        let body = self.build_request_body(text);

        debug!(
            "Building Reverie TTS request: speaker={}, format={:?}, sample_rate={}, speed={}",
            self.reverie_config.speaker_code(),
            self.reverie_config.format,
            self.reverie_config.sample_rate,
            self.reverie_config.speed
        );

        // Build the HTTP request with all required headers
        client
            .post(crate::core::tts::standard::override_rest_endpoint(
                REVERIE_TTS_URL,
                self.reverie_config.endpoint_override.as_deref(),
            ))
            .header("REV-API-KEY", &self.config.api_key)
            .header("REV-APP-ID", &self.reverie_config.app_id)
            .header("REV-APPNAME", TTS_APP_NAME)
            .header("speaker", self.reverie_config.speaker_code())
            .header("Content-Type", "application/json")
            .header("Accept", self.reverie_config.format.content_type())
            .json(&body)
    }

    /// Returns a reference to the base TTS configuration.
    #[inline]
    fn get_config(&self) -> &TTSConfig {
        &self.config
    }

    /// Returns the precompiled pronunciation replacer if configured.
    #[inline]
    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Config Hash
// =============================================================================

/// Computes a hash of the TTS configuration for cache keying.
fn compute_reverie_tts_config_hash(
    config: &TTSConfig,
    reverie_config: &ReverieTtsConfig,
) -> String {
    let mut s = String::with_capacity(256);

    // Provider identifier
    s.push_str("reverie|");

    // Speaker
    s.push_str(&reverie_config.speaker_code());
    s.push('|');

    // Format
    s.push_str(reverie_config.format.as_str());
    s.push('|');
    s.push_str(&reverie_config.sample_rate.to_string());
    s.push('|');

    // Voice parameters
    s.push_str(&format!("{:.3}", reverie_config.speed));
    s.push('|');
    s.push_str(&format!("{:.3}", reverie_config.pitch));
    s.push('|');

    // SSML input mode changes how the SAME text is synthesized (markup vs literal), so it must be
    // part of the cache key to avoid serving a literal-text render for an SSML request (or vice
    // versa) — the prior review's audio cache-collision bug class.
    s.push_str(if reverie_config.ssml_input {
        "ssml|"
    } else {
        "text|"
    });

    // Speaking rate from base config
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }

    // Compute xxHash3-128 and format as hex
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

// =============================================================================
// ReverieTts Provider
// =============================================================================

/// Reverie Text-to-Speech provider implementation.
///
/// Uses the Reverie TTS REST API optimized for Indian languages with 36+ voices.
///
/// # Key Features
///
/// - **22+ Indian Languages**: Hindi, Tamil, Telugu, Bengali, and more
/// - **36+ Voices**: Male and female voices for each language
/// - **SSML Support**: Custom voice control via SSML markup
/// - **Adjustable Parameters**: Speed (0.5-1.5) and pitch (-3 to +3)
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::reverie::ReverieTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("hi_female".to_string()),
///     model: "your-app-id".to_string(),
///     ..Default::default()
/// };
///
/// let mut tts = ReverieTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("नमस्ते!", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct ReverieTts {
    /// Generic HTTP-based TTS provider for connection pooling and streaming.
    provider: TTSProvider,

    /// Reverie-specific request builder.
    request_builder: ReverieRequestBuilder,

    /// Precomputed configuration hash for cache keying.
    config_hash: String,
}

impl ReverieTts {
    /// Creates a new Reverie TTS provider instance.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration with API key and voice settings
    ///
    /// The `api_key` field should contain the Reverie API key.
    /// The `model` field should contain the App ID (or set REVERIE_APP_ID env var).
    /// The `voice_id` field should contain the speaker code (e.g., "hi_female").
    ///
    /// # Returns
    /// * `Ok(Self)` - A new provider instance ready for connection
    /// * `Err(TTSError::InvalidConfiguration)` - If configuration is invalid
    pub fn create(config: TTSConfig) -> TTSResult<Self> {
        // Create Reverie-specific config
        let reverie_config =
            ReverieTtsConfig::from_base(config.clone()).map_err(TTSError::InvalidConfiguration)?;

        // Validate configuration
        if let Err(e) = reverie_config.validate() {
            return Err(TTSError::InvalidConfiguration(e));
        }

        // Create request builder
        let request_builder = ReverieRequestBuilder::new(config.clone(), reverie_config.clone());

        // Compute config hash for caching
        let config_hash = compute_reverie_tts_config_hash(&config, &reverie_config);

        info!(
            "Created ReverieTts provider: speaker={}, format={:?}, sample_rate={}",
            reverie_config.speaker_code(),
            reverie_config.format,
            reverie_config.sample_rate
        );

        Ok(Self {
            provider: TTSProvider::new(),
            request_builder,
            config_hash,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// Mirrors `DeepgramTTS::from_standard`: maps the standardized features onto Reverie's
    /// provider config via [`ReverieTtsConfig::from_standard`] (speed, pitch, sample_rate,
    /// language + the `format` extra), then constructs the provider via [`Self::with_config`].
    /// Capability gaps (volume, stability, emotion, instructions, ssml, seed, …) have no Reverie
    /// field and stay at their defaults — never fabricated.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let reverie_config =
            ReverieTtsConfig::from_standard(std).map_err(TTSError::InvalidConfiguration)?;
        Self::with_config(reverie_config)
    }

    /// Creates a new Reverie TTS provider with custom configuration.
    pub fn with_config(reverie_config: ReverieTtsConfig) -> TTSResult<Self> {
        // Validate configuration
        if let Err(e) = reverie_config.validate() {
            return Err(TTSError::InvalidConfiguration(e));
        }

        let config = reverie_config.base.clone();
        let request_builder = ReverieRequestBuilder::new(config.clone(), reverie_config.clone());
        let config_hash = compute_reverie_tts_config_hash(&config, &reverie_config);

        info!(
            "Created ReverieTts provider with config: speaker={}, speed={}, pitch={}",
            reverie_config.speaker_code(),
            reverie_config.speed,
            reverie_config.pitch
        );

        Ok(Self {
            provider: TTSProvider::new(),
            request_builder,
            config_hash,
        })
    }

    /// Sets the request manager for connection pooling.
    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }

    /// Returns a reference to the Reverie-specific configuration.
    #[inline]
    pub fn reverie_config(&self) -> &ReverieTtsConfig {
        &self.request_builder.reverie_config
    }

    /// Sets the speaker.
    pub fn set_speaker(&mut self, speaker: super::config::ReverieSpeaker) {
        self.request_builder.reverie_config.speaker = speaker;
        self.recompute_config_hash();
    }

    /// Sets the speaker from a code string.
    pub fn set_speaker_code(&mut self, code: &str) -> Result<(), String> {
        self.request_builder.reverie_config.speaker =
            super::config::ReverieSpeaker::from_code(code)?;
        self.recompute_config_hash();
        Ok(())
    }

    /// Sets the speed.
    pub fn set_speed(&mut self, speed: f32) {
        self.request_builder.reverie_config.speed = speed.clamp(super::MIN_SPEED, super::MAX_SPEED);
        self.recompute_config_hash();
    }

    /// Sets the pitch.
    pub fn set_pitch(&mut self, pitch: f32) {
        self.request_builder.reverie_config.pitch = pitch.clamp(super::MIN_PITCH, super::MAX_PITCH);
        self.recompute_config_hash();
    }

    /// Sets the sample rate.
    pub fn set_sample_rate(&mut self, rate: u32) {
        self.request_builder.reverie_config.sample_rate = if super::is_valid_sample_rate(rate) {
            rate
        } else {
            super::nearest_sample_rate(rate)
        };
        self.recompute_config_hash();
    }

    /// Sets the audio format.
    pub fn set_format(&mut self, format: super::config::ReverieTtsAudioFormat) {
        self.request_builder.reverie_config.format = format;
        self.recompute_config_hash();
    }

    /// Recomputes the config hash after parameter changes.
    fn recompute_config_hash(&mut self) {
        self.config_hash = compute_reverie_tts_config_hash(
            &self.request_builder.config,
            &self.request_builder.reverie_config,
        );
    }

    /// Validates text length before synthesis.
    fn validate_text(text: &str) -> TTSResult<()> {
        if text.len() > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters (got {})",
                MAX_TEXT_LENGTH,
                text.len()
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl BaseTTS for ReverieTts {
    /// Create a new instance of the TTS provider.
    fn new(config: TTSConfig) -> TTSResult<Self> {
        ReverieTts::create(config)
    }

    /// Get the underlying TTSProvider for HTTP-based providers.
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the TTS provider.
    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(REVERIE_TTS_URL, &self.request_builder.config)
            .await?;

        info!("Reverie TTS provider connected and ready");
        Ok(())
    }

    /// Disconnect from the TTS provider.
    async fn disconnect(&mut self) -> TTSResult<()> {
        self.provider.generic_disconnect().await
    }

    /// Check if the TTS provider is ready to process requests.
    #[inline]
    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    /// Get the current connection state.
    #[inline]
    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }

    /// Send text to the TTS provider for synthesis.
    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        // Validate text length
        Self::validate_text(text)?;

        // Auto-reconnect if needed
        if !self.is_ready() {
            info!("Reverie TTS not ready, attempting to connect...");
            self.connect().await?;
        }

        // Set config hash once on first speak (idempotent)
        self.provider
            .set_tts_config_hash(self.config_hash.clone())
            .await;

        // Delegate to generic provider
        self.provider
            .generic_speak(self.request_builder.clone(), text, flush)
            .await
    }

    /// Clear any queued text from the synthesis queue.
    async fn clear(&mut self) -> TTSResult<()> {
        self.provider.generic_clear().await
    }

    /// Flush the TTS provider queue.
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

    /// Get provider-specific information.
    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "reverie",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "endpoint": REVERIE_TTS_URL,
            "speaker": self.request_builder.reverie_config.speaker_code(),
            "format": self.request_builder.reverie_config.format.as_str(),
            "sample_rate": self.request_builder.reverie_config.sample_rate,
            "speed": self.request_builder.reverie_config.speed,
            "pitch": self.request_builder.reverie_config.pitch,
            "supported_formats": ["WAV", "MP3"],
            "supported_sample_rates": super::SUPPORTED_SAMPLE_RATES,
            "features": {
                "ssml_support": true,
                "max_text_length": MAX_TEXT_LENGTH,
                "speed_range": [super::MIN_SPEED, super::MAX_SPEED],
                "pitch_range": [super::MIN_PITCH, super::MAX_PITCH],
                "indian_language_optimization": true,
            },
            "supported_languages": [
                "hi", "en", "ta", "te", "bn", "mr", "gu", "kn", "ml", "pa", "or", "as", "ur"
            ],
            "documentation": "https://docs.reverieinc.com/endpoints/text-to-speech"
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::reverie::config::{ReverieSpeaker, ReverieTtsAudioFormat};
    use crate::core::tts::reverie::{MAX_PITCH, MAX_SPEED, MIN_PITCH, MIN_SPEED};

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "reverie".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("hi_female".to_string()),
            model: "test-app-id".to_string(),
            speaking_rate: Some(1.0),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(22050),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        }
    }

    // =========================================================================
    // from_standard (W1 keystone) Tests
    // =========================================================================

    // The provider struct's `from_standard` (mirroring `DeepgramTTS::from_standard`) maps a
    // standardized advanced feature (speed -> Reverie's speed multiplier) all the way onto the
    // provider config the request builder reads — proving the standardized dispatch path reaches
    // the struct, not just the config-level method.
    #[test]
    fn from_standard_maps_speed_onto_provider_config() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                speed: Some(1.4),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = ReverieTts::from_standard(&std).unwrap();
        assert_eq!(tts.reverie_config().speed, 1.4);
    }

    // WIRE-LEVEL (S1/S5 recurring bug class): the typed `ssml` feature must route the input into the
    // `ssml` BODY key (not the `text` array) of the POST body that hits revapi.reverieinc.com — not
    // merely flip a config bool. Build the real request body and assert the wire shape.
    #[test]
    fn ssml_feature_routes_input_to_ssml_body_key() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                ssml: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = ReverieTts::from_standard(&std).unwrap();
        // (1) config carries the flag …
        assert!(tts.reverie_config().ssml_input);

        // (2) … AND the built request body carries the input under `ssml`, with NO `text` array.
        let body = tts
            .request_builder
            .build_request_body("<speak>नमस्ते</speak>");
        assert_eq!(body["ssml"], "<speak>नमस्ते</speak>");
        assert!(
            body.get("text").is_none(),
            "SSML input mode must omit the plain-text `text` array"
        );
    }

    // Default path stays plain-text: without the ssml feature the body keeps the `text` array shape.
    #[test]
    fn plain_text_body_when_ssml_not_requested() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();
        let builder = ReverieRequestBuilder::new(config, reverie_config);
        let body = builder.build_request_body("नमस्ते");
        assert_eq!(body["text"], json!(["नमस्ते"]));
        assert!(body.get("ssml").is_none());
    }

    // Cache-collision guard (prior review's bug class): the SAME text under SSML-on vs SSML-off must
    // produce DIFFERENT config hashes, since SSML changes how the audio is synthesized.
    #[test]
    fn config_hash_differs_for_ssml_input_mode() {
        let config = create_test_config();
        let plain = ReverieTtsConfig::from_base(config.clone()).unwrap();
        let mut ssml = plain.clone();
        ssml.ssml_input = true;

        let hash_plain = compute_reverie_tts_config_hash(&config, &plain);
        let hash_ssml = compute_reverie_tts_config_hash(&config, &ssml);
        assert_ne!(
            hash_plain, hash_ssml,
            "ssml_input must change the cache key to avoid serving a literal-text render for SSML"
        );
    }

    // =========================================================================
    // ReverieRequestBuilder Tests
    // =========================================================================

    #[test]
    fn test_request_builder_new() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();

        let builder = ReverieRequestBuilder::new(config, reverie_config);

        assert!(builder.pronunciation_replacer.is_none());
    }

    #[test]
    fn test_request_builder_get_config() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();
        let builder = ReverieRequestBuilder::new(config, reverie_config);

        let retrieved = builder.get_config();
        assert_eq!(retrieved.voice_id, Some("hi_female".to_string()));
    }

    #[test]
    fn test_request_builder_build_request_body() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();
        let builder = ReverieRequestBuilder::new(config, reverie_config);

        let body = builder.build_request_body("नमस्ते");

        assert_eq!(body["text"], json!(["नमस्ते"]));
        assert_eq!(body["sample_rate"], 22050);
        assert_eq!(body["format"], "WAV");
    }

    #[test]
    fn test_request_builder_build_request_body_with_speed() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone())
            .unwrap()
            .with_speed(1.3);
        let builder = ReverieRequestBuilder::new(config, reverie_config);

        let body = builder.build_request_body("Test");

        let speed = body["speed"].as_f64().unwrap();
        assert!((speed - 1.3).abs() < 0.001);
    }

    // =========================================================================
    // HTTP Request Building Tests
    // =========================================================================

    #[test]
    fn test_build_http_request_url() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();
        let builder = ReverieRequestBuilder::new(config, reverie_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello");
        let request = request_builder.build().unwrap();

        assert_eq!(request.url().as_str(), REVERIE_TTS_URL);
        assert_eq!(request.method(), reqwest::Method::POST);
    }

    #[test]
    fn test_build_http_request_headers() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();
        let builder = ReverieRequestBuilder::new(config, reverie_config);

        let client = reqwest::Client::new();
        let request = builder
            .build_http_request(&client, "Hello")
            .build()
            .unwrap();

        // Verify REV-API-KEY header
        let api_key_header = request.headers().get("rev-api-key").unwrap();
        assert_eq!(api_key_header.to_str().unwrap(), "test-api-key");

        // Verify REV-APP-ID header
        let app_id_header = request.headers().get("rev-app-id").unwrap();
        assert_eq!(app_id_header.to_str().unwrap(), "test-app-id");

        // Verify REV-APPNAME header
        let appname_header = request.headers().get("rev-appname").unwrap();
        assert_eq!(appname_header.to_str().unwrap(), "tts");

        // Verify speaker header
        let speaker_header = request.headers().get("speaker").unwrap();
        assert_eq!(speaker_header.to_str().unwrap(), "hi_female");

        // Verify Content-Type header
        let content_type = request.headers().get("content-type").unwrap();
        assert_eq!(content_type.to_str().unwrap(), "application/json");
    }

    // =========================================================================
    // Config Hash Tests
    // =========================================================================

    #[test]
    fn test_compute_config_hash() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();

        let hash = compute_reverie_tts_config_hash(&config, &reverie_config);

        // Hash should be 32-char hex
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Same config produces same hash
        let hash2 = compute_reverie_tts_config_hash(&config, &reverie_config);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_config_hash_different_speaker() {
        let config = create_test_config();
        let reverie_config1 = ReverieTtsConfig::from_base(config.clone()).unwrap();

        let mut config2 = config.clone();
        config2.voice_id = Some("en_male".to_string());
        let reverie_config2 = ReverieTtsConfig::from_base(config2.clone()).unwrap();

        let hash1 = compute_reverie_tts_config_hash(&config, &reverie_config1);
        let hash2 = compute_reverie_tts_config_hash(&config2, &reverie_config2);

        assert_ne!(hash1, hash2);
    }

    // =========================================================================
    // ReverieTts Provider Tests
    // =========================================================================

    #[test]
    fn test_reverie_tts_creation() {
        let config = create_test_config();
        let tts = ReverieTts::create(config).unwrap();

        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_reverie_tts_creation_missing_api_key() {
        let mut config = create_test_config();
        config.api_key = String::new();

        let result = ReverieTts::create(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_reverie_tts_set_speaker() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_speaker(ReverieSpeaker::english_male());

        assert_eq!(tts.reverie_config().speaker_code(), "en_male");
    }

    #[test]
    fn test_reverie_tts_set_speaker_code() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_speaker_code("ta_female").unwrap();

        assert_eq!(tts.reverie_config().speaker_code(), "ta_female");
    }

    #[test]
    fn test_reverie_tts_set_speed() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_speed(1.3);

        assert!((tts.reverie_config().speed - 1.3).abs() < 0.001);
    }

    #[test]
    fn test_reverie_tts_set_speed_clamped() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_speed(2.0);
        assert!((tts.reverie_config().speed - MAX_SPEED).abs() < 0.001);

        tts.set_speed(0.1);
        assert!((tts.reverie_config().speed - MIN_SPEED).abs() < 0.001);
    }

    #[test]
    fn test_reverie_tts_set_pitch() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_pitch(1.5);

        assert!((tts.reverie_config().pitch - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_reverie_tts_set_pitch_clamped() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_pitch(5.0);
        assert!((tts.reverie_config().pitch - MAX_PITCH).abs() < 0.001);

        tts.set_pitch(-5.0);
        assert!((tts.reverie_config().pitch - MIN_PITCH).abs() < 0.001);
    }

    #[test]
    fn test_reverie_tts_set_sample_rate() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_sample_rate(16000);

        assert_eq!(tts.reverie_config().sample_rate, 16000);
    }

    #[test]
    fn test_reverie_tts_set_format() {
        let config = create_test_config();
        let mut tts = ReverieTts::create(config).unwrap();

        tts.set_format(ReverieTtsAudioFormat::Mp3);

        assert_eq!(tts.reverie_config().format, ReverieTtsAudioFormat::Mp3);
    }

    #[test]
    fn test_reverie_tts_get_provider_info() {
        let config = create_test_config();
        let tts = ReverieTts::create(config).unwrap();

        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "reverie");
        assert_eq!(info["version"], "1.0.0");
        assert_eq!(info["api_type"], "HTTP REST");
        assert_eq!(info["speaker"], "hi_female");
        assert!(info["supported_formats"].is_array());
        assert!(info["supported_languages"].is_array());
        assert_eq!(info["features"]["ssml_support"], true);
        assert_eq!(info["features"]["indian_language_optimization"], true);
    }

    // =========================================================================
    // Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_text_success() {
        let result = ReverieTts::validate_text("नमस्ते, दुनिया!");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_text_too_long() {
        let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
        let result = ReverieTts::validate_text(&long_text);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TTSError::InvalidConfiguration(_)));
    }

    #[test]
    fn test_validate_text_at_limit() {
        let text = "a".repeat(MAX_TEXT_LENGTH);
        let result = ReverieTts::validate_text(&text);
        assert!(result.is_ok());
    }

    // =========================================================================
    // Clone Tests
    // =========================================================================

    #[test]
    fn test_request_builder_clone() {
        let config = create_test_config();
        let reverie_config = ReverieTtsConfig::from_base(config.clone()).unwrap();
        let builder = ReverieRequestBuilder::new(config, reverie_config);

        let cloned = builder.clone();

        assert_eq!(cloned.config.api_key, builder.config.api_key);
        assert_eq!(
            cloned.reverie_config.speaker_code(),
            builder.reverie_config.speaker_code()
        );
    }
}
