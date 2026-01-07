//! Play.ht TTS request builder and provider implementation.
//!
//! This module implements the `TTSRequestBuilder` trait for Play.ht TTS, which builds
//! HTTP POST requests with proper headers, authentication, and JSON body for the
//! Play.ht Text-to-Speech REST API.
//!
//! # Architecture
//!
//! The `PlayHtRequestBuilder` constructs HTTP requests for the Play.ht TTS API:
//! - URL: `https://api.play.ht/api/v2/tts/stream`
//! - Authentication: `X-USER-ID` + `AUTHORIZATION` headers
//! - Content-Type: `application/json`
//!
//! The `PlayHtTts` struct is the main TTS provider that uses the generic `TTSProvider`
//! infrastructure with `PlayHtRequestBuilder` for Play.ht-specific request construction.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::playht::PlayHtTts;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-playht-api-key".to_string(),
//!     voice_id: Some("s3://voice-cloning-zero-shot/.../manifest.json".to_string()),
//!     audio_format: Some("mp3".to_string()),
//!     sample_rate: Some(48000),
//!     ..Default::default()
//! };
//!
//! let mut tts = PlayHtTts::with_user_id(config, "your-user-id".to_string())?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tracing::{debug, info, warn};
use xxhash_rust::xxh3::xxh3_128;

use super::config::PlayHtTtsConfig;
use super::messages::PlayHtApiError;
use super::{DEFAULT_SPEED, MAX_TEXT_LENGTH, PLAYHT_TTS_URL};
use crate::core::tts::base::{
    AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

// =============================================================================
// PlayHtRequestBuilder
// =============================================================================

/// Play.ht-specific request builder for constructing TTS API requests.
///
/// Implements `TTSRequestBuilder` to construct HTTP POST requests for the
/// Play.ht Text-to-Speech REST API with proper headers and JSON body.
///
/// # Key Features
///
/// - **Low Latency**: ~190ms typical latency for Play3.0-mini
/// - **PlayDialog**: Multi-turn dialogue support with two-speaker generation
/// - **36+ Languages**: Support for auto-detection or explicit language codes
/// - **Multiple Formats**: MP3, WAV, FLAC, OGG, Raw PCM, mu-law
#[derive(Clone)]
pub struct PlayHtRequestBuilder {
    /// Base TTS configuration (contains api_key, voice_id, etc.)
    config: TTSConfig,

    /// Play.ht-specific configuration (voice_engine, user_id, etc.)
    playht_config: PlayHtTtsConfig,

    /// Pre-compiled pronunciation replacement patterns.
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl PlayHtRequestBuilder {
    /// Creates a new Play.ht request builder.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration
    /// * `playht_config` - Play.ht-specific configuration
    pub fn new(config: TTSConfig, playht_config: PlayHtTtsConfig) -> Self {
        // Pre-compile pronunciation replacer if pronunciations are configured
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        Self {
            config,
            playht_config,
            pronunciation_replacer,
        }
    }

    /// Builds the request body as JSON.
    fn build_request_body(&self, text: &str) -> serde_json::Value {
        let mut body = json!({
            "voice": self.playht_config.voice_id(),
            "text": text,
            "voice_engine": self.playht_config.voice_engine.as_str(),
            "output_format": self.playht_config.output_format.as_str(),
            "sample_rate": self.playht_config.sample_rate,
        });

        // Add speed if non-default
        if (self.playht_config.speed - DEFAULT_SPEED).abs() > 0.001 {
            body["speed"] = json!(self.playht_config.speed);
        }

        // Add quality if specified
        if let Some(quality) = &self.playht_config.quality {
            body["quality"] = json!(quality);
        }

        // Add temperature if specified
        if let Some(temp) = self.playht_config.temperature {
            body["temperature"] = json!(temp);
        }

        // Add seed if specified
        if let Some(seed) = self.playht_config.seed {
            body["seed"] = json!(seed);
        }

        // Add language if specified (Play3.0-mini only)
        if let Some(lang) = &self.playht_config.language {
            body["language"] = json!(lang);
        }

        // Add guidance parameters if specified
        if let Some(tg) = self.playht_config.text_guidance {
            body["text_guidance"] = json!(tg);
        }
        if let Some(vg) = self.playht_config.voice_guidance {
            body["voice_guidance"] = json!(vg);
        }
        if let Some(sg) = self.playht_config.style_guidance {
            body["style_guidance"] = json!(sg);
        }
        if let Some(rp) = self.playht_config.repetition_penalty {
            body["repetition_penalty"] = json!(rp);
        }

        // Add PlayDialog multi-turn parameters
        if let Some(v2) = &self.playht_config.voice_2 {
            body["voice_2"] = json!(v2);
        }
        if let Some(tp) = &self.playht_config.turn_prefix {
            body["turn_prefix"] = json!(tp);
        }
        if let Some(tp2) = &self.playht_config.turn_prefix_2 {
            body["turn_prefix_2"] = json!(tp2);
        }
        if let Some(vcs) = self.playht_config.voice_conditioning_seconds {
            body["voice_conditioning_seconds"] = json!(vcs);
        }
        if let Some(nc) = self.playht_config.num_candidates {
            body["num_candidates"] = json!(nc);
        }

        body
    }
}

impl TTSRequestBuilder for PlayHtRequestBuilder {
    /// Build the Play.ht TTS HTTP request with URL, headers, and JSON body.
    ///
    /// # Request Format
    ///
    /// **URL**: `https://api.play.ht/api/v2/tts/stream`
    /// **Method**: POST
    ///
    /// **Headers**:
    /// | Header | Value | Purpose |
    /// |--------|-------|---------|
    /// | X-USER-ID | {user_id} | User identification |
    /// | AUTHORIZATION | {api_key} | API authentication |
    /// | Content-Type | application/json | Request body format |
    /// | Accept | {based on format} | Response format |
    ///
    /// **Body**:
    /// ```json
    /// {
    ///   "voice": "s3://voice-cloning-zero-shot/.../manifest.json",
    ///   "text": "Hello, world!",
    ///   "voice_engine": "Play3.0-mini",
    ///   "output_format": "mp3",
    ///   "sample_rate": 48000
    /// }
    /// ```
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        let body = self.build_request_body(text);

        debug!(
            "Building Play.ht TTS request: voice={}, voice_engine={}, format={:?}, speed={}",
            self.playht_config.voice_id(),
            self.playht_config.voice_engine,
            self.playht_config.output_format,
            self.playht_config.speed
        );

        // Build the HTTP request with all required headers
        client
            .post(PLAYHT_TTS_URL)
            .header("X-USER-ID", &self.playht_config.user_id)
            .header("AUTHORIZATION", &self.config.api_key)
            .header("Content-Type", "application/json")
            .header("Accept", self.playht_config.output_format.content_type())
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
/// - Provider name ("playht")
/// - Voice ID
/// - Voice engine
/// - Output format
/// - Sample rate
/// - Speed
/// - Quality
/// - Temperature
/// - Seed (if provided)
/// - Language (if provided)
/// - Guidance parameters (if provided)
/// - PlayDialog parameters (if provided)
fn compute_playht_tts_config_hash(config: &TTSConfig, playht_config: &PlayHtTtsConfig) -> String {
    let mut s = String::with_capacity(512);

    // Provider identifier
    s.push_str("playht|");

    // Voice
    s.push_str(playht_config.voice_id());
    s.push('|');

    // Voice engine
    s.push_str(playht_config.voice_engine.as_str());
    s.push('|');

    // Format
    s.push_str(playht_config.output_format.as_str());
    s.push('|');
    s.push_str(&playht_config.sample_rate.to_string());
    s.push('|');

    // Speed
    s.push_str(&format!("{:.3}", playht_config.speed));
    s.push('|');

    // Quality
    if let Some(q) = &playht_config.quality {
        s.push_str(q);
    }
    s.push('|');

    // Temperature
    if let Some(t) = playht_config.temperature {
        s.push_str(&format!("{:.3}", t));
    }
    s.push('|');

    // Seed
    if let Some(seed) = playht_config.seed {
        s.push_str(&seed.to_string());
    }
    s.push('|');

    // Language
    if let Some(lang) = &playht_config.language {
        s.push_str(lang);
    }
    s.push('|');

    // Guidance parameters
    if let Some(tg) = playht_config.text_guidance {
        s.push_str(&format!("{:.3}", tg));
    }
    s.push('|');
    if let Some(vg) = playht_config.voice_guidance {
        s.push_str(&format!("{:.3}", vg));
    }
    s.push('|');
    if let Some(sg) = playht_config.style_guidance {
        s.push_str(&format!("{:.3}", sg));
    }
    s.push('|');
    if let Some(rp) = playht_config.repetition_penalty {
        s.push_str(&format!("{:.3}", rp));
    }
    s.push('|');

    // PlayDialog parameters
    if let Some(v2) = &playht_config.voice_2 {
        s.push_str(v2);
    }
    s.push('|');
    if let Some(tp) = &playht_config.turn_prefix {
        s.push_str(tp);
    }
    s.push('|');
    if let Some(tp2) = &playht_config.turn_prefix_2 {
        s.push_str(tp2);
    }
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
// PlayHtTts Provider
// =============================================================================

/// Play.ht Text-to-Speech provider implementation.
///
/// Uses the Play.ht TTS REST API with low-latency (~190ms) speech synthesis.
///
/// # Key Features
///
/// - **Low Latency**: ~190ms typical latency for Play3.0-mini
/// - **PlayDialog**: Multi-turn dialogue support with two speakers
/// - **36+ Languages**: Support for multiple languages
/// - **Voice Cloning**: Create custom voices from audio samples
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
/// use waav_gateway::core::tts::playht::PlayHtTts;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-api-key".to_string(),
///     voice_id: Some("s3://voice/.../manifest.json".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = PlayHtTts::with_user_id(config, "your-user-id".to_string())?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct PlayHtTts {
    /// Generic HTTP-based TTS provider for connection pooling and streaming.
    provider: TTSProvider,

    /// Play.ht-specific request builder.
    request_builder: PlayHtRequestBuilder,

    /// Precomputed configuration hash for cache keying.
    config_hash: String,
}

impl PlayHtTts {
    /// Creates a new Play.ht TTS provider instance.
    ///
    /// The user ID is retrieved from the `PLAYHT_USER_ID` environment variable.
    /// This is required for Play.ht API authentication (dual-header auth).
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration with API key and voice settings
    ///
    /// # Returns
    /// * `Ok(Self)` - A new provider instance ready for connection
    /// * `Err(TTSError::InvalidConfiguration)` - If configuration is invalid or PLAYHT_USER_ID not set
    ///
    /// # Environment Variables
    /// * `PLAYHT_USER_ID` - Required. Your Play.ht user ID.
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        // Get user_id from environment variable
        let user_id = std::env::var("PLAYHT_USER_ID").unwrap_or_default();
        if user_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "PLAYHT_USER_ID environment variable is required for Play.ht authentication"
                    .to_string(),
            ));
        }

        Self::with_user_id(config, user_id)
    }

    /// Creates a new Play.ht TTS provider instance with explicit user ID.
    ///
    /// Use this method when you want to provide the user ID directly
    /// instead of reading from the environment variable.
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration with API key and voice settings
    /// * `user_id` - Play.ht user ID for authentication
    ///
    /// # Returns
    /// * `Ok(Self)` - A new provider instance ready for connection
    /// * `Err(TTSError::InvalidConfiguration)` - If configuration is invalid
    pub fn with_user_id(config: TTSConfig, user_id: String) -> TTSResult<Self> {
        // Create Play.ht-specific config
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), user_id);

        // Validate configuration
        if let Err(e) = playht_config.validate() {
            return Err(TTSError::InvalidConfiguration(e));
        }

        // Create request builder
        let request_builder = PlayHtRequestBuilder::new(config.clone(), playht_config.clone());

        // Compute config hash for caching
        let config_hash = compute_playht_tts_config_hash(&config, &playht_config);

        info!(
            "Created PlayHtTts provider: voice={}, voice_engine={}, format={:?}",
            playht_config.voice_id(),
            playht_config.voice_engine,
            playht_config.output_format
        );

        Ok(Self {
            provider: TTSProvider::new()?,
            request_builder,
            config_hash,
        })
    }

    /// Creates a new Play.ht TTS provider with custom configuration.
    ///
    /// This method allows full control over Play.ht-specific settings including
    /// voice engine, dialogue parameters, and guidance values.
    ///
    /// # Arguments
    /// * `playht_config` - Complete Play.ht TTS configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::playht::{PlayHtTts, PlayHtTtsConfig, PlayHtModel};
    /// use waav_gateway::core::tts::TTSConfig;
    ///
    /// let base = TTSConfig {
    ///     api_key: "your-api-key".to_string(),
    ///     voice_id: Some("s3://voice/.../manifest.json".to_string()),
    ///     ..Default::default()
    /// };
    ///
    /// let config = PlayHtTtsConfig::from_base(base, "your-user-id".to_string())
    ///     .with_model(PlayHtModel::PlayDialog)
    ///     .with_speed(1.2);
    ///
    /// let tts = PlayHtTts::with_config(config)?;
    /// ```
    pub fn with_config(playht_config: PlayHtTtsConfig) -> TTSResult<Self> {
        // Validate configuration
        if let Err(e) = playht_config.validate() {
            return Err(TTSError::InvalidConfiguration(e));
        }

        let config = playht_config.base.clone();
        let request_builder = PlayHtRequestBuilder::new(config.clone(), playht_config.clone());
        let config_hash = compute_playht_tts_config_hash(&config, &playht_config);

        info!(
            "Created PlayHtTts provider with config: voice={}, speed={}",
            playht_config.voice_id(),
            playht_config.speed
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

    /// Returns a reference to the Play.ht-specific configuration.
    #[inline]
    pub fn playht_config(&self) -> &PlayHtTtsConfig {
        &self.request_builder.playht_config
    }

    /// Sets the voice ID.
    pub fn set_voice(&mut self, voice_id: impl Into<String>) {
        self.request_builder.playht_config.base.voice_id = Some(voice_id.into());
        self.recompute_config_hash();
    }

    /// Sets the voice engine (model).
    pub fn set_voice_engine(&mut self, engine: super::PlayHtModel) {
        self.request_builder.playht_config.voice_engine = engine;
        self.recompute_config_hash();
    }

    /// Sets the speed multiplier.
    ///
    /// # Arguments
    /// * `speed` - Value between 0.5 and 2.0 (clamped if out of range)
    pub fn set_speed(&mut self, speed: f32) {
        self.request_builder.playht_config.speed = speed.clamp(super::MIN_SPEED, super::MAX_SPEED);
        self.recompute_config_hash();
    }

    /// Sets the temperature value.
    ///
    /// # Arguments
    /// * `temperature` - Value between 0.0 and 1.0 (clamped if out of range)
    pub fn set_temperature(&mut self, temperature: f32) {
        self.request_builder.playht_config.temperature =
            Some(temperature.clamp(super::MIN_TEMPERATURE, super::MAX_TEMPERATURE));
        self.recompute_config_hash();
    }

    /// Sets the random seed for deterministic output.
    pub fn set_seed(&mut self, seed: i64) {
        self.request_builder.playht_config.seed = Some(seed);
        self.recompute_config_hash();
    }

    /// Clears the random seed.
    pub fn clear_seed(&mut self) {
        self.request_builder.playht_config.seed = None;
        self.recompute_config_hash();
    }

    /// Sets the language code.
    pub fn set_language(&mut self, language: impl Into<String>) {
        self.request_builder.playht_config.language = Some(language.into());
        self.recompute_config_hash();
    }

    /// Sets the second speaker voice for PlayDialog.
    pub fn set_voice_2(&mut self, voice: impl Into<String>) {
        self.request_builder.playht_config.voice_2 = Some(voice.into());
        self.recompute_config_hash();
    }

    /// Sets the turn prefixes for PlayDialog multi-turn.
    pub fn set_turn_prefixes(&mut self, prefix1: impl Into<String>, prefix2: impl Into<String>) {
        self.request_builder.playht_config.turn_prefix = Some(prefix1.into());
        self.request_builder.playht_config.turn_prefix_2 = Some(prefix2.into());
        self.recompute_config_hash();
    }

    /// Recomputes the config hash after parameter changes.
    fn recompute_config_hash(&mut self) {
        self.config_hash = compute_playht_tts_config_hash(
            &self.request_builder.config,
            &self.request_builder.playht_config,
        );
    }

    /// Validates text length before synthesis.
    ///
    /// Uses character count (not byte length) to properly handle Unicode text.
    /// Play.ht API allows up to 20,000 characters per request.
    fn validate_text(text: &str) -> TTSResult<()> {
        // Use char count for proper Unicode handling (not byte length)
        let char_count = text.chars().count();
        if char_count > MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text exceeds maximum length of {} characters (got {})",
                MAX_TEXT_LENGTH, char_count
            )));
        }
        Ok(())
    }

    /// Parses a Play.ht API error response body into a structured error.
    ///
    /// This is useful for extracting detailed error information from failed API calls.
    ///
    /// # Arguments
    /// * `response_body` - The raw response body from the API
    ///
    /// # Returns
    /// * `Some(PlayHtApiError)` - If the response could be parsed as a Play.ht error
    /// * `None` - If parsing failed
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // When handling API errors, parse the response for details:
    /// if let Some(api_error) = PlayHtTts::parse_api_error(&response_body) {
    ///     eprintln!("Play.ht API error: {}", api_error);
    ///     if let Some(code) = &api_error.code {
    ///         eprintln!("Error code: {}", code);
    ///     }
    /// }
    /// ```
    pub fn parse_api_error(response_body: &[u8]) -> Option<PlayHtApiError> {
        match serde_json::from_slice::<PlayHtApiError>(response_body) {
            Ok(error) => {
                // Only return if there's actually error content
                if error.message.is_some() || error.code.is_some() {
                    Some(error)
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    /// Converts an HTTP status code and response body into a descriptive error.
    ///
    /// This method parses Play.ht API error responses to provide actionable error messages.
    ///
    /// # Arguments
    /// * `status` - HTTP status code
    /// * `response_body` - Optional response body bytes
    ///
    /// # Returns
    /// A `TTSError` with detailed error information
    pub fn error_from_response(status: u16, response_body: Option<&[u8]>) -> TTSError {
        // Try to parse the API error from response body
        let api_error = response_body.and_then(Self::parse_api_error);

        // Build error message based on status code and API error
        let message = match (status, &api_error) {
            // Unauthorized
            (401, Some(err)) => format!(
                "Play.ht authentication failed: {}. Verify your API key and user ID.",
                err
            ),
            (401, None) => {
                "Play.ht authentication failed. Verify your API key (AUTHORIZATION header) and user ID (X-USER-ID header).".to_string()
            }

            // Forbidden
            (403, Some(err)) => format!(
                "Play.ht access denied: {}. Check your subscription and voice permissions.",
                err
            ),
            (403, None) => {
                "Play.ht access denied. Check your subscription tier and voice permissions."
                    .to_string()
            }

            // Not Found
            (404, Some(err)) => format!("Play.ht resource not found: {}", err),
            (404, None) => "Play.ht voice not found. Verify the voice ID is correct.".to_string(),

            // Rate Limited
            (429, Some(err)) => format!(
                "Play.ht rate limit exceeded: {}. Please retry after a short delay.",
                err
            ),
            (429, None) => {
                "Play.ht rate limit exceeded. Please retry after a short delay.".to_string()
            }

            // Server Errors
            (500..=599, Some(err)) => {
                format!("Play.ht server error ({}): {}", status, err)
            }
            (500..=599, None) => {
                format!("Play.ht server error ({}). Please retry later.", status)
            }

            // Generic error with API message
            (_, Some(err)) => format!("Play.ht API error ({}): {}", status, err),

            // Generic error without API message
            (_, None) => format!("Play.ht API request failed with status {}", status),
        };

        // Log the error details for debugging
        if let Some(err) = &api_error {
            warn!(
                status = status,
                error_message = ?err.message,
                error_code = ?err.code,
                "Play.ht API error"
            );
        }

        TTSError::ProviderError(message)
    }
}

#[async_trait]
impl BaseTTS for PlayHtTts {
    /// Create a new instance of the TTS provider.
    ///
    /// The user ID is retrieved from the `PLAYHT_USER_ID` environment variable.
    /// For explicit user ID, use `PlayHtTts::with_user_id(config, user_id)` instead.
    fn new(config: TTSConfig) -> TTSResult<Self> {
        // Delegate to the main new() which reads from PLAYHT_USER_ID env var
        PlayHtTts::new(config)
    }

    /// Get the underlying TTSProvider for HTTP-based providers.
    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    /// Connect to the TTS provider.
    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(PLAYHT_TTS_URL, &self.request_builder.config)
            .await?;

        info!("Play.ht TTS provider connected and ready");
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
            info!("Play.ht TTS not ready, attempting to connect...");
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
            "provider": "playht",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "endpoint": PLAYHT_TTS_URL,
            "voice": self.request_builder.playht_config.voice_id(),
            "voice_engine": self.request_builder.playht_config.voice_engine.as_str(),
            "speed": self.request_builder.playht_config.speed,
            "supported_formats": ["mp3", "wav", "mulaw", "flac", "ogg", "raw"],
            "supported_sample_rates": [8000, 16000, 24000, 44100, 48000],
            "features": {
                "voice_cloning": true,
                "min_clone_audio_seconds": 30,
                "max_text_length": MAX_TEXT_LENGTH,
                "emotion_control": false,
                "multi_turn_dialogue": self.request_builder.playht_config.voice_engine.supports_dialogue(),
                "speed_range": [0.5, 2.0],
                "temperature_range": [0.0, 1.0],
            },
            "supported_models": [
                "Play3.0-mini",
                "PlayDialog",
                "PlayDialogMultilingual",
                "PlayDialogArabic",
                "PlayHT2.0-turbo"
            ],
            "supported_languages": [
                "af", "sq", "am", "ar", "bn", "bg", "ca", "hr", "cs", "da",
                "nl", "en", "fr", "gl", "de", "el", "he", "hi", "hu", "id",
                "it", "ja", "ko", "ms", "zh", "pl", "pt", "ru", "sr", "es",
                "sv", "tl", "th", "tr", "uk", "ur", "xh"
            ],
            "documentation": "https://docs.play.ht"
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
    use crate::core::tts::playht::{PlayHtAudioFormat, PlayHtModel};

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "playht".to_string(),
            api_key: "test-api-key".to_string(),
            voice_id: Some("s3://test-voice/manifest.json".to_string()),
            model: String::new(),
            speaking_rate: None,
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(48000),
            connection_timeout: Some(30),
            request_timeout: Some(60),
            pronunciations: Vec::new(),
            request_pool_size: Some(4),
            emotion_config: None,
        }
    }

    // =========================================================================
    // PlayHtRequestBuilder Tests
    // =========================================================================

    #[test]
    fn test_playht_request_builder_new() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());

        let builder = PlayHtRequestBuilder::new(config, playht_config);

        assert!(builder.pronunciation_replacer.is_none());
        assert_eq!(builder.playht_config.output_format, PlayHtAudioFormat::Mp3);
    }

    #[test]
    fn test_playht_request_builder_with_pronunciations() {
        let mut config = create_test_config();
        config.pronunciations = vec![Pronunciation {
            word: "API".to_string(),
            pronunciation: "A P I".to_string(),
        }];

        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        assert!(builder.pronunciation_replacer.is_some());
    }

    #[test]
    fn test_playht_request_builder_get_config() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let retrieved = builder.get_config();
        assert_eq!(
            retrieved.voice_id,
            Some("s3://test-voice/manifest.json".to_string())
        );
    }

    #[test]
    fn test_playht_request_builder_build_request_body_default() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let body = builder.build_request_body("Hello world");

        assert_eq!(body["voice"], "s3://test-voice/manifest.json");
        assert_eq!(body["text"], "Hello world");
        assert_eq!(body["voice_engine"], "Play3.0-mini");
        assert_eq!(body["output_format"], "mp3");
        assert_eq!(body["sample_rate"], 48000);
        // Default speed should not be in body
        assert!(body.get("speed").is_none());
    }

    #[test]
    fn test_playht_request_builder_build_request_body_custom() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string())
            .with_model(PlayHtModel::PlayDialog)
            .with_speed(1.5)
            .with_temperature(0.8)
            .with_seed(12345)
            .with_turn_prefixes("S1:", "S2:");
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let body = builder.build_request_body("Hello");

        assert_eq!(body["voice"], "s3://test-voice/manifest.json");
        assert_eq!(body["text"], "Hello");
        assert_eq!(body["voice_engine"], "PlayDialog");
        let speed = body["speed"].as_f64().unwrap();
        assert!((speed - 1.5).abs() < 0.001, "speed: {}", speed);
        let temp = body["temperature"].as_f64().unwrap();
        assert!((temp - 0.8).abs() < 0.001, "temperature: {}", temp);
        assert_eq!(body["seed"], 12345);
        assert_eq!(body["turn_prefix"], "S1:");
        assert_eq!(body["turn_prefix_2"], "S2:");
    }

    // =========================================================================
    // HTTP Request Building Tests
    // =========================================================================

    #[test]
    fn test_build_http_request_url() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        assert_eq!(request.url().as_str(), PLAYHT_TTS_URL);
        assert_eq!(request.method(), reqwest::Method::POST);
    }

    #[test]
    fn test_build_http_request_headers() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        // Verify X-USER-ID header
        let user_id_header = request.headers().get("x-user-id").unwrap();
        assert_eq!(user_id_header.to_str().unwrap(), "test-user");

        // Verify AUTHORIZATION header
        let auth_header = request.headers().get("authorization").unwrap();
        assert_eq!(auth_header.to_str().unwrap(), "test-api-key");

        // Verify Content-Type header
        let content_type = request.headers().get("content-type").unwrap();
        assert_eq!(content_type.to_str().unwrap(), "application/json");

        // Verify Accept header (for MP3)
        let accept_header = request.headers().get("accept").unwrap();
        assert_eq!(accept_header.to_str().unwrap(), "audio/mpeg");
    }

    #[test]
    fn test_build_http_request_headers_raw() {
        let mut config = create_test_config();
        config.audio_format = Some("raw".to_string());
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Test").build().unwrap();

        let accept = request.headers().get("accept").unwrap();
        assert_eq!(accept.to_str().unwrap(), "audio/pcm");
    }

    #[test]
    fn test_build_http_request_body() {
        let config = create_test_config();
        let playht_config =
            PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string()).with_speed(1.5);
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let client = reqwest::Client::new();
        let request = builder
            .build_http_request(&client, "Hello world")
            .build()
            .unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["voice"], "s3://test-voice/manifest.json");
        assert_eq!(body_json["text"], "Hello world");
        assert_eq!(body_json["voice_engine"], "Play3.0-mini");
        assert_eq!(body_json["output_format"], "mp3");
        assert_eq!(body_json["sample_rate"], 48000);
    }

    // =========================================================================
    // Config Hash Tests
    // =========================================================================

    #[test]
    fn test_compute_playht_tts_config_hash() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());

        let hash = compute_playht_tts_config_hash(&config, &playht_config);

        // Hash should be 32-char hex
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Same config produces same hash
        let hash2 = compute_playht_tts_config_hash(&config, &playht_config);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_config_hash_different_voice() {
        let config = create_test_config();
        let playht_config1 = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());

        let mut config2 = config.clone();
        config2.voice_id = Some("different".to_string());
        let playht_config2 = PlayHtTtsConfig::from_base(config2.clone(), "test-user".to_string());

        let hash1 = compute_playht_tts_config_hash(&config, &playht_config1);
        let hash2 = compute_playht_tts_config_hash(&config2, &playht_config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_config_hash_different_speed() {
        let config = create_test_config();
        let playht_config1 = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let playht_config2 =
            PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string()).with_speed(1.5);

        let hash1 = compute_playht_tts_config_hash(&config, &playht_config1);
        let hash2 = compute_playht_tts_config_hash(&config, &playht_config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_config_hash_different_model() {
        let config = create_test_config();
        let playht_config1 = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let playht_config2 = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string())
            .with_model(PlayHtModel::PlayDialog);

        let hash1 = compute_playht_tts_config_hash(&config, &playht_config1);
        let hash2 = compute_playht_tts_config_hash(&config, &playht_config2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_config_hash_different_seed() {
        let config = create_test_config();
        let playht_config1 = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let playht_config2 =
            PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string()).with_seed(12345);

        let hash1 = compute_playht_tts_config_hash(&config, &playht_config1);
        let hash2 = compute_playht_tts_config_hash(&config, &playht_config2);

        assert_ne!(hash1, hash2);
    }

    // =========================================================================
    // PlayHtTts Provider Tests
    // =========================================================================

    #[test]
    fn test_playht_tts_creation() {
        let config = create_test_config();
        let tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[test]
    fn test_playht_tts_with_config() {
        let base = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(base, "test-user".to_string())
            .with_model(PlayHtModel::PlayDialog)
            .with_speed(1.5);

        let tts = PlayHtTts::with_config(playht_config).unwrap();

        assert_eq!(tts.playht_config().voice_engine, PlayHtModel::PlayDialog);
        assert!((tts.playht_config().speed - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_playht_tts_set_voice() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_voice("new-voice-id");

        assert_eq!(tts.playht_config().voice_id(), "new-voice-id");
    }

    #[test]
    fn test_playht_tts_set_voice_engine() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_voice_engine(PlayHtModel::PlayDialog);

        assert_eq!(tts.playht_config().voice_engine, PlayHtModel::PlayDialog);
    }

    #[test]
    fn test_playht_tts_set_speed() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_speed(1.5);

        assert!((tts.playht_config().speed - 1.5).abs() < 0.001);
    }

    #[test]
    fn test_playht_tts_set_speed_clamped() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_speed(3.0);
        assert!((tts.playht_config().speed - super::super::MAX_SPEED).abs() < 0.001);

        tts.set_speed(0.1);
        assert!((tts.playht_config().speed - super::super::MIN_SPEED).abs() < 0.001);
    }

    #[test]
    fn test_playht_tts_set_temperature() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_temperature(0.8);

        assert_eq!(tts.playht_config().temperature, Some(0.8));
    }

    #[test]
    fn test_playht_tts_set_seed() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_seed(12345);

        assert_eq!(tts.playht_config().seed, Some(12345));
    }

    #[test]
    fn test_playht_tts_clear_seed() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_seed(12345);
        tts.clear_seed();

        assert!(tts.playht_config().seed.is_none());
    }

    #[test]
    fn test_playht_tts_set_language() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_language("es");

        assert_eq!(tts.playht_config().language, Some("es".to_string()));
    }

    #[test]
    fn test_playht_tts_set_turn_prefixes() {
        let config = create_test_config();
        let mut tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        tts.set_turn_prefixes("Speaker1:", "Speaker2:");

        assert_eq!(
            tts.playht_config().turn_prefix,
            Some("Speaker1:".to_string())
        );
        assert_eq!(
            tts.playht_config().turn_prefix_2,
            Some("Speaker2:".to_string())
        );
    }

    #[test]
    fn test_playht_tts_get_provider_info() {
        let config = create_test_config();
        let tts = PlayHtTts::with_user_id(config, "test-user".to_string()).unwrap();

        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "playht");
        assert_eq!(info["version"], "1.0.0");
        assert_eq!(info["api_type"], "HTTP REST");
        assert_eq!(info["connection_pooling"], true);
        assert_eq!(info["endpoint"], PLAYHT_TTS_URL);
        assert_eq!(info["voice"], "s3://test-voice/manifest.json");
        assert_eq!(info["voice_engine"], "Play3.0-mini");
        assert_eq!(info["features"]["voice_cloning"], true);
        assert_eq!(info["features"]["emotion_control"], false);
        assert_eq!(info["features"]["max_text_length"], MAX_TEXT_LENGTH);
        assert!(info["supported_formats"].is_array());
        assert!(info["supported_languages"].is_array());
        assert!(info["supported_models"].is_array());
    }

    // =========================================================================
    // Validation Tests
    // =========================================================================

    #[test]
    fn test_validate_text_success() {
        let result = PlayHtTts::validate_text("Hello, world!");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_text_too_long() {
        let long_text = "a".repeat(MAX_TEXT_LENGTH + 1);
        let result = PlayHtTts::validate_text(&long_text);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, TTSError::InvalidConfiguration(_)));
    }

    #[test]
    fn test_validate_text_at_limit() {
        let text = "a".repeat(MAX_TEXT_LENGTH);
        let result = PlayHtTts::validate_text(&text);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_text_unicode_characters() {
        // Test with Unicode: "你好" is 2 characters but 6 bytes
        // Create a string of exactly MAX_TEXT_LENGTH Unicode characters
        let unicode_text = "你".repeat(MAX_TEXT_LENGTH);
        let result = PlayHtTts::validate_text(&unicode_text);
        assert!(result.is_ok());

        // One more character should fail
        let too_long = "你".repeat(MAX_TEXT_LENGTH + 1);
        let result = PlayHtTts::validate_text(&too_long);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_text_mixed_unicode() {
        // Test mixed ASCII and Unicode - should count characters not bytes
        // "Hello 世界" = 8 characters but 12 bytes
        let mixed = "Hello 世界";
        assert_eq!(mixed.chars().count(), 8);
        assert_eq!(mixed.len(), 12); // bytes
        let result = PlayHtTts::validate_text(mixed);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_text_emoji() {
        // Emoji: "👋" is 1 character but 4 bytes
        let emoji_text = "👋".repeat(MAX_TEXT_LENGTH);
        let result = PlayHtTts::validate_text(&emoji_text);
        assert!(result.is_ok());

        // One more should fail
        let too_long = "👋".repeat(MAX_TEXT_LENGTH + 1);
        let result = PlayHtTts::validate_text(&too_long);
        assert!(result.is_err());
    }

    // =========================================================================
    // Edge Case Tests
    // =========================================================================

    #[test]
    fn test_build_http_request_empty_text() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "").build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body).unwrap();

        assert_eq!(body_json["text"], "");
    }

    #[test]
    fn test_build_http_request_unicode() {
        let config = create_test_config();
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

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
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

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
        let playht_config = PlayHtTtsConfig::from_base(config.clone(), "test-user".to_string());
        let builder = PlayHtRequestBuilder::new(config, playht_config);

        let cloned = builder.clone();

        assert_eq!(cloned.config.api_key, builder.config.api_key);
        assert_eq!(
            cloned.playht_config.voice_id(),
            builder.playht_config.voice_id()
        );
        assert_eq!(cloned.playht_config.user_id, builder.playht_config.user_id);
    }

    // =========================================================================
    // API Error Parsing Tests
    // =========================================================================

    #[test]
    fn test_parse_api_error_with_message() {
        let json = r#"{"message": "Rate limit exceeded", "code": "RATE_LIMIT", "status": 429}"#;
        let error = PlayHtTts::parse_api_error(json.as_bytes());

        assert!(error.is_some());
        let err = error.unwrap();
        assert_eq!(err.message, Some("Rate limit exceeded".to_string()));
        assert_eq!(err.code, Some("RATE_LIMIT".to_string()));
        assert_eq!(err.status, Some(429));
    }

    #[test]
    fn test_parse_api_error_with_alias_fields() {
        let json = r#"{"error_message": "Bad request", "error_code": "BAD_REQUEST"}"#;
        let error = PlayHtTts::parse_api_error(json.as_bytes());

        assert!(error.is_some());
        let err = error.unwrap();
        assert_eq!(err.message, Some("Bad request".to_string()));
        assert_eq!(err.code, Some("BAD_REQUEST".to_string()));
    }

    #[test]
    fn test_parse_api_error_empty_json() {
        let json = r#"{}"#;
        let error = PlayHtTts::parse_api_error(json.as_bytes());

        // Empty error should return None (no useful info)
        assert!(error.is_none());
    }

    #[test]
    fn test_parse_api_error_invalid_json() {
        let json = "not valid json";
        let error = PlayHtTts::parse_api_error(json.as_bytes());

        assert!(error.is_none());
    }

    #[test]
    fn test_error_from_response_401() {
        let error = PlayHtTts::error_from_response(401, None);
        assert!(matches!(error, TTSError::ProviderError(_)));
        let msg = format!("{:?}", error);
        assert!(msg.contains("authentication"));
    }

    #[test]
    fn test_error_from_response_401_with_body() {
        let json = r#"{"message": "Invalid API key"}"#;
        let error = PlayHtTts::error_from_response(401, Some(json.as_bytes()));
        let msg = format!("{:?}", error);
        assert!(msg.contains("Invalid API key"));
    }

    #[test]
    fn test_error_from_response_403() {
        let error = PlayHtTts::error_from_response(403, None);
        let msg = format!("{:?}", error);
        assert!(msg.contains("access denied") || msg.contains("subscription"));
    }

    #[test]
    fn test_error_from_response_404() {
        let error = PlayHtTts::error_from_response(404, None);
        let msg = format!("{:?}", error);
        assert!(msg.contains("not found") || msg.contains("voice"));
    }

    #[test]
    fn test_error_from_response_429() {
        let error = PlayHtTts::error_from_response(429, None);
        let msg = format!("{:?}", error);
        assert!(msg.contains("rate limit"));
    }

    #[test]
    fn test_error_from_response_500() {
        let error = PlayHtTts::error_from_response(500, None);
        let msg = format!("{:?}", error);
        assert!(msg.contains("server error") || msg.contains("500"));
    }

    #[test]
    fn test_error_from_response_generic() {
        let error = PlayHtTts::error_from_response(418, None);
        let msg = format!("{:?}", error);
        assert!(msg.contains("418"));
    }
}
