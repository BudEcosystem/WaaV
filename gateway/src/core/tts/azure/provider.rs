//! Microsoft Azure Text-to-Speech request builder and provider implementation.
//!
//! This module implements the `TTSRequestBuilder` trait for Azure TTS, which builds
//! HTTP POST requests with proper headers, authentication, and SSML body for the
//! Azure Text-to-Speech REST API.
//!
//! # Architecture
//!
//! The `AzureRequestBuilder` constructs HTTP requests for the Azure TTS API:
//! - URL: `https://{region}.tts.speech.microsoft.com/cognitiveservices/v1`
//! - Authentication: `Ocp-Apim-Subscription-Key` header
//! - Content-Type: `application/ssml+xml`
//! - Output format: `X-Microsoft-OutputFormat` header
//!
//! The `AzureTTS` struct is the main TTS provider that uses the generic `TTSProvider`
//! infrastructure with `AzureRequestBuilder` for Azure-specific request construction.
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::azure::AzureTTS;
//! use waav_gateway::core::tts::{TTSConfig, BaseTTS};
//!
//! let config = TTSConfig {
//!     api_key: "your-azure-subscription-key".to_string(),
//!     voice_id: Some("en-US-JennyNeural".to_string()),
//!     audio_format: Some("linear16".to_string()),
//!     sample_rate: Some(24000),
//!     ..Default::default()
//! };
//!
//! let mut tts = AzureTTS::new(config)?;
//! tts.connect().await?;
//! tts.speak("Hello, world!", true).await?;
//! ```

use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;
use xxhash_rust::xxh3::xxh3_128;

use super::config::{AZURE_OUTPUT_FORMAT_HEADER, AzureTTSConfig};
use crate::core::providers::azure::{
    AZURE_AUTHORIZATION_HEADER, AZURE_SUBSCRIPTION_KEY_HEADER, AzureRegion,
};
use crate::core::tts::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSResult};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

/// User-Agent header value for Azure TTS requests.
const USER_AGENT: &str = "waav-gateway";

// =============================================================================
// AzureRequestBuilder
// =============================================================================

/// Azure-specific request builder for constructing TTS API requests.
///
/// Implements `TTSRequestBuilder` to construct HTTP POST requests for the
/// Azure Text-to-Speech REST API with proper headers and SSML body.
#[derive(Clone)]
pub struct AzureRequestBuilder {
    /// Base TTS configuration (shared across all providers).
    config: TTSConfig,
    /// Azure-specific configuration.
    azure_config: AzureTTSConfig,
    /// Compiled pronunciation patterns for text replacement.
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl AzureRequestBuilder {
    /// Creates a new Azure request builder.
    ///
    /// # Arguments
    ///
    /// * `config` - Base TTS configuration
    /// * `azure_config` - Azure-specific configuration
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::azure::{AzureRequestBuilder, AzureTTSConfig};
    /// use waav_gateway::core::tts::TTSConfig;
    ///
    /// let base = TTSConfig::default();
    /// let azure = AzureTTSConfig::from_base(base.clone());
    ///
    /// let builder = AzureRequestBuilder::new(base, azure);
    /// ```
    pub fn new(config: TTSConfig, azure_config: AzureTTSConfig) -> Self {
        // Create pronunciation replacer if pronunciations are configured
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        Self {
            config,
            azure_config,
            pronunciation_replacer,
        }
    }
}

impl TTSRequestBuilder for AzureRequestBuilder {
    /// Build the Azure TTS HTTP request with URL, headers, and SSML body.
    ///
    /// # Request Format
    ///
    /// - **URL**: `https://{region}.tts.speech.microsoft.com/cognitiveservices/v1`
    /// - **Method**: POST
    /// - **Headers**:
    ///   - `Ocp-Apim-Subscription-Key`: Azure subscription key
    ///   - `Content-Type`: `application/ssml+xml`
    ///   - `X-Microsoft-OutputFormat`: Audio format (e.g., `raw-24khz-16bit-mono-pcm`)
    ///   - `User-Agent`: Identifies the client application
    /// - **Body**: SSML document with voice and optional prosody settings
    ///
    /// # Arguments
    ///
    /// * `client` - The HTTP client to use for building the request
    /// * `text` - The text to synthesize
    ///
    /// # Returns
    ///
    /// A `reqwest::RequestBuilder` ready to be sent.
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build the TTS endpoint URL.
        let url = self.azure_config.build_tts_url();

        // Build the SSML body
        let ssml_body = self.azure_config.build_ssml_for_text(text);

        // Get the output format header value
        let output_format = self.azure_config.output_format.as_str();

        // Build the request with Azure-specific headers and SSML body. The synth POST honors an
        // endpoint override (W-T0 mock/proxy), keeping the `cognitiveservices/v1` path+query.
        let mut request = client
            .post(crate::core::tts::standard::override_rest_endpoint(
                &url,
                self.azure_config.endpoint_override.as_deref(),
            ))
            .header("Content-Type", "application/ssml+xml")
            .header(AZURE_OUTPUT_FORMAT_HEADER, output_format)
            .header("User-Agent", USER_AGENT)
            .body(ssml_body);

        // Custom-voice deployment id → `?deploymentId=...` query param (reqwest percent-encodes).
        if let Some(deployment_id) = self
            .azure_config
            .deployment_id
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            request = request.query(&[("deploymentId", deployment_id)]);
        }

        // Authenticate with a Bearer (Entra) token when supplied, otherwise the subscription key.
        // The cognitiveservices/v1 endpoint accepts either credential.
        if let Some(token) = self
            .azure_config
            .auth_token
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            request.header(AZURE_AUTHORIZATION_HEADER, format!("Bearer {token}"))
        } else {
            request.header(AZURE_SUBSCRIPTION_KEY_HEADER, &self.config.api_key)
        }
    }

    /// Returns a reference to the base TTS configuration.
    fn get_config(&self) -> &TTSConfig {
        &self.config
    }

    /// Returns the precompiled pronunciation replacer if configured.
    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Config Hash
// =============================================================================

/// Computes a hash of the TTS configuration for cache keying.
///
/// This creates a stable hash from configuration fields that affect audio output,
/// ensuring different configurations produce different cache keys.
fn compute_azure_tts_config_hash(config: &TTSConfig, azure_config: &AzureTTSConfig) -> String {
    let mut s = String::new();
    s.push_str("azure|");
    s.push_str(config.voice_id.as_deref().unwrap_or(""));
    s.push('|');
    s.push_str(&config.model);
    s.push('|');
    s.push_str(azure_config.output_format.as_str());
    s.push('|');
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }
    s.push('|');
    s.push_str(azure_config.region.as_str());
    s.push('|');
    // Emotion (mstts:express-as style) changes the audio, so it MUST be in the cache key — else two
    // requests with the same text/voice/rate but different emotion collide. (Review S1.)
    if let Some(emotion) = &azure_config.emotion {
        s.push_str(emotion);
    }
    s.push('|');

    // Every SSML option that alters the rendered audio (pitch/volume/contour/range, express-as
    // styledegree/role, emphasis, say-as, phoneme, sub, lexicon, audioduration, silence, voice
    // effect, personal-voice profile, background audio) and the language override / deployment id
    // must be folded into the cache key, else two requests with identical text/voice but different
    // SSML knobs collide and return the wrong audio. (Review S1 collision bug class.)
    let o = &azure_config.ssml_options;
    for part in [
        azure_config.language_override.as_deref(),
        azure_config.deployment_id.as_deref(),
        o.pitch.as_deref(),
        o.volume.as_deref(),
        o.contour.as_deref(),
        o.range.as_deref(),
        o.style_degree.as_deref(),
        o.role.as_deref(),
        o.emphasis.as_deref(),
        o.say_as_interpret_as.as_deref(),
        o.say_as_format.as_deref(),
        o.say_as_detail.as_deref(),
        o.phoneme_alphabet.as_deref(),
        o.phoneme_ph.as_deref(),
        o.sub_alias.as_deref(),
        o.lexicon_uri.as_deref(),
        o.audio_duration.as_deref(),
        o.silence_type.as_deref(),
        o.silence_value.as_deref(),
        o.effect.as_deref(),
        o.speaker_profile_id.as_deref(),
        o.background_audio_src.as_deref(),
        o.background_audio_volume.as_deref(),
        o.background_audio_fadein.as_deref(),
        o.background_audio_fadeout.as_deref(),
    ] {
        if let Some(p) = part {
            s.push_str(p);
        }
        s.push('|');
    }
    // NOTE: `auth_token` is deliberately excluded — it is a header-only credential and does not
    // change the synthesized audio.

    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

// =============================================================================
// AzureTTS Provider
// =============================================================================

/// Microsoft Azure Text-to-Speech provider implementation.
///
/// This provider uses the Azure Cognitive Services Text-to-Speech REST API
/// for speech synthesis. It supports all Azure neural voices and audio formats.
///
/// # Features
///
/// - Neural voices with natural prosody
/// - Multiple audio formats (PCM, MP3, Opus)
/// - Speaking rate control via SSML
/// - Pronunciation replacement
/// - Audio caching support
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::azure::AzureTTS;
/// use waav_gateway::core::tts::{TTSConfig, BaseTTS};
///
/// let config = TTSConfig {
///     api_key: "your-azure-subscription-key".to_string(),
///     voice_id: Some("en-US-JennyNeural".to_string()),
///     ..Default::default()
/// };
///
/// let mut tts = AzureTTS::new(config)?;
/// tts.connect().await?;
/// tts.speak("Hello, world!", true).await?;
/// tts.disconnect().await?;
/// ```
pub struct AzureTTS {
    /// Generic HTTP-based TTS provider for connection pooling and audio streaming.
    provider: TTSProvider,
    /// Azure-specific request builder.
    request_builder: AzureRequestBuilder,
    /// Precomputed configuration hash for cache keying.
    config_hash: String,
}

impl AzureTTS {
    fn from_parts_unchecked(config: TTSConfig, azure_config: AzureTTSConfig) -> Self {
        let request_builder = AzureRequestBuilder::new(config.clone(), azure_config.clone());
        let config_hash = compute_azure_tts_config_hash(&config, &azure_config);

        Self {
            provider: TTSProvider::new(),
            request_builder,
            config_hash,
        }
    }

    /// Creates a new Azure TTS provider instance.
    ///
    /// # Arguments
    ///
    /// * `config` - Base TTS configuration with Azure subscription key and voice settings
    ///
    /// # Returns
    ///
    /// * `Ok(Self)` - A new provider instance ready for connection
    /// * `Err(TTSError)` - If configuration is invalid
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::{TTSConfig, azure::AzureTTS};
    ///
    /// let config = TTSConfig {
    ///     api_key: "your-azure-subscription-key".to_string(),
    ///     voice_id: Some("en-US-JennyNeural".to_string()),
    ///     audio_format: Some("linear16".to_string()),
    ///     sample_rate: Some(24000),
    ///     ..Default::default()
    /// };
    ///
    /// let tts = AzureTTS::new(config)?;
    /// ```
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        Self::with_region(config, AzureRegion::default())
    }

    /// Creates a new Azure TTS provider with a specific region.
    ///
    /// # Arguments
    ///
    /// * `config` - Base TTS configuration
    /// * `region` - Azure region for the Speech Service endpoint
    ///
    /// # Returns
    ///
    /// * `Ok(Self)` - A new provider instance ready for connection
    /// * `Err(TTSError)` - If configuration is invalid
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use waav_gateway::core::tts::{TTSConfig, azure::AzureTTS};
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let config = TTSConfig::default();
    /// let tts = AzureTTS::with_region(config, AzureRegion::WestEurope)?;
    /// ```
    pub fn with_region(config: TTSConfig, region: AzureRegion) -> TTSResult<Self> {
        // Create Azure-specific config with the specified region
        let azure_config = AzureTTSConfig::with_region(config.clone(), region);

        Ok(Self::from_parts_unchecked(config, azure_config))
    }

    /// Build the provider from the standardized config (W1 keystone), mirroring
    /// `DeepgramTTS::from_standard`. Delegates feature mapping to
    /// [`AzureTTSConfig::from_standard`] (the `ssml` → `use_ssml` flag plus the `region` extra) so
    /// the standardized SSML toggle reaches the request builder — which decides whether to emit
    /// `mstts:express-as` SSML — through the standardized dispatch instead of being dropped at the
    /// flat boundary. (Speed/pitch/volume/emotion are Azure SSML-time controls without struct
    /// fields, so they stay at default here, matching the config-level mapping.)
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let config = std.base.clone();
        let azure_config = AzureTTSConfig::from_standard(std)?;
        azure_config.validate()?;

        Ok(Self::from_parts_unchecked(config, azure_config))
    }

    /// Sets the request manager for this instance.
    ///
    /// This allows sharing a request manager across multiple provider instances
    /// for connection pooling.
    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }

    /// Returns the Azure-specific configuration.
    pub fn azure_config(&self) -> &AzureTTSConfig {
        &self.request_builder.azure_config
    }
}

impl Default for AzureTTS {
    fn default() -> Self {
        let config = TTSConfig::default();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        Self::from_parts_unchecked(config, azure_config)
    }
}

#[async_trait]
impl BaseTTS for AzureTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        AzureTTS::new(config)
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        let url = self.request_builder.azure_config.build_tts_url();
        self.provider
            .generic_connect_with_config(&url, &self.request_builder.config)
            .await?;
        info!("Azure TTS provider connected");
        Ok(())
    }

    async fn disconnect(&mut self) -> TTSResult<()> {
        self.provider.generic_disconnect().await
    }

    fn is_ready(&self) -> bool {
        self.provider.is_ready()
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.provider.get_connection_state()
    }

    async fn speak(&mut self, text: &str, flush: bool) -> TTSResult<()> {
        // Handle reconnection if needed
        if !self.is_ready() {
            info!("Azure TTS not ready, attempting to connect...");
            self.connect().await?;
        }

        // Set config hash once on first speak (idempotent)
        self.provider
            .set_tts_config_hash(self.config_hash.clone())
            .await;

        self.provider
            .generic_speak(self.request_builder.clone(), text, flush)
            .await
    }

    async fn clear(&mut self) -> TTSResult<()> {
        self.provider.generic_clear().await
    }

    async fn flush(&self) -> TTSResult<()> {
        self.provider.generic_flush().await
    }

    fn on_audio(&mut self, callback: Arc<dyn AudioCallback>) -> TTSResult<()> {
        self.provider.generic_on_audio(callback)
    }

    fn remove_audio_callback(&mut self) -> TTSResult<()> {
        self.provider.generic_remove_audio_callback()
    }

    fn get_provider_info(&self) -> serde_json::Value {
        serde_json::json!({
            "provider": "azure",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "endpoint": self.request_builder.azure_config.build_tts_url(),
            "region": self.request_builder.azure_config.region.as_str(),
            "supported_formats": [
                "raw-8khz-8bit-mono-mulaw",
                "raw-8khz-8bit-mono-alaw",
                "raw-8khz-16bit-mono-pcm",
                "raw-16khz-16bit-mono-pcm",
                "raw-22050hz-16bit-mono-pcm",
                "raw-24khz-16bit-mono-pcm",
                "raw-44100hz-16bit-mono-pcm",
                "raw-48khz-16bit-mono-pcm",
                "audio-16khz-32kbitrate-mono-mp3",
                "audio-16khz-64kbitrate-mono-mp3",
                "audio-24khz-48kbitrate-mono-mp3",
                "audio-24khz-96kbitrate-mono-mp3",
                "audio-48khz-96kbitrate-mono-mp3",
                "audio-48khz-192kbitrate-mono-mp3",
                "audio-16khz-16bit-32kbps-mono-opus",
                "audio-24khz-16bit-24kbps-mono-opus",
                "audio-24khz-16bit-48kbps-mono-opus"
            ],
            "supported_sample_rates": [8000, 16000, 22050, 24000, 44100, 48000],
            "documentation": "https://learn.microsoft.com/en-us/azure/ai-services/speech-service/rest-text-to-speech"
        })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tts::azure::AzureAudioEncoding;
    use crate::core::tts::base::Pronunciation;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn create_test_config() -> TTSConfig {
        TTSConfig {
            provider: "azure".to_string(),
            api_key: "test-subscription-key".to_string(),
            voice_id: Some("en-US-JennyNeural".to_string()),
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
    // AzureRequestBuilder Tests
    // =========================================================================

    #[test]
    fn test_azure_request_builder_new() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        assert!(builder.pronunciation_replacer.is_none());
        assert_eq!(
            builder.azure_config.output_format,
            AzureAudioEncoding::Raw24Khz16BitMonoPcm
        );
    }

    #[test]
    fn test_azure_request_builder_with_pronunciations() {
        let mut config = create_test_config();
        config.pronunciations = vec![
            Pronunciation {
                word: "API".to_string(),
                pronunciation: "A P I".to_string(),
            },
            Pronunciation {
                word: "SDK".to_string(),
                pronunciation: "S D K".to_string(),
            },
        ];

        let azure_config = AzureTTSConfig::from_base(config.clone());
        let builder = AzureRequestBuilder::new(config, azure_config);

        assert!(builder.pronunciation_replacer.is_some());
    }

    #[test]
    fn test_azure_request_builder_get_config() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let retrieved_config = builder.get_config();
        assert_eq!(
            retrieved_config.voice_id,
            Some("en-US-JennyNeural".to_string())
        );
        assert_eq!(retrieved_config.sample_rate, Some(24000));
    }

    #[test]
    fn test_azure_request_builder_get_pronunciation_replacer_none() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        assert!(builder.get_pronunciation_replacer().is_none());
    }

    #[test]
    fn test_azure_request_builder_get_pronunciation_replacer_some() {
        let mut config = create_test_config();
        config.pronunciations = vec![Pronunciation {
            word: "API".to_string(),
            pronunciation: "A P I".to_string(),
        }];

        let azure_config = AzureTTSConfig::from_base(config.clone());
        let builder = AzureRequestBuilder::new(config, azure_config);

        assert!(builder.get_pronunciation_replacer().is_some());
    }

    // =========================================================================
    // HTTP Request Building Tests
    // =========================================================================

    #[test]
    fn test_build_http_request_url() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        assert_eq!(
            request.url().as_str(),
            "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1"
        );
        assert_eq!(request.method(), reqwest::Method::POST);
    }

    #[test]
    fn test_build_http_request_url_different_regions() {
        let config = create_test_config();

        let test_cases = vec![
            (AzureRegion::EastUS, "eastus"),
            (AzureRegion::WestEurope, "westeurope"),
            (AzureRegion::JapanEast, "japaneast"),
            (AzureRegion::SoutheastAsia, "southeastasia"),
        ];

        for (region, region_str) in test_cases {
            let azure_config = AzureTTSConfig::with_region(config.clone(), region);
            let builder = AzureRequestBuilder::new(config.clone(), azure_config);

            let client = reqwest::Client::new();
            let request_builder = builder.build_http_request(&client, "Test");
            let request = request_builder.build().unwrap();

            let expected_url = format!(
                "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
                region_str
            );
            assert_eq!(request.url().as_str(), expected_url);
        }
    }

    #[test]
    fn test_build_http_request_headers() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        // Verify subscription key header
        let sub_key_header = request
            .headers()
            .get(AZURE_SUBSCRIPTION_KEY_HEADER)
            .unwrap();
        assert_eq!(sub_key_header.to_str().unwrap(), "test-subscription-key");

        // Verify Content-Type header
        let content_type = request.headers().get("content-type").unwrap();
        assert_eq!(content_type.to_str().unwrap(), "application/ssml+xml");

        // Verify X-Microsoft-OutputFormat header
        let output_format = request.headers().get(AZURE_OUTPUT_FORMAT_HEADER).unwrap();
        assert_eq!(output_format.to_str().unwrap(), "raw-24khz-16bit-mono-pcm");

        // Verify User-Agent header
        let user_agent = request.headers().get("user-agent").unwrap();
        assert_eq!(user_agent.to_str().unwrap(), USER_AGENT);
    }

    #[test]
    fn test_build_http_request_output_format_mp3() {
        let mut config = create_test_config();
        config.audio_format = Some("mp3".to_string());

        let azure_config = AzureTTSConfig::from_base(config.clone());
        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Test");
        let request = request_builder.build().unwrap();

        let output_format = request.headers().get(AZURE_OUTPUT_FORMAT_HEADER).unwrap();
        assert_eq!(
            output_format.to_str().unwrap(),
            "audio-24khz-96kbitrate-mono-mp3"
        );
    }

    #[test]
    fn test_build_http_request_output_format_mulaw() {
        let mut config = create_test_config();
        config.audio_format = Some("mulaw".to_string());
        config.sample_rate = Some(8000);

        let azure_config = AzureTTSConfig::from_base(config.clone());
        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Test");
        let request = request_builder.build().unwrap();

        let output_format = request.headers().get(AZURE_OUTPUT_FORMAT_HEADER).unwrap();
        assert_eq!(output_format.to_str().unwrap(), "raw-8khz-8bit-mono-mulaw");
    }

    #[test]
    fn test_build_http_request_body_contains_ssml() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello world");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // Verify SSML structure
        assert!(body_str.contains("<speak"));
        assert!(body_str.contains("version='1.0'"));
        assert!(body_str.contains("xml:lang='en-US'"));
        assert!(body_str.contains("<voice name='en-US-JennyNeural'>"));
        assert!(body_str.contains("Hello world"));
        assert!(body_str.contains("</speak>"));
    }

    #[test]
    fn test_build_http_request_body_with_speaking_rate() {
        let mut config = create_test_config();
        config.speaking_rate = Some(1.5);

        let azure_config = AzureTTSConfig::from_base(config.clone());
        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Fast speech");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // Verify prosody element with rate
        assert!(body_str.contains("<prosody rate=\"150%\">"));
        assert!(body_str.contains("Fast speech"));
        assert!(body_str.contains("</prosody>"));
    }

    // WIRE-LEVEL emotion test: the `mstts:express-as style` attribute must appear in the
    // SERIALIZED HTTP request body (the actual bytes sent to cognitiveservices/v1), not just
    // in a config struct field. This is the bug class the last review caught — a feature that
    // is stored but never reaches the wire. Confirmed against the Azure Speech SSML reference
    // (speech-synthesis-markup-voice, doc ms.date 2026-01-30): the v1 REST synth endpoint
    // consumes SSML, and emotion is the `<mstts:express-as style="...">` element.
    #[test]
    fn test_build_http_request_body_contains_express_as_emotion() {
        let config = create_test_config();
        let mut azure_config = AzureTTSConfig::from_base(config.clone());
        azure_config.emotion = Some("cheerful".to_string());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Great news");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // The express-as style attribute is on the wire.
        assert!(
            body_str.contains("<mstts:express-as style=\"cheerful\">"),
            "express-as missing from request body: {body_str}"
        );
        assert!(body_str.contains("</mstts:express-as>"));
        // Namespace declared so the mstts element is valid SSML.
        assert!(
            body_str.contains("xmlns:mstts="),
            "mstts namespace missing from request body: {body_str}"
        );
        assert!(body_str.contains("Great news"));
    }

    // Wire-level negative: no emotion configured → no express-as on the wire.
    #[test]
    fn test_build_http_request_body_no_emotion_omits_express_as() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Plain");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        assert!(!body_str.contains("mstts:express-as"));
        assert!(!body_str.contains("xmlns:mstts"));
    }

    // Wire-level end-to-end: a StandardTTSConfig with features.emotion set produces a request
    // body carrying express-as through the full standardized dispatch
    // (from_standard → request builder → serialized HTTP body).
    #[test]
    fn from_standard_emotion_reaches_request_body() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                emotion: Some("sad".into()),
                ..Default::default()
            },
            extras: ProviderExtras(serde_json::Map::new()),
        };
        let tts = AzureTTS::from_standard(&std).unwrap();
        assert_eq!(tts.azure_config().emotion.as_deref(), Some("sad"));

        let client = reqwest::Client::new();
        let request_builder = tts.request_builder.build_http_request(&client, "Oh no");
        let request = request_builder.build().unwrap();
        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        assert!(
            body_str.contains("<mstts:express-as style=\"sad\">"),
            "emotion not on the wire via from_standard: {body_str}"
        );
        assert!(body_str.contains("xmlns:mstts="));
    }

    // WIRE-LEVEL: deploymentId (extras) must reach the request URL as `?deploymentId=...`. The
    // custom-voice deployment is selected by this query param on the cognitiveservices/v1 endpoint.
    #[test]
    fn from_standard_deployment_id_reaches_request_url() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("deploymentId".into(), serde_json::json!("dep-abc-123"));
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let tts = AzureTTS::from_standard(&std).unwrap();
        assert_eq!(
            tts.azure_config().deployment_id.as_deref(),
            Some("dep-abc-123")
        );

        let client = reqwest::Client::new();
        let request = tts
            .request_builder
            .build_http_request(&client, "Hi")
            .build()
            .unwrap();
        assert_eq!(
            request.url().query(),
            Some("deploymentId=dep-abc-123"),
            "deploymentId not on the wire: {}",
            request.url()
        );
    }

    // WIRE-LEVEL: an Entra/Bearer `auth_token` (extras) must replace the subscription-key header
    // with `Authorization: Bearer <token>` on the actual request.
    #[test]
    fn from_standard_auth_token_sets_bearer_header() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("auth_token".into(), serde_json::json!("eyJ.token.val"));
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let tts = AzureTTS::from_standard(&std).unwrap();

        let client = reqwest::Client::new();
        let request = tts
            .request_builder
            .build_http_request(&client, "Hi")
            .build()
            .unwrap();
        let auth = request.headers().get("authorization").unwrap();
        assert_eq!(auth.to_str().unwrap(), "Bearer eyJ.token.val");
        // Subscription-key header must NOT be present when bearer auth is used.
        assert!(
            request
                .headers()
                .get(AZURE_SUBSCRIPTION_KEY_HEADER)
                .is_none()
        );
    }

    // Negative: without auth_token, the subscription key header is used (default credential path).
    #[test]
    fn no_auth_token_uses_subscription_key() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());
        let builder = AzureRequestBuilder::new(config, azure_config);
        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Hi").build().unwrap();
        assert_eq!(
            request
                .headers()
                .get(AZURE_SUBSCRIPTION_KEY_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "test-subscription-key"
        );
        assert!(request.headers().get("authorization").is_none());
    }

    // WIRE-LEVEL end-to-end: typed pitch/volume + an extras SSML knob reach the serialized request
    // body via the full standardized dispatch (from_standard → request builder → HTTP body bytes).
    #[test]
    fn from_standard_ssml_options_reach_request_body() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("emphasis".into(), serde_json::json!("strong"));
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                pitch: Some(-3.0),
                volume: Some(80.0),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = AzureTTS::from_standard(&std).unwrap();

        let client = reqwest::Client::new();
        let request = tts
            .request_builder
            .build_http_request(&client, "Hi")
            .build()
            .unwrap();
        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();
        assert!(
            body_str.contains("pitch=\"-3%\""),
            "pitch not on wire: {body_str}"
        );
        assert!(
            body_str.contains("volume=\"80\""),
            "volume not on wire: {body_str}"
        );
        assert!(
            body_str.contains("<emphasis level=\"strong\">"),
            "emphasis not on wire: {body_str}"
        );
    }

    // Cache-hash collision guards for the audio-changing SSML knobs (Review S1).
    #[test]
    fn config_hash_differs_on_ssml_options_and_deployment() {
        let config = create_test_config();
        let base = AzureTTSConfig::from_base(config.clone());
        let base_hash = compute_azure_tts_config_hash(&config, &base);

        let mut pitch_cfg = base.clone();
        pitch_cfg.ssml_options.pitch = Some("+4%".into());
        assert_ne!(
            base_hash,
            compute_azure_tts_config_hash(&config, &pitch_cfg)
        );

        let mut role_cfg = base.clone();
        role_cfg.ssml_options.role = Some("Boy".into());
        assert_ne!(base_hash, compute_azure_tts_config_hash(&config, &role_cfg));

        let mut dep_cfg = base.clone();
        dep_cfg.deployment_id = Some("dep-9".into());
        assert_ne!(base_hash, compute_azure_tts_config_hash(&config, &dep_cfg));

        let mut lang_cfg = base.clone();
        lang_cfg.language_override = Some("de-DE".into());
        assert_ne!(base_hash, compute_azure_tts_config_hash(&config, &lang_cfg));
    }

    // auth_token is a header-only credential and must NOT change the cache key (no audio change).
    #[test]
    fn config_hash_ignores_auth_token() {
        let config = create_test_config();
        let base = AzureTTSConfig::from_base(config.clone());
        let mut tok_cfg = base.clone();
        tok_cfg.auth_token = Some("secret-token".into());
        assert_eq!(
            compute_azure_tts_config_hash(&config, &base),
            compute_azure_tts_config_hash(&config, &tok_cfg)
        );
    }

    #[test]
    fn test_build_http_request_body_escapes_xml() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Hello <user> & welcome!");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // Verify XML escaping
        assert!(body_str.contains("Hello &lt;user&gt; &amp; welcome!"));
        assert!(!body_str.contains("Hello <user> & welcome!"));
    }

    #[test]
    fn test_build_http_request_different_voice() {
        let mut config = create_test_config();
        config.voice_id = Some("de-DE-KatjaNeural".to_string());

        let azure_config = AzureTTSConfig::from_base(config.clone());
        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "Guten Tag");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // Verify German voice and language
        assert!(body_str.contains("xml:lang='de-DE'"));
        assert!(body_str.contains("<voice name='de-DE-KatjaNeural'>"));
    }

    // =========================================================================
    // Config Hash Tests
    // =========================================================================

    #[test]
    fn test_compute_azure_tts_config_hash() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let hash = compute_azure_tts_config_hash(&config, &azure_config);

        // Hash should be a 32-char hex string
        assert_eq!(hash.len(), 32);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));

        // Same config should produce same hash
        let hash2 = compute_azure_tts_config_hash(&config, &azure_config);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_compute_azure_tts_config_hash_different_configs() {
        let config1 = create_test_config();
        let azure_config1 = AzureTTSConfig::from_base(config1.clone());

        let mut config2 = create_test_config();
        config2.voice_id = Some("en-US-AriaNeural".to_string());
        let azure_config2 = AzureTTSConfig::from_base(config2.clone());

        let hash1 = compute_azure_tts_config_hash(&config1, &azure_config1);
        let hash2 = compute_azure_tts_config_hash(&config2, &azure_config2);

        // Different configs should produce different hashes
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_compute_azure_tts_config_hash_different_regions() {
        let config = create_test_config();

        let azure_config1 = AzureTTSConfig::with_region(config.clone(), AzureRegion::EastUS);
        let azure_config2 = AzureTTSConfig::with_region(config.clone(), AzureRegion::WestEurope);

        let hash1 = compute_azure_tts_config_hash(&config, &azure_config1);
        let hash2 = compute_azure_tts_config_hash(&config, &azure_config2);

        // Different regions should produce different hashes
        assert_ne!(hash1, hash2);
    }

    // =========================================================================
    // AzureTTS Provider Tests
    // =========================================================================

    #[test]
    fn test_azure_tts_creation() {
        let config = create_test_config();
        let tts = AzureTTS::new(config).unwrap();

        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    // W1 keystone: a StandardTTSConfig advanced feature Azure supports (the `ssml` → `use_ssml`
    // toggle, plus the non-standard `region` extra) reaches the provider's resolved
    // `azure_config` through the provider struct's `from_standard`, mirroring
    // `DeepgramTTS::from_standard`.
    #[test]
    fn from_standard_reaches_provider_config() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("region".into(), serde_json::json!("westeurope"));
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures {
                // from_base defaults use_ssml to true; flip it to prove the feature is honored.
                ssml: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = AzureTTS::from_standard(&std).unwrap();
        assert!(!tts.azure_config().use_ssml);
        assert_eq!(tts.azure_config().region, AzureRegion::WestEurope);
        assert_eq!(tts.azure_config().base.api_key, "test-subscription-key");
    }

    #[test]
    fn from_standard_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let std = crate::core::tts::standard::StandardTTSConfig::from_base(create_test_config())
            .with_endpoint_override("http://127.0.0.1:9000");

        match AzureTTS::from_standard(&std) {
            Ok(_) => panic!("Azure provider construction must reject unsafe endpoint_override"),
            Err(err) => assert!(err.to_string().contains("SSRF protection")),
        }
    }

    #[test]
    fn from_standard_rejects_ssrf_ssml_provider_urls() {
        let _env = crate::core::net::ssrf_env_lock();
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};

        let mut lexicon_extras = serde_json::Map::new();
        lexicon_extras.insert(
            "lexicon_uri".into(),
            serde_json::json!("http://127.0.0.1:9000/lex.xml"),
        );
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures::default(),
            extras: ProviderExtras(lexicon_extras),
        };
        match AzureTTS::from_standard(&std) {
            Ok(_) => panic!("Azure provider construction must reject unsafe lexicon_uri"),
            Err(err) => assert!(err.to_string().contains("SSRF protection")),
        }

        let mut bg_extras = serde_json::Map::new();
        bg_extras.insert(
            "background_audio_src".into(),
            serde_json::json!("file:///tmp/bg.wav"),
        );
        let std = StandardTTSConfig {
            base: create_test_config(),
            features: TtsFeatures::default(),
            extras: ProviderExtras(bg_extras),
        };
        match AzureTTS::from_standard(&std) {
            Ok(_) => panic!("Azure provider construction must reject unsafe background_audio_src"),
            Err(err) => assert!(err.to_string().contains("URL scheme")),
        }
    }

    #[test]
    fn test_azure_tts_with_region() {
        let config = create_test_config();
        let tts = AzureTTS::with_region(config, AzureRegion::WestEurope).unwrap();

        assert_eq!(tts.azure_config().region, AzureRegion::WestEurope);
        assert_eq!(
            tts.azure_config().build_tts_url(),
            "https://westeurope.tts.speech.microsoft.com/cognitiveservices/v1"
        );
    }

    #[test]
    fn azure_tts_default_does_not_depend_on_result_unwrap() {
        let tts = AzureTTS::default();

        assert!(!tts.is_ready());
        assert_eq!(tts.azure_config().region, AzureRegion::EastUS);
        assert_eq!(tts.azure_config().base.api_key, "");
    }

    #[test]
    fn test_azure_tts_get_provider_info() {
        let config = create_test_config();
        let tts = AzureTTS::new(config).unwrap();

        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "azure");
        assert_eq!(info["version"], "1.0.0");
        assert_eq!(info["api_type"], "HTTP REST");
        assert_eq!(info["region"], "eastus");
        assert!(
            info["endpoint"]
                .as_str()
                .unwrap()
                .contains("tts.speech.microsoft.com")
        );
        assert!(info["supported_formats"].is_array());
        assert!(info["supported_sample_rates"].is_array());
    }

    #[test]
    fn test_azure_request_builder_clone() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        // Clone should work since it derives Clone
        let cloned = builder.clone();

        assert_eq!(cloned.config.api_key, builder.config.api_key);
        assert_eq!(cloned.azure_config.region, builder.azure_config.region);
    }

    // =========================================================================
    // Edge Cases
    // =========================================================================

    #[test]
    fn test_build_http_request_empty_text() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let request_builder = builder.build_http_request(&client, "");
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // Empty text should still produce valid SSML
        assert!(body_str.contains("<speak"));
        assert!(body_str.contains("</speak>"));
    }

    #[test]
    fn test_build_http_request_unicode_text() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let text = "Hello, 世界! Привет мир! 🌍";
        let request_builder = builder.build_http_request(&client, text);
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // Unicode text should be preserved (no escaping needed for Unicode)
        assert!(body_str.contains("世界"));
        assert!(body_str.contains("Привет"));
        assert!(body_str.contains("🌍"));
    }

    #[test]
    fn test_build_http_request_all_xml_special_chars() {
        let config = create_test_config();
        let azure_config = AzureTTSConfig::from_base(config.clone());

        let builder = AzureRequestBuilder::new(config, azure_config);

        let client = reqwest::Client::new();
        let text = "Test <tag> & \"quote\" 'apos' end";
        let request_builder = builder.build_http_request(&client, text);
        let request = request_builder.build().unwrap();

        let body = request.body().unwrap().as_bytes().unwrap();
        let body_str = std::str::from_utf8(body).unwrap();

        // All XML special characters should be escaped
        assert!(body_str.contains("&lt;tag&gt;"));
        assert!(body_str.contains("&amp;"));
        assert!(body_str.contains("&quot;quote&quot;"));
        assert!(body_str.contains("&apos;apos&apos;"));
    }
}
