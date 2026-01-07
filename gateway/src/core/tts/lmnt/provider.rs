//! LMNT TTS request builder and provider implementation.
//!
//! This module implements the `TTSRequestBuilder` trait for LMNT TTS, which builds
//! HTTP POST requests with proper headers, authentication, and JSON body for the
//! LMNT Text-to-Speech REST API.
//!
//! # Architecture
//!
//! The `LmntRequestBuilder` constructs HTTP requests for the LMNT TTS API:
//! - URL: `https://api.lmnt.com/v1/ai/speech/bytes`
//! - Authentication: `X-API-Key: {api_key}` header
//! - Content-Type: `application/json`
//!
//! The `LmntTts` struct is the main TTS provider that uses the generic `TTSProvider`
//! infrastructure with `LmntRequestBuilder` for LMNT-specific request construction.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::lmnt::LmntTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-lmnt-api-key".to_string(),
//!     voice_id: Some("lily".to_string()),
//!     audio_format: Some("linear16".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = LmntTts::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, info};
use xxhash_rust::xxh3::xxh3_128;

use super::config::LmntTtsConfig;
use super::{
    DEFAULT_LANGUAGE, DEFAULT_MODEL, DEFAULT_SPEED, DEFAULT_TEMPERATURE, DEFAULT_TOP_P,
    LMNT_TTS_URL, MAX_TEXT_LENGTH,
};
use crate::core::tts::base::{
    AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

// =============================================================================
// LmntRequestBuilder
// =============================================================================

/// LMNT-specific request builder for constructing TTS API requests.
///
/// Implements `TTSRequestBuilder` to construct HTTP POST requests for the
/// LMNT Text-to-Speech REST API with proper headers and JSON body.
///
/// # Key Features
///
/// - **Low Latency**: ~150ms typical latency for audio generation
/// - **Voice Parameters**: Control expressiveness with `top_p` and `temperature`
/// - **22+ Languages**: Support for auto-detection or explicit language codes
/// - **Multiple Formats**: PCM, MP3, µ-law, WebM, and more
#[derive(Clone)]
pub struct LmntRequestBuilder {
    /// Base TTS configuration (contains api_key, voice_id, etc.)
    config: TTSConfig,

    /// LMNT-specific configuration (top_p, temperature, language, etc.)
    lmnt_config: LmntTtsConfig,

    /// Pre-compiled pronunciation replacement patterns.
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl LmntRequestBuilder {
    /// Creates a new LMNT request builder.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration
    /// * `lmnt_config` - LMNT-specific configuration
    pub fn new(config: TTSConfig, lmnt_config: LmntTtsConfig) -> Self {
        // Pre-compile pronunciation replacer if pronunciations are configured
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        Self {
            config,
            lmnt_config,
            pronunciation_replacer,
        }
    }

    /// Builds the request body as JSON.
    fn build_request_body(&self, text: &str) -> serde_json::Value {
        let mut body = json!({
            "voice": self.lmnt_config.voice_id(),
            "text": text,
        });

        // Add model if non-default
        if self.lmnt_config.model != DEFAULT_MODEL {
            body["model"] = json!(&self.lmnt_config.model);
        }

        // Add language if non-default
        if self.lmnt_config.language != DEFAULT_LANGUAGE {
            body["language"] = json!(&self.lmnt_config.language);
        }

        // Always include format and sample_rate for explicit control
        body["format"] = json!(self.lmnt_config.output_format.as_str());
        body["sample_rate"] = json!(self.lmnt_config.sample_rate);

        // Add top_p if non-default
        if (self.lmnt_config.top_p - DEFAULT_TOP_P).abs() > 0.001 {
            body["top_p"] = json!(self.lmnt_config.top_p);
        }

        // Add temperature if non-default
        if (self.lmnt_config.temperature - DEFAULT_TEMPERATURE).abs() > 0.001 {
            body["temperature"] = json!(self.lmnt_config.temperature);
        }

        // Add speed if non-default
        if (self.lmnt_config.speed - DEFAULT_SPEED).abs() > 0.001 {
            body["speed"] = json!(self.lmnt_config.speed);
        }

        // Add seed if provided
        if let Some(seed) = self.lmnt_config.seed {
            body["seed"] = json!(seed);
        }

        // Add debug if enabled
        if self.lmnt_config.debug {
            body["debug"] = json!(true);
        }

        body
    }
}

impl TTSRequestBuilder for LmntRequestBuilder {
    /// Build the LMNT TTS HTTP request with URL, headers, and JSON body.
    ///
    /// # Request Format
    ///
    /// **URL**: `https://api.lmnt.com/v1/ai/speech/bytes`
    /// **Method**: POST
    ///
    /// **Headers**:
    /// | Header | Value | Purpose |
    /// |--------|-------|---------|
    /// | X-API-Key | {api_key} | Authentication |
    /// | Content-Type | application/json | Request body format |
    /// | Accept | {based on format} | Response format |
    ///
    /// **Body**:
    /// ```json
    /// {
    ///   "voice": "lily",
    ///   "text": "Hello, world!",
    ///   "format": "pcm_s16le",
    ///   "sample_rate": 24000,
    ///   "top_p": 0.8,
    ///   "temperature": 1.0
    /// }
    /// ```
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        let body = self.build_request_body(text);

        debug!(
            "Building LMNT TTS request: voice={}, language={}, format={:?}, top_p={}, temperature={}",
            self.lmnt_config.voice_id(),
            self.lmnt_config.language,
            self.lmnt_config.output_format,
            self.lmnt_config.top_p,
            self.lmnt_config.temperature
        );

        // Build the HTTP request with all required headers
        client
            .post(LMNT_TTS_URL)
            .header("X-API-Key", &self.config.api_key)
            .header("Content-Type", "application/json")
            .header("Accept", self.lmnt_config.output_format.content_type())
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
///
/// The hash includes all fields that affect audio output:
/// - Provider name ("lmnt")
/// - Voice ID
/// - Model
/// - Language
/// - Output format
/// - Sample rate
/// - top_p
/// - temperature
/// - speed
/// - seed (if provided)
fn compute_lmnt_tts_config_hash(config: &TTSConfig, lmnt_config: &LmntTtsConfig) -> String {
    let mut s = String::with_capacity(256);

    // Provider identifier
    s.push_str("lmnt|");

    // Voice
    s.push_str(lmnt_config.voice_id());
    s.push('|');

    // Model
    s.push_str(&lmnt_config.model);
    s.push('|');

    // Language
    s.push_str(&lmnt_config.language);
    s.push('|');

    // Format
    s.push_str(lmnt_config.output_format.as_str());
    s.push('|');
    s.push_str(&lmnt_config.sample_rate.to_string());
    s.push('|');

    // Voice parameters
    s.push_str(&format!("{:.3}", lmnt_config.top_p));
    s.push('|');
    s.push_str(&format!("{:.3}", lmnt_config.temperature));
    s.push('|');
    s.push_str(&format!("{:.3}", lmnt_config.speed));
    s.push('|');

    // Seed (affects output)
    if let Some(seed) = lmnt_config.seed {
        s.push_str(&seed.to_string());
    }
    s.push('|');

    // Speaking rate from base config (if different from speed)
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }

    // Compute xxHash3-128 and format as hex
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

// =============================================================================
// LmntTts Provider
// =============================================================================

/// LMNT Text-to-Speech provider implementation.
///
/// Uses the LMNT TTS REST API with ultra-low latency (~150ms) speech synthesis.
///
/// # Key Features
///
/// - **Ultra-Low Latency**: ~150ms typical latency for audio generation
/// - **Voice Parameters**: Control speech with `top_p` and `temperature`
/// - **22+ Languages**: Support for auto-detection or explicit codes
/// - **Voice Cloning**: Create custom voices from 5+ second audio samples
///
/// # Architecture
///
/// This provider delegates connection pooling and audio streaming to the
/// generic `TTSProvider` infrastructure, which handles:
/// - HTTP connection pooling via `ReqManager`
/// - Audio chunk buffering and streaming
/// - Ordered delivery via dispatcher task
/// - Audio caching with config+text hash keys
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::lmnt::LmntTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("lily".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = LmntTts::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct LmntTts {
    /// Generic HTTP-based TTS provider for connection pooling and streaming.
    provider: TTSProvider,

    /// LMNT-specific request builder.
    request_builder: LmntRequestBuilder,

    /// Precomputed configuration hash for cache keying.
    config_hash: String,
}

impl LmntTts {
    /// Creates a new LMNT TTS provider instance.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration with API key and voice settings
    ///
    /// # Returns
    /// * `Ok(Self)` - A new provider instance ready for connection
    /// * `Err(TTSError::InvalidConfiguration)` - If configuration is invalid
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        // Create LMNT-specific config
        let lmnt_config = LmntTtsConfig::from_base(config.clone());

        // Validate configuration
        if let Err(e) = lmnt_config.validate() {
            return Err(TTSError::InvalidConfiguration(e));
        }

        // Create request builder
        let request_builder = LmntRequestBuilder::new(config.clone(), lmnt_config.clone());

        // Compute config hash for caching
        let config_hash = compute_lmnt_tts_config_hash(&config, &lmnt_config);

        info!(
            "Created LmntTts provider: voice={}, language={}, format={:?}",
            lmnt_config.voice_id(),
            lmnt_config.language,
            lmnt_config.output_format
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder,
            config_hash,
        })
    }

    /// Creates a new LMNT TTS provider with custom configuration.
    ///
    /// This method allows full control over LMNT-specific settings including
    /// voice parameters and language.
    ///
    /// # Arguments
    /// * `lmnt_config` - Complete LMNT TTS configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::lmnt::{LmntTts, LmntTtsConfig};
    /// use waav_gateway::core::tts::TTSConfig;
    ///
    /// let base = TTSConfig {
    ///     api_key: "your-api-key".to_string(),
    ///     voice_id: Some("lily".to_string()),
    ///     ..Default::default()
    /// };
    ///
    /// let config = LmntTtsConfig::from_base(base)
    ///     .with_language("en")
    ///     .with_top_p(0.9)
    ///     .with_temperature(1.2);
    ///
    /// let tts = LmntTts::with_config(config)?;
    /// ```
    pub fn with_config(lmnt_config: LmntTtsConfig) -> TTSResult<Self> {
        // Validate configuration
        if let Err(e) = lmnt_config.validate() {
            return Err(TTSError::InvalidConfiguration(e));
        }

        let config = lmnt_config.base.clone();
        let request_builder = LmntRequestBuilder::new(config.clone(), lmnt_config.clone());
        let config_hash = compute_lmnt_tts_config_hash(&config, &lmnt_config);

        info!(
            "Created LmntTts provider with config: voice={}, top_p={}, temperature={}",
            lmnt_config.voice_id(),
            lmnt_config.top_p,
            lmnt_config.temperature
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder,
            config_hash,
        })
    }

    /// Sets the request manager for connection pooling.
    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }

    /// Returns a reference to the LMNT-specific configuration.
    #[inline]
    pub fn lmnt_config(&self) -> &LmntTtsConfig {
        &self.request_builder.lmnt_config
    }

    /// Sets the voice ID.
    pub fn set_voice(&mut self, voice_id: impl Into<String>) {
        self.request_builder.lmnt_config.base.voice_id = Some(voice_id.into());
        self.recompute_config_hash();
    }

    /// Sets the language code.
    pub fn set_language(&mut self, language: impl Into<String>) {
        self.request_builder.lmnt_config.language = language.into();
        self.recompute_config_hash();
    }

    /// Sets the top_p value (speech stability).
    ///
    /// # Arguments
    /// * `top_p` - Value between 0 and 1 (clamped if out of range)
    pub fn set_top_p(&mut self, top_p: f32) {
        self.request_builder.lmnt_config.top_p = top_p.clamp(super::MIN_TOP_P, super::MAX_TOP_P);
        self.recompute_config_hash();
    }

    /// Sets the temperature value (expressiveness).
    ///
    /// # Arguments
    /// * `temperature` - Value >= 0 (clamped if negative)
    pub fn set_temperature(&mut self, temperature: f32) {
        self.request_builder.lmnt_config.temperature = temperature.max(super::MIN_TEMPERATURE);
        self.recompute_config_hash();
    }

    /// Sets the speed multiplier.
    ///
    /// # Arguments
    /// * `speed` - Value between 0.25 and 2.0 (clamped if out of range)
    pub fn set_speed(&mut self, speed: f32) {
        self.request_builder.lmnt_config.speed = speed.clamp(super::MIN_SPEED, super::MAX_SPEED);
        self.recompute_config_hash();
    }

    /// Sets the random seed for deterministic output.
    pub fn set_seed(&mut self, seed: i64) {
        self.request_builder.lmnt_config.seed = Some(seed);
        self.recompute_config_hash();
    }

    /// Clears the random seed.
    pub fn clear_seed(&mut self) {
        self.request_builder.lmnt_config.seed = None;
        self.recompute_config_hash();
    }

    /// Enables or disables debug mode.
    pub fn set_debug(&mut self, debug: bool) {
        self.request_builder.lmnt_config.debug = debug;
        // Debug doesn't affect audio output, so no hash recompute needed
    }

    /// Recomputes the config hash after parameter changes.
    fn recompute_config_hash(&mut self) {
        self.config_hash = compute_lmnt_tts_config_hash(
            &self.request_builder.config,
            &self.request_builder.lmnt_config,
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

impl Default for LmntTts {
    fn default() -> Self {
        Self::new(TTSConfig::default()).expect("Default LmntTts should have valid configuration")
    }
}

#[async_trait]
impl BaseTTS for LmntTts {
    /// Create a new instance of the TTS provider.
    fn new(config: TTSConfig) -> TTSResult<Self> {
        LmntTts::new(config)
    }

    /// Get the underlying TTSProvider for HTTP-based providers.
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the TTS provider.
    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(LMNT_TTS_URL, &self.request_builder.config)
            .await?;

        info!("LMNT TTS provider connected and ready");
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
            info!("LMNT TTS not ready, attempting to connect...");
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
            "provider": "lmnt",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "endpoint": LMNT_TTS_URL,
            "voice": self.request_builder.lmnt_config.voice_id(),
            "model": self.request_builder.lmnt_config.model,
            "language": self.request_builder.lmnt_config.language,
            "top_p": self.request_builder.lmnt_config.top_p,
            "temperature": self.request_builder.lmnt_config.temperature,
            "speed": self.request_builder.lmnt_config.speed,
            "supported_formats": ["mp3", "pcm_s16le", "pcm_f32le", "ulaw", "webm", "aac", "wav"],
            "supported_sample_rates": [8000, 16000, 24000],
            "features": {
                "voice_cloning": true,
                "min_clone_audio_seconds": 5,
                "max_text_length": MAX_TEXT_LENGTH,
                "emotion_control": false,
                "top_p_range": [0.0, 1.0],
                "temperature_range": [0.0, "∞"],
                "speed_range": [0.25, 2.0],
            },
            "supported_languages": [
                "auto", "ar", "de", "en", "es", "fr", "hi", "id", "it", "ja",
                "ko", "nl", "pl", "pt", "ru", "sv", "th", "tr", "uk", "ur", "vi", "zh"
            ],
            "documentation": "https://docs.lmnt.com"
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::base::Pronunciation;
    use crate::core::tts::lmnt::LmntAudioFormat;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "lmnt".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("lily".to_string()),
            model: String::new(),
            speaking_rate: None,
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        }
    }

    // =========================================================================
    // LmntRequestBuilder Tests
    // =========================================================================

    #[test]
    fn test_lmnt_request_builder_new() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());

        let builder = LmntRequestBuilder::new(config, lmnt_config);

        assert!(builder.pronunciation_replacer.is_none());
        assert_eq!(builder.lmnt_config.output_format, LmntAudioFormat::PcmS16le);
    }

    #[test]
    fn test_lmnt_request_builder_with_pronunciations() {
        let mut config = create_test_config();
        config.pronunciations = vec![Pronunciation {
            word: "API".to_string(),
            pronunciation: "A P I".to_string(),
        }];

        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        assert!(builder.pronunciation_replacer.is_some());
    }

    #[test]
    fn test_lmnt_request_builder_get_config() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let retrieved = builder.get_config();
        assert_eq!(retrieved.voice_id, Some("lily".to_string()));
    }

    #[test]
    fn test_lmnt_request_builder_build_request_body_default() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let body = builder.build_request_body("Hello world");

        assert_eq!(body["voice"], "lily");
        assert_eq!(body["text"], "Hello world");
        assert_eq!(body["format"], "pcm_s16le");
        assert_eq!(body["sample_rate"], 24000);
        // Default values should not be in body
        assert!(body.get("model").is_none());
        assert!(body.get("language").is_none());
        assert!(body.get("top_p").is_none());
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn test_lmnt_request_builder_build_request_body_custom() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone())
            .with_model("custom-model")
            .with_language("en")
            .with_top_p(0.9)
            .with_temperature(1.5)
            .with_speed(1.3)
            .with_seed(12345)
            .with_debug(true);
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let body = builder.build_request_body("Hello");

        assert_eq!(body["voice"], "lily");
        assert_eq!(body["text"], "Hello");
        assert_eq!(body["model"], "custom-model");
        assert_eq!(body["language"], "en");
        // Use approximate comparison for f32 values serialized to JSON
        let top_p = body["top_p"].as_f64().unwrap();
        assert!((top_p - 0.9).abs() < 0.001, "top_p: {}", top_p);
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 1.5).abs() < 0.001, "temperature: {}", temp);
        let speed = body["speed"].as_f64().unwrap();
        assert!((speed - 1.3).abs() < 0.001, "speed: {}", speed);
        assert_eq!(body["seed"], 12345);
        assert_eq!(body["debug"], true);
    }

    // =========================================================================
    // HTTP Request Building Tests
    // =========================================================================

    #[test]
    fn test_build_http_request_url() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        assert_eq!(request.url().as_str(), LMNT_TTS_URL);
        assert_eq!(request.method(), reqwest::Method::POST);
    }

    #[test]
    fn test_build_http_request_headers() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        // Verify X-API-Key header
        let api_key_header = request.headers().get("x-api-key").unwrap();
        assert_eq!(api_key_header.to_str().unwrap(), "test-api-key");

        // Verify Content-Type header
        let content_type = request.headers().get("content-type").unwrap();
        assert_eq!(content_type.to_str().unwrap(), "application/json");

        // Verify Accept header (for PCM)
        let accept_header = request.headers().get("accept").unwrap();
        assert_eq!(accept_header.to_str().unwrap(), "audio/pcm");
    }

    #[test]
    fn test_build_http_request_headers_mp3() {
        let mut config = create_test_config();
        config.audio_format = Some("mp3".to_string());
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Test").build().unwrap();

        let accept = request.headers().get("accept").unwrap();
        assert_eq!(accept.to_str().unwrap(), "audio/mpeg");
    }

    #[test]
    fn test_build_http_request_body() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone()).with_language("en");
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let client = reqwest::Client::new();
        let request = builder
            .build_http_request(&client, "Hello world")
            .build()
            .unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["voice"], "lily");
        assert_eq!(body_json["text"], "Hello world");
        assert_eq!(body_json["language"], "en");
        assert_eq!(body_json["format"], "pcm_s16le");
        assert_eq!(body_json["sample_rate"], 24000);
    }

    // =========================================================================
    // Config Hash Tests
    // =========================================================================

    #[test]
    fn test_compute_lmnt_tts_config_hash() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());

        let hash = compute_lmnt_tts_config_hash(&config, &lmnt_config);

        // Hash should be 32-char hex
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Same config produces same hash
        let hash2 = compute_lmnt_tts_config_hash(&config, &lmnt_config);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_config_hash_different_voice() {
        let config = create_test_config();
        let lmnt_config1 = LmntTtsConfig::from_base(config.clone());

        let mut config2 = config.clone();
        config2.voice_id = Some("different".to_string());
        let lmnt_config2 = LmntTtsConfig::from_base(config2.clone());

        let hash1 = compute_lmnt_tts_config_hash(&config, &lmnt_config1);
        let hash2 = compute_lmnt_tts_config_hash(&config2, &lmnt_config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_config_hash_different_top_p() {
        let config = create_test_config();
        let lmnt_config1 = LmntTtsConfig::from_base(config.clone());
        let lmnt_config2 = LmntTtsConfig::from_base(config.clone()).with_top_p(0.5);

        let hash1 = compute_lmnt_tts_config_hash(&config, &lmnt_config1);
        let hash2 = compute_lmnt_tts_config_hash(&config, &lmnt_config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_config_hash_different_temperature() {
        let config = create_test_config();
        let lmnt_config1 = LmntTtsConfig::from_base(config.clone());
        let lmnt_config2 = LmntTtsConfig::from_base(config.clone()).with_temperature(1.5);

        let hash1 = compute_lmnt_tts_config_hash(&config, &lmnt_config1);
        let hash2 = compute_lmnt_tts_config_hash(&config, &lmnt_config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_config_hash_different_seed() {
        let config = create_test_config();
        let lmnt_config1 = LmntTtsConfig::from_base(config.clone());
        let lmnt_config2 = LmntTtsConfig::from_base(config.clone()).with_seed(12345);

        let hash1 = compute_lmnt_tts_config_hash(&config, &lmnt_config1);
        let hash2 = compute_lmnt_tts_config_hash(&config, &lmnt_config2);

        assert_ne!(hash1, hash2);
    }

    // =========================================================================
    // LmntTts Provider Tests
    // =========================================================================

    #[test]
    fn test_lmnt_tts_creation() {
        let config = create_test_config();
        let tts = LmntTts::new(config).unwrap();

        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_lmnt_tts_default() {
        let tts = LmntTts::default();
        assert!(!tts.is_ready());
        // Default TTSConfig has voice_id "aura-asteria-en" which is used
        // The voice_id will be whatever the base config provides
        assert!(!tts.lmnt_config().voice_id().is_empty());
    }

    #[test]
    fn test_lmnt_tts_with_config() {
        let base = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(base)
            .with_language("en")
            .with_top_p(0.9);

        let tts = LmntTts::with_config(lmnt_config).unwrap();

        assert_eq!(tts.lmnt_config().language, "en");
        assert!((tts.lmnt_config().top_p - 0.9).abs() < 0.001);
    }

    #[test]
    fn test_lmnt_tts_set_voice() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_voice("different_voice");

        assert_eq!(tts.lmnt_config().voice_id(), "different_voice");
    }

    #[test]
    fn test_lmnt_tts_set_language() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_language("es");

        assert_eq!(tts.lmnt_config().language, "es");
    }

    #[test]
    fn test_lmnt_tts_set_top_p() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_top_p(0.5);

        assert!((tts.lmnt_config().top_p - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_lmnt_tts_set_top_p_clamped() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_top_p(1.5);
        assert!((tts.lmnt_config().top_p - super::super::MAX_TOP_P).abs() < 0.001);

        tts.set_top_p(-0.5);
        assert!((tts.lmnt_config().top_p - super::super::MIN_TOP_P).abs() < 0.001);
    }

    #[test]
    fn test_lmnt_tts_set_temperature() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_temperature(1.5);

        assert!((tts.lmnt_config().temperature - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_lmnt_tts_set_temperature_clamped() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_temperature(-0.5);

        assert!((tts.lmnt_config().temperature - super::super::MIN_TEMPERATURE).abs() < 0.001);
    }

    #[test]
    fn test_lmnt_tts_set_speed() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_speed(1.5);

        assert!((tts.lmnt_config().speed - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_lmnt_tts_set_speed_clamped() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_speed(3.0);
        assert!((tts.lmnt_config().speed - super::super::MAX_SPEED).abs() < 0.001);

        tts.set_speed(0.1);
        assert!((tts.lmnt_config().speed - super::super::MIN_SPEED).abs() < 0.001);
    }

    #[test]
    fn test_lmnt_tts_set_seed() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_seed(12345);

        assert_eq!(tts.lmnt_config().seed, Some(12345));
    }

    #[test]
    fn test_lmnt_tts_clear_seed() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_seed(12345);
        tts.clear_seed();

        assert!(tts.lmnt_config().seed.is_none());
    }

    #[test]
    fn test_lmnt_tts_set_debug() {
        let config = create_test_config();
        let mut tts = LmntTts::new(config).unwrap();

        tts.set_debug(true);

        assert!(tts.lmnt_config().debug);
    }

    #[test]
    fn test_lmnt_tts_get_provider_info() {
        let config = create_test_config();
        let tts = LmntTts::new(config).unwrap();

        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "lmnt");
        assert_eq!(info["version"], "1.0.0");
        assert_eq!(info["api_type"], "HTTP REST");
        assert_eq!(info["connection_pooling"], true);
        assert_eq!(info["endpoint"], LMNT_TTS_URL);
        assert_eq!(info["voice"], "lily");
        assert_eq!(info["features"]["voice_cloning"], true);
        assert_eq!(info["features"]["emotion_control"], false);
        assert_eq!(info["features"]["max_text_length"], MAX_TEXT_LENGTH);
        assert!(info["supported_formats"].is_array());
        assert!(info["supported_languages"].is_array());
    }

    // =========================================================================
    // Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_text_success() {
        let result = LmntTts::validate_text("Hello, world!");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_text_too_long() {
        let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
        let result = LmntTts::validate_text(&long_text);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TTSError::InvalidConfiguration(_)));
    }

    #[test]
    fn test_validate_text_at_limit() {
        let text = "a".repeat(MAX_TEXT_LENGTH);
        let result = LmntTts::validate_text(&text);
        assert!(result.is_ok());
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_build_http_request_empty_text() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "").build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["text"], "");
    }

    #[test]
    fn test_build_http_request_unicode() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let client = reqwest::Client::new();
        let text = "Hello, 世界! 🌍";
        let request = builder.build_http_request(&client, text).build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["text"], text);
    }

    #[test]
    fn test_build_http_request_special_characters() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let client = reqwest::Client::new();
        let text = r#"Hello "world"! It's a <test> with & symbols."#;
        let request = builder.build_http_request(&client, text).build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["text"], text);
    }

    // =========================================================================
    // Clone Tests
    // =========================================================================

    #[test]
    fn test_request_builder_clone() {
        let config = create_test_config();
        let lmnt_config = LmntTtsConfig::from_base(config.clone());
        let builder = LmntRequestBuilder::new(config, lmnt_config);

        let cloned = builder.clone();

        assert_eq!(cloned.config.api_key, builder.config.api_key);
        assert_eq!(
            cloned.lmnt_config.voice_id(),
            builder.lmnt_config.voice_id()
        );
    }
}
