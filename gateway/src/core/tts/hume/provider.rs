//! Hume AI Octave TTS request builder and provider implementation.
//!
//! This module implements the `TTSRequestBuilder` trait for Hume TTS, which builds
//! HTTP POST requests with proper headers, authentication, and JSON body for the
//! Hume AI Text-to-Speech REST API.
//!
//! # Architecture
//!
//! The `HumeRequestBuilder` constructs HTTP requests for the Hume TTS API:
//! - URL: `https://api.hume.ai/v0/tts/stream/file`
//! - Authentication: `X-Hume-Api-Key: {api_key}` header
//! - Content-Type: `application/json`
//!
//! The `HumeTTS` struct is the main TTS provider that uses the generic `TTSProvider`
//! infrastructure with `HumeRequestBuilder` for Hume-specific request construction.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::hume::HumeTTS;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-hume-api-key".to_string(),
//!     voice_id: Some("Kora".to_string()),
//!     audio_format: Some("linear16".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = HumeTTS::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use tracing::{debug, info};
use xxhash_rust::xxh3::xxh3_128;

use super::config::{HUME_TTS_STREAM_URL, HumeTTSConfig};
use super::messages::{HumeRequestFormat, HumeTTSRequest, HumeUtterance, HumeVoiceSpec};
use crate::core::tts::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

// =============================================================================
// HumeRequestBuilder
// =============================================================================

/// Hume AI-specific request builder for constructing TTS API requests.
///
/// Implements `TTSRequestBuilder` to construct HTTP POST requests for the
/// Hume AI Text-to-Speech REST API with proper headers and JSON body.
///
/// # Key Features
///
/// - Natural language emotion instructions via `description` field
/// - Speed control (0.5 to 2.0)
/// - Instant mode for low-latency streaming
/// - Context continuity via generation_id
#[derive(Clone)]
pub struct HumeRequestBuilder {
    /// Base TTS configuration (contains api_key, voice_id, etc.)
    config: TTSConfig,

    /// Hume-specific configuration (description, speed, instant_mode)
    hume_config: HumeTTSConfig,

    /// Pre-compiled pronunciation replacement patterns.
    pronunciation_replacer: Option<PronunciationReplacer>,

    /// Previous text for context continuity.
    previous_text: Option<String>,
}

impl HumeRequestBuilder {
    /// Creates a new Hume request builder.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration
    /// * `hume_config` - Hume-specific configuration
    pub fn new(config: TTSConfig, hume_config: HumeTTSConfig) -> Self {
        // Pre-compile pronunciation replacer if pronunciations are configured
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        Self {
            config,
            hume_config,
            pronunciation_replacer,
            previous_text: None,
        }
    }

    /// Sets previous text for context continuity.
    pub fn with_previous_text(&mut self, text: Option<String>) {
        self.previous_text = text;
    }

    /// Builds the voice specification for the request.
    fn build_voice_spec(&self) -> HumeVoiceSpec {
        HumeVoiceSpec::by_name(self.hume_config.voice_name())
    }

    /// Builds the output format for the request.
    fn build_format(&self) -> HumeRequestFormat {
        HumeRequestFormat::new(
            self.hume_config.output_format.format.as_str(),
            self.hume_config.output_format.sample_rate,
        )
    }

    /// Builds the request body as JSON.
    fn build_request_body(&self, text: &str) -> HumeTTSRequest {
        // Create utterance with all configured settings
        let mut utterance = HumeUtterance::new(text).with_voice(self.build_voice_spec());

        // Add description (acting instructions) if configured
        if let Some(desc) = &self.hume_config.description {
            utterance = utterance.with_description(desc);
        }

        // Add speed if configured
        if let Some(speed) = self.hume_config.speed {
            utterance = utterance.with_speed(speed);
        }

        // Add trailing silence if configured
        if let Some(silence) = self.hume_config.trailing_silence {
            utterance = utterance.with_trailing_silence(silence);
        }

        // Build request
        let mut request = HumeTTSRequest::with_utterances(vec![utterance])
            .with_format(self.build_format())
            .with_instant_mode(self.hume_config.instant_mode);

        // Add generation ID if configured
        if let Some(gen_id) = &self.hume_config.generation_id {
            request = request.with_generation_id(gen_id);
        }

        // Add num_generations if configured
        if let Some(num) = self.hume_config.num_generations {
            request = request.with_num_generations(num);
        }

        request
    }
}

impl TTSRequestBuilder for HumeRequestBuilder {
    /// Build the Hume TTS HTTP request with URL, headers, and JSON body.
    ///
    /// # Request Format
    ///
    /// **URL**: `https://api.hume.ai/v0/tts/stream/file`
    /// **Method**: POST
    ///
    /// **Headers**:
    /// | Header | Value | Purpose |
    /// |--------|-------|---------|
    /// | X-Hume-Api-Key | {api_key} | Authentication |
    /// | Content-Type | application/json | Request body format |
    /// | Accept | {based on format} | Response format |
    ///
    /// **Body**:
    /// ```json
    /// {
    ///   "utterances": [{
    ///     "text": "Hello, world!",
    ///     "voice": { "name": "Kora" },
    ///     "description": "happy, energetic",
    ///     "speed": 1.0
    ///   }],
    ///   "format": { "type": "pcm16", "sample_rate": 24000 },
    ///   "instant_mode": true
    /// }
    /// ```
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        let body = self.build_request_body(text);

        debug!(
            "Building Hume TTS request: voice={}, description={:?}, speed={:?}, format={:?}",
            self.hume_config.voice_name(),
            self.hume_config.description,
            self.hume_config.speed,
            self.hume_config.output_format.format
        );

        // Build the HTTP request with all required headers
        client
            .post(HUME_TTS_STREAM_URL)
            .header("X-Hume-Api-Key", &self.config.api_key)
            .header("Content-Type", "application/json")
            .header(
                "Accept",
                self.hume_config.output_format.format.content_type(),
            )
            .json(&body)
    }

    /// Build HTTP request with context from previous text.
    fn build_http_request_with_context(
        &self,
        client: &reqwest::Client,
        text: &str,
        previous_text: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let mut body = self.build_request_body(text);

        // Add context for continuity if previous text provided
        if let Some(prev) = previous_text {
            body = body.with_context(super::messages::HumeContext::with_previous_text(prev));
        }

        debug!(
            "Building Hume TTS request with context: text_len={}, prev_text_len={:?}",
            text.len(),
            previous_text.map(|t| t.len())
        );

        client
            .post(HUME_TTS_STREAM_URL)
            .header("X-Hume-Api-Key", &self.config.api_key)
            .header("Content-Type", "application/json")
            .header(
                "Accept",
                self.hume_config.output_format.format.content_type(),
            )
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
/// - Provider name ("hume")
/// - Voice name
/// - Description (acting instructions)
/// - Speed
/// - Output format
/// - Instant mode
fn compute_hume_tts_config_hash(config: &TTSConfig, hume_config: &HumeTTSConfig) -> String {
    let mut s = String::with_capacity(256);

    // Provider identifier
    s.push_str("hume|");

    // Voice
    s.push_str(hume_config.voice_name());
    s.push('|');

    // Description (affects audio emotion)
    if let Some(desc) = &hume_config.description {
        s.push_str(desc);
    }
    s.push('|');

    // Speed
    if let Some(speed) = hume_config.speed {
        s.push_str(&format!("{speed:.3}"));
    }
    s.push('|');

    // Format
    s.push_str(hume_config.output_format.format.as_str());
    s.push('|');
    s.push_str(&hume_config.output_format.sample_rate.to_string());
    s.push('|');

    // Instant mode
    s.push_str(if hume_config.instant_mode { "1" } else { "0" });
    s.push('|');

    // Speaking rate from base config
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }

    // Compute xxHash3-128 and format as hex
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

// =============================================================================
// HumeTTS Provider
// =============================================================================

/// Hume AI Octave Text-to-Speech provider implementation.
///
/// Uses the Hume TTS REST API with natural language emotion instructions
/// for high-quality, emotionally expressive speech synthesis.
///
/// # Key Features
///
/// - **Natural Language Emotions**: Control emotion via `description` field
///   - Example: "happy, energetic", "sad, melancholic", "whispered fearfully"
/// - **Speed Control**: 0.5 to 2.0 range
/// - **Instant Mode**: Low-latency streaming
/// - **Context Continuity**: Maintain voice consistency across utterances
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
/// use waav_gateway::core::tts::hume::HumeTTS;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("Kora".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = HumeTTS::new(config)?;
///
/// // Set emotion via the Hume-specific API
/// tts.set_description("warm, friendly, inviting");
///
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct HumeTTS {
    /// Generic HTTP-based TTS provider for connection pooling and streaming.
    provider: TTSProvider,

    /// Hume-specific request builder.
    request_builder: HumeRequestBuilder,

    /// Precomputed configuration hash for cache keying.
    config_hash: String,
}

impl HumeTTS {
    /// Creates a new Hume TTS provider instance.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration with API key and voice settings
    ///
    /// # Returns
    /// * `Ok(Self)` - A new provider instance ready for connection
    /// * `Err(TTSError::InvalidConfiguration)` - If configuration is invalid
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        // Create Hume-specific config
        let hume_config = HumeTTSConfig::from_base(config.clone());

        // Create request builder
        let request_builder = HumeRequestBuilder::new(config.clone(), hume_config.clone());

        // Compute config hash for caching
        let config_hash = compute_hume_tts_config_hash(&config, &hume_config);

        info!(
            "Created HumeTTS provider: voice={}, description={:?}, format={:?}",
            hume_config.voice_name(),
            hume_config.description,
            hume_config.output_format.format
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder,
            config_hash,
        })
    }

    /// Creates a new Hume TTS provider with custom configuration.
    ///
    /// This method allows full control over Hume-specific settings including
    /// emotion instructions, speed, and instant mode.
    ///
    /// # Arguments
    /// * `hume_config` - Complete Hume TTS configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::hume::{HumeTTS, HumeTTSConfig};
    /// use waav_gateway::core::tts::TTSConfig;
    ///
    /// let base = TTSConfig {
    ///     api_key: "your-api-key".to_string(),
    ///     voice_id: Some("Kora".to_string()),
    ///     ..Default::default()
    /// };
    ///
    /// let config = HumeTTSConfig::from_base(base)
    ///     .with_description("excited, energetic")
    ///     .with_speed(1.2);
    ///
    /// let tts = HumeTTS::with_config(config)?;
    /// ```
    pub fn with_config(hume_config: HumeTTSConfig) -> TTSResult<Self> {
        let config = hume_config.base.clone();
        let request_builder = HumeRequestBuilder::new(config.clone(), hume_config.clone());
        let config_hash = compute_hume_tts_config_hash(&config, &hume_config);

        info!(
            "Created HumeTTS provider with config: voice={}, description={:?}",
            hume_config.voice_name(),
            hume_config.description
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

    /// Returns a reference to the Hume-specific configuration.
    #[inline]
    pub fn hume_config(&self) -> &HumeTTSConfig {
        &self.request_builder.hume_config
    }

    /// Sets the acting instructions (emotion/style description).
    ///
    /// This method updates the emotion instructions that will be used for
    /// all subsequent `speak()` calls.
    ///
    /// # Arguments
    /// * `description` - Natural language description (max 100 chars)
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// tts.set_description("happy, energetic");
    /// tts.set_description("sad, melancholic, slow");
    /// tts.set_description("whispered fearfully");
    /// ```
    pub fn set_description(&mut self, description: impl Into<String>) {
        let desc = description.into();
        // Truncate if too long
        self.request_builder.hume_config.description =
            Some(if desc.len() > super::config::MAX_DESCRIPTION_LENGTH {
                desc[..super::config::MAX_DESCRIPTION_LENGTH].to_string()
            } else {
                desc
            });

        // Recompute config hash since description affects cache
        self.config_hash = compute_hume_tts_config_hash(
            &self.request_builder.config,
            &self.request_builder.hume_config,
        );
    }

    /// Clears the acting instructions.
    pub fn clear_description(&mut self) {
        self.request_builder.hume_config.description = None;
        self.config_hash = compute_hume_tts_config_hash(
            &self.request_builder.config,
            &self.request_builder.hume_config,
        );
    }

    /// Sets the speaking speed.
    ///
    /// # Arguments
    /// * `speed` - Speed factor (0.5 to 2.0, 1.0 is normal)
    pub fn set_speed(&mut self, speed: f32) {
        self.request_builder.hume_config.speed =
            Some(speed.clamp(super::config::MIN_SPEED, super::config::MAX_SPEED));
        self.config_hash = compute_hume_tts_config_hash(
            &self.request_builder.config,
            &self.request_builder.hume_config,
        );
    }

    /// Sets instant mode for low-latency streaming.
    pub fn set_instant_mode(&mut self, enabled: bool) {
        self.request_builder.hume_config.instant_mode = enabled;
        self.config_hash = compute_hume_tts_config_hash(
            &self.request_builder.config,
            &self.request_builder.hume_config,
        );
    }

    /// Sets the generation ID for context continuity.
    pub fn set_generation_id(&mut self, id: impl Into<String>) {
        self.request_builder.hume_config.generation_id = Some(id.into());
    }

    /// Clears the generation ID.
    pub fn clear_generation_id(&mut self) {
        self.request_builder.hume_config.generation_id = None;
    }
}

impl Default for HumeTTS {
    fn default() -> Self {
        Self::new(TTSConfig::default()).expect("Default HumeTTS should have valid configuration")
    }
}

#[async_trait]
impl BaseTTS for HumeTTS {
    /// Create a new instance of the TTS provider.
    fn new(config: TTSConfig) -> TTSResult<Self> {
        HumeTTS::new(config)
    }

    /// Get the underlying TTSProvider for HTTP-based providers.
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the TTS provider.
    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(HUME_TTS_STREAM_URL, &self.request_builder.config)
            .await?;

        info!("Hume TTS provider connected and ready");
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
        // Auto-reconnect if needed
        if !self.is_ready() {
            info!("Hume TTS not ready, attempting to connect...");
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
            "provider": "hume",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "endpoint": HUME_TTS_STREAM_URL,
            "voice": self.request_builder.hume_config.voice_name(),
            "description": self.request_builder.hume_config.description,
            "instant_mode": self.request_builder.hume_config.instant_mode,
            "supported_formats": ["pcm16", "mp3", "wav", "mulaw", "alaw"],
            "supported_sample_rates": [8000, 16000, 22050, 24000, 44100, 48000],
            "features": {
                "emotion_control": true,
                "emotion_method": "natural_language",
                "max_description_length": 100,
                "speed_range": [0.5, 2.0],
                "voice_cloning": true,
                "context_continuity": true,
            },
            "supported_languages": [
                "en", "es", "fr", "de", "it", "pt", "nl", "pl", "ru",
                "zh", "ja"
            ],
            "documentation": "https://dev.hume.ai/docs/text-to-speech-tts/overview"
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
    use crate::core::tts::hume::HumeAudioFormat;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "hume".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("Kora".to_string()),
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
    // HumeRequestBuilder Tests
    // =========================================================================

    #[test]
    fn test_hume_request_builder_new() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());

        let builder = HumeRequestBuilder::new(config, hume_config);

        assert!(builder.pronunciation_replacer.is_none());
        assert_eq!(
            builder.hume_config.output_format.format,
            HumeAudioFormat::Pcm16
        );
    }

    #[test]
    fn test_hume_request_builder_with_pronunciations() {
        let mut config = create_test_config();
        config.pronunciations = vec![Pronunciation {
            word: "API".to_string(),
            pronunciation: "A P I".to_string(),
        }];

        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        assert!(builder.pronunciation_replacer.is_some());
    }

    #[test]
    fn test_hume_request_builder_get_config() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let retrieved = builder.get_config();
        assert_eq!(retrieved.voice_id, Some("Kora".to_string()));
    }

    #[test]
    fn test_hume_request_builder_build_voice_spec() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let voice_spec = builder.build_voice_spec();
        match voice_spec {
            HumeVoiceSpec::ByName { name } => assert_eq!(name, "Kora"),
            _ => panic!("Expected ByName variant"),
        }
    }

    #[test]
    fn test_hume_request_builder_build_format() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let format = builder.build_format();
        assert_eq!(format.format_type, "pcm16");
        assert_eq!(format.sample_rate, 24000);
    }

    #[test]
    fn test_hume_request_builder_build_request_body() {
        let config = create_test_config();
        let mut hume_config = HumeTTSConfig::from_base(config.clone());
        hume_config.description = Some("happy, energetic".to_string());
        hume_config.speed = Some(1.2);

        let builder = HumeRequestBuilder::new(config, hume_config);
        let body = builder.build_request_body("Hello world");

        assert_eq!(body.utterances.len(), 1);
        assert_eq!(body.utterances[0].text, "Hello world");
        assert_eq!(
            body.utterances[0].description,
            Some("happy, energetic".to_string())
        );
        assert_eq!(body.utterances[0].speed, Some(1.2));
        assert!(body.format.is_some());
        assert_eq!(body.instant_mode, Some(true));
    }

    // =========================================================================
    // HTTP Request Building Tests
    // =========================================================================

    #[test]
    fn test_build_http_request_url() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        assert_eq!(request.url().as_str(), HUME_TTS_STREAM_URL);
        assert_eq!(request.method(), reqwest::Method::POST);
    }

    #[test]
    fn test_build_http_request_headers() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        // Verify X-Hume-Api-Key header
        let api_key_header = request.headers().get("x-hume-api-key").unwrap();
        assert_eq!(api_key_header.to_str().unwrap(), "test-api-key");

        // Verify Content-Type header
        let content_type = request.headers().get("content-type").unwrap();
        assert_eq!(content_type.to_str().unwrap(), "application/json");

        // Verify Accept header
        let accept_header = request.headers().get("accept").unwrap();
        assert_eq!(accept_header.to_str().unwrap(), "audio/pcm");
    }

    #[test]
    fn test_build_http_request_body() {
        let config = create_test_config();
        let mut hume_config = HumeTTSConfig::from_base(config.clone());
        hume_config.description = Some("excited".to_string());

        let builder = HumeRequestBuilder::new(config, hume_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        // Verify utterances
        assert!(body_json["utterances"].is_array());
        assert_eq!(body_json["utterances"][0]["text"], "Hello world");
        assert_eq!(body_json["utterances"][0]["description"], "excited");

        // Verify format
        assert_eq!(body_json["format"]["type"], "pcm16");
        assert_eq!(body_json["format"]["sample_rate"], 24000);

        // Verify instant_mode
        assert_eq!(body_json["instant_mode"], true);
    }

    #[test]
    fn test_build_http_request_mp3_format() {
        let mut config = create_test_config();
        config.audio_format = Some("mp3".to_string());

        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Test").build().unwrap();

        // Check Accept header for MP3
        let accept = request.headers().get("accept").unwrap();
        assert_eq!(accept.to_str().unwrap(), "audio/mpeg");
    }

    // =========================================================================
    // Config Hash Tests
    // =========================================================================

    #[test]
    fn test_compute_hume_tts_config_hash() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());

        let hash = compute_hume_tts_config_hash(&config, &hume_config);

        // Hash should be 32-char hex
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Same config produces same hash
        let hash2 = compute_hume_tts_config_hash(&config, &hume_config);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_config_hash_different_description() {
        let config = create_test_config();
        let hume_config1 = HumeTTSConfig::from_base(config.clone());

        let mut hume_config2 = HumeTTSConfig::from_base(config.clone());
        hume_config2.description = Some("different".to_string());

        let hash1 = compute_hume_tts_config_hash(&config, &hume_config1);
        let hash2 = compute_hume_tts_config_hash(&config, &hume_config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_config_hash_different_speed() {
        let config = create_test_config();
        let mut hume_config1 = HumeTTSConfig::from_base(config.clone());
        hume_config1.speed = Some(1.0);

        let mut hume_config2 = HumeTTSConfig::from_base(config.clone());
        hume_config2.speed = Some(1.5);

        let hash1 = compute_hume_tts_config_hash(&config, &hume_config1);
        let hash2 = compute_hume_tts_config_hash(&config, &hume_config2);

        assert_ne!(hash1, hash2);
    }

    // =========================================================================
    // HumeTTS Provider Tests
    // =========================================================================

    #[test]
    fn test_hume_tts_creation() {
        let config = create_test_config();
        let tts = HumeTTS::new(config).unwrap();

        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_hume_tts_default() {
        let tts = HumeTTS::default();
        assert!(!tts.is_ready());
        // Default TTSConfig has voice_id "aura-asteria-en" which is parsed as custom voice
        // The voice_name will be whatever the base config provides
        assert!(!tts.hume_config().voice_name().is_empty());
    }

    #[test]
    fn test_hume_tts_with_config() {
        let base = create_test_config();
        let hume_config = HumeTTSConfig::from_base(base)
            .with_description("happy")
            .with_speed(1.5);

        let tts = HumeTTS::with_config(hume_config).unwrap();

        assert_eq!(tts.hume_config().description, Some("happy".to_string()));
        assert_eq!(tts.hume_config().speed, Some(1.5));
    }

    #[test]
    fn test_hume_tts_set_description() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        tts.set_description("sad, melancholic");

        assert_eq!(
            tts.hume_config().description,
            Some("sad, melancholic".to_string())
        );
    }

    #[test]
    fn test_hume_tts_set_description_truncates() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        let long_desc = "a".repeat(150);
        tts.set_description(long_desc);

        assert_eq!(
            tts.hume_config().description.as_ref().unwrap().len(),
            super::super::config::MAX_DESCRIPTION_LENGTH
        );
    }

    #[test]
    fn test_hume_tts_clear_description() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        tts.set_description("happy");
        tts.clear_description();

        assert!(tts.hume_config().description.is_none());
    }

    #[test]
    fn test_hume_tts_set_speed() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        tts.set_speed(1.5);

        assert_eq!(tts.hume_config().speed, Some(1.5));
    }

    #[test]
    fn test_hume_tts_set_speed_clamps() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        tts.set_speed(5.0);
        assert_eq!(tts.hume_config().speed, Some(2.0));

        tts.set_speed(0.1);
        assert_eq!(tts.hume_config().speed, Some(0.5));
    }

    #[test]
    fn test_hume_tts_set_instant_mode() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        tts.set_instant_mode(false);
        assert!(!tts.hume_config().instant_mode);

        tts.set_instant_mode(true);
        assert!(tts.hume_config().instant_mode);
    }

    #[test]
    fn test_hume_tts_set_generation_id() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        tts.set_generation_id("gen-123");
        assert_eq!(tts.hume_config().generation_id, Some("gen-123".to_string()));
    }

    #[test]
    fn test_hume_tts_clear_generation_id() {
        let config = create_test_config();
        let mut tts = HumeTTS::new(config).unwrap();

        tts.set_generation_id("gen-123");
        tts.clear_generation_id();

        assert!(tts.hume_config().generation_id.is_none());
    }

    #[test]
    fn test_hume_tts_get_provider_info() {
        let config = create_test_config();
        let tts = HumeTTS::new(config).unwrap();

        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "hume");
        assert_eq!(info["version"], "1.0.0");
        assert_eq!(info["api_type"], "HTTP REST");
        assert_eq!(info["connection_pooling"], true);
        assert_eq!(info["endpoint"], HUME_TTS_STREAM_URL);
        assert_eq!(info["voice"], "Kora");
        assert_eq!(info["features"]["emotion_control"], true);
        assert_eq!(info["features"]["emotion_method"], "natural_language");
        assert_eq!(info["features"]["max_description_length"], 100);
        assert!(info["supported_formats"].is_array());
    }

    // =========================================================================
    // Clone Tests
    // =========================================================================

    #[test]
    fn test_request_builder_clone() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let cloned = builder.clone();

        assert_eq!(cloned.config.api_key, builder.config.api_key);
        assert_eq!(
            cloned.hume_config.voice_name(),
            builder.hume_config.voice_name()
        );
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_build_http_request_empty_text() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "").build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["utterances"][0]["text"], "");
    }

    #[test]
    fn test_build_http_request_unicode() {
        let config = create_test_config();
        let hume_config = HumeTTSConfig::from_base(config.clone());
        let builder = HumeRequestBuilder::new(config, hume_config);

        let client = reqwest::Client::new();
        let text = "Hello, 世界! 🌍";
        let request = builder.build_http_request(&client, text).build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["utterances"][0]["text"], text);
    }
}
