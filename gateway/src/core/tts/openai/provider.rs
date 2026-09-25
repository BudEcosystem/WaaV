//! OpenAI TTS provider implementation.
//!
//! This module provides the OpenAI TTS provider that implements the `BaseTTS` trait
//! using OpenAI's text-to-speech API.
//!
//! # API Reference
//!
//! - Endpoint: `POST https://api.openai.com/v1/audio/speech`
//! - Models: tts-1, tts-1-hd, gpt-4o-mini-tts
//! - Voices: alloy, ash, ballad, coral, echo, fable, onyx, nova, sage, shimmer, verse
//! - Output: mp3, opus, aac, flac, wav, pcm (24kHz)
//! - Speed: 0.25 to 4.0

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use xxhash_rust::xxh3::xxh3_128;

use super::config::{
    AudioOutputFormat, OpenAITTSModel, OpenAIVoice, openai_tts_model_id,
    openai_tts_model_supports_instructions, openai_tts_voice_id,
};
use crate::core::tts::base::{
    AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSError, TTSResult,
};
use crate::core::tts::provider::{PronunciationReplacer, TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

/// OpenAI TTS API endpoint (default).
pub const OPENAI_TTS_URL: &str = "https://api.openai.com/v1/audio/speech";
const OPENAI_BASE_URL_ENV: &str = "OPENAI_BASE_URL";

/// Resolve the OpenAI TTS endpoint, honoring the standard `OPENAI_BASE_URL` override (as the
/// OpenAI SDKs do). This lets the provider target OpenAI-compatible speech endpoints (Azure
/// OpenAI, a proxy, or a local server) and enables credential-free contract/e2e testing.
#[inline]
pub fn openai_tts_url() -> String {
    resolve_openai_tts_url().unwrap_or_else(|e| {
        tracing::error!(error = %e, "invalid OpenAI TTS endpoint override; using production endpoint for display-only URL");
        OPENAI_TTS_URL.to_string()
    })
}

fn openai_tts_url_from_base(source: &str, base: &str) -> Result<Option<String>, String> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Ok(None);
    }
    crate::core::net::validate_url_for_ssrf(base, &["http", "https"])
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))?;
    Ok(Some(format!("{base}/v1/audio/speech")))
}

fn resolve_openai_tts_url() -> Result<String, String> {
    match std::env::var(OPENAI_BASE_URL_ENV) {
        Ok(base) => Ok(openai_tts_url_from_base(OPENAI_BASE_URL_ENV, &base)?
            .unwrap_or_else(|| OPENAI_TTS_URL.to_string())),
        Err(std::env::VarError::NotPresent) => Ok(OPENAI_TTS_URL.to_string()),
        Err(std::env::VarError::NotUnicode(_)) => {
            Err(format!("{OPENAI_BASE_URL_ENV} must be valid UTF-8"))
        }
    }
}

// =============================================================================
// Request Builder
// =============================================================================

/// OpenAI-specific TTS request builder
#[derive(Clone)]
struct OpenAIRequestBuilder {
    /// Base TTS configuration
    config: TTSConfig,
    /// Validated endpoint URL
    endpoint_url: String,
    /// The `model` sent on the wire — the configured id verbatim (see `openai_tts_model_id`).
    model: String,
    /// The `voice` sent on the wire — the configured voice verbatim (see `openai_tts_voice_id`).
    voice: String,
    /// Parsed audio output format
    response_format: AudioOutputFormat,
    /// Speaking speed (0.25 to 4.0)
    speed: f32,
    /// Delivery/acting instructions (sent to every model except the tts-1 family).
    instructions: Option<String>,
    /// Pronunciation replacer
    pronunciation_replacer: Option<PronunciationReplacer>,
}

impl TTSRequestBuilder for OpenAIRequestBuilder {
    /// Build the OpenAI-specific HTTP request
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Build the request body
        let mut body = json!({
            "model": self.model,
            "input": text,
            "voice": self.voice,
            "response_format": self.response_format.as_str(),
        });

        // Add speed if not default (1.0)
        if (self.speed - 1.0).abs() > 0.001 {
            body["speed"] = json!(self.speed);
        }

        // `instructions` is documented as unsupported on tts-1 / tts-1-hd only (sending it there
        // would be rejected), so it is gated on "not a tts-1 family model" — which covers
        // gpt-4o-mini-tts, its dated snapshots and newer models — rather than on the exact
        // gpt-4o-mini-tts enum variant, which dropped it for every snapshot. (Review S3.)
        if let Some(instr) = &self.instructions
            && openai_tts_model_supports_instructions(&self.model)
        {
            body["instructions"] = json!(instr);
        }

        client
            .post(&self.endpoint_url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
    }

    /// Get the configuration
    fn get_config(&self) -> &TTSConfig {
        &self.config
    }

    /// Get precompiled pronunciation replacer
    fn get_pronunciation_replacer(&self) -> Option<&PronunciationReplacer> {
        self.pronunciation_replacer.as_ref()
    }
}

// =============================================================================
// Config Hash for Caching
// =============================================================================

/// Compute a hash of the TTS configuration for caching purposes.
///
/// Keyed on the model/voice actually SENT (verbatim ids), so two distinct models no longer
/// collapse onto one key, and on the `instructions` when the model receives them — they change
/// the produced audio, so omitting them served one request's audio for another's.
fn compute_tts_config_hash(
    config: &TTSConfig,
    model: &str,
    voice: &str,
    instructions: Option<&str>,
) -> String {
    let mut s = String::new();
    s.push_str("openai");
    s.push('|');
    s.push_str(model);
    s.push('|');
    s.push_str(voice);
    s.push('|');
    if let Some(instr) = instructions.filter(|_| openai_tts_model_supports_instructions(model)) {
        s.push_str(instr);
    }
    s.push('|');
    s.push_str(config.audio_format.as_deref().unwrap_or("mp3"));
    s.push('|');
    if let Some(sr) = config.sample_rate {
        s.push_str(&sr.to_string());
    }
    s.push('|');
    if let Some(rate) = config.speaking_rate {
        s.push_str(&format!("{rate:.3}"));
    }
    let hash = xxh3_128(s.as_bytes());
    format!("{hash:032x}")
}

// =============================================================================
// OpenAI TTS Provider
// =============================================================================

/// OpenAI TTS provider implementation using the OpenAI Audio Speech API
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::{BaseTTS, TTSConfig, OpenAITTS, AudioCallback, AudioData};
/// use std::sync::Arc;
///
/// struct MyCallback;
/// impl AudioCallback for MyCallback {
///     fn on_audio(&self, audio: AudioData) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
///         Box::pin(async { println!("Received {} bytes", audio.data.len()); })
///     }
///     fn on_error(&self, _: TTSError) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
///         Box::pin(async {})
///     }
///     fn on_complete(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
///         Box::pin(async {})
///     }
/// }
///
/// #[tokio::main]
/// async fn main() {
///     let config = TTSConfig {
///         api_key: "sk-...".to_string(),
///         voice_id: Some("nova".to_string()),
///         model: "tts-1".to_string(),
///         ..Default::default()
///     };
///
///     let mut tts = OpenAITTS::new(config).unwrap();
///     tts.connect().await.unwrap();
///     tts.on_audio(Arc::new(MyCallback)).unwrap();
///     tts.speak("Hello, world!", true).await.unwrap();
/// }
/// ```
pub struct OpenAITTS {
    /// Generic HTTP-based TTS provider
    provider: TTSProvider,
    /// Request builder with OpenAI-specific configuration
    request_builder: OpenAIRequestBuilder,
    /// Precomputed config hash for caching
    config_hash: String,
}

impl OpenAITTS {
    /// Create a new OpenAI TTS instance
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        let endpoint_url = resolve_openai_tts_url().map_err(TTSError::InvalidConfiguration)?;
        Ok(Self::build(config, endpoint_url))
    }

    fn build(config: TTSConfig, endpoint_url: String) -> Self {
        // Model and voice are sent VERBATIM. OpenAI requires both, so a default applies only when
        // one is EMPTY; an id the enums do not list (a dated snapshot, a new voice) is NOT
        // rewritten to tts-1 / alloy — that silently replaced the caller's choice.
        let model = openai_tts_model_id(&config.model);
        let voice = openai_tts_voice_id(config.voice_id.as_deref());

        // Parse audio format
        let response_format = if let Some(ref format) = config.audio_format {
            AudioOutputFormat::from_str_or_default(format)
        } else {
            // Default to PCM for consistency with other providers in WaaV
            AudioOutputFormat::Pcm
        };

        // Parse speed (default 1.0, clamp to valid range)
        let speed = config.speaking_rate.unwrap_or(1.0).clamp(0.25, 4.0);

        // Create pronunciation replacer if needed
        let pronunciation_replacer = if !config.pronunciations.is_empty() {
            Some(PronunciationReplacer::new(&config.pronunciations))
        } else {
            None
        };

        let config_hash = compute_tts_config_hash(&config, &model, &voice, None);

        let request_builder = OpenAIRequestBuilder {
            config,
            endpoint_url,
            model,
            voice,
            response_format,
            speed,
            instructions: None,
            pronunciation_replacer,
        };

        Self {
            provider: TTSProvider::new(),
            request_builder,
            config_hash,
        }
    }

    /// Build from the standardized config (W1 keystone for TTS — uniform entry point).
    ///
    /// OpenAI's speech API exposes a narrow control surface (model, voice, format, speed, plus
    /// `instructions` on gpt-4o-mini-tts). `speed` folds into `speaking_rate`; `instructions` maps
    /// to the gpt-4o-mini-tts delivery/acting field (ignored on tts-1/tts-1-hd). The remaining
    /// features (ElevenLabs voice settings, pitch/volume, emotion, ssml, word_timestamps,
    /// streaming, seed, language, sample_rate) have no OpenAI parameter and are not mapped.
    ///
    /// [`TtsFeatures`]: crate::core::tts::standard::TtsFeatures
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let f = &std.features;
        let mut base = std.base.clone();
        if let Some(speed) = f.speed {
            base.speaking_rate = Some(speed);
        }
        let mut tts = OpenAITTS::new(base)?;
        // Delivery instructions (Review S3); gated to the models that accept them in the body.
        // They change the audio, so the cache key is recomputed to include them.
        if let Some(instr) = &f.instructions {
            tts.request_builder.instructions = Some(instr.clone());
            tts.config_hash = compute_tts_config_hash(
                &tts.request_builder.config,
                &tts.request_builder.model,
                &tts.request_builder.voice,
                Some(instr),
            );
        }
        Ok(tts)
    }

    /// Get the configured model, classified into the known-model enum.
    ///
    /// Classification only: an id the enum does not list classifies as `Tts1` but is still
    /// sent verbatim — [`Self::model_id`] is the value on the wire.
    pub fn model(&self) -> OpenAITTSModel {
        OpenAITTSModel::from_str_or_default(&self.request_builder.model)
    }

    /// The `model` sent to OpenAI (verbatim; `tts-1` only when none was configured).
    pub fn model_id(&self) -> &str {
        &self.request_builder.model
    }

    /// Get the configured voice, classified into the known-voice enum.
    ///
    /// Classification only: a voice the enum does not list classifies as `Alloy` but is still
    /// sent verbatim — [`Self::voice_id`] is the value on the wire.
    pub fn voice(&self) -> OpenAIVoice {
        OpenAIVoice::from_str_or_default(&self.request_builder.voice)
    }

    /// The `voice` sent to OpenAI (verbatim; `alloy` only when none was configured).
    pub fn voice_id(&self) -> &str {
        &self.request_builder.voice
    }

    /// Get the configured output format
    pub fn output_format(&self) -> AudioOutputFormat {
        self.request_builder.response_format
    }
}

impl Default for OpenAITTS {
    fn default() -> Self {
        let config = TTSConfig::default();
        match Self::new(config.clone()) {
            Ok(tts) => tts,
            Err(_) => Self::build(config, OPENAI_TTS_URL.to_string()),
        }
    }
}

#[async_trait]
impl BaseTTS for OpenAITTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        OpenAITTS::new(config)
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        self.provider
            .generic_connect_with_config(
                &self.request_builder.endpoint_url,
                &self.request_builder.config,
            )
            .await
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
            tracing::info!("OpenAI TTS not ready, attempting to connect...");
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
            "provider": "openai",
            "version": "1.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "supported_formats": ["mp3", "opus", "aac", "flac", "wav", "pcm"],
            "default_sample_rate": 24000,
            "supported_models": [
                "tts-1",
                "tts-1-hd",
                "gpt-4o-mini-tts"
            ],
            "supported_voices": [
                "alloy", "ash", "ballad", "coral", "echo",
                "fable", "onyx", "nova", "sage", "shimmer", "verse"
            ],
            "speed_range": {
                "min": 0.25,
                "max": 4.0,
                "default": 1.0
            },
            "endpoint": OPENAI_TTS_URL,
            "documentation": "https://platform.openai.com/docs/api-reference/audio/createSpeech",
        })
    }

    async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (TTS): OpenAI exposes a narrow control surface, so `from_standard` maps only
    // `speed` (→ speaking_rate → builder speed); every other feature is a capability gap. This
    // asserts the mapped feature reaches the right field and the base config carries through.
    #[tokio::test]
    async fn from_standard_maps_speed() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "openai".into(),
                api_key: "test_key".into(),
                voice_id: Some("nova".into()),
                model: "tts-1-hd".into(),
                speaking_rate: Some(1.0),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                // Capability gaps below are intentionally ignored by OpenAI.
                stability: Some(0.7),
                instructions: Some("speak cheerfully".into()),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = OpenAITTS::from_standard(&std).unwrap();
        // speed (1.5) folded into speaking_rate and applied to the builder.
        assert!((tts.request_builder.speed - 1.5).abs() < 0.001);
        // base carried through (api key, voice, model).
        assert_eq!(tts.request_builder.config.api_key, "test_key");
        assert_eq!(tts.voice(), OpenAIVoice::Nova);
        assert_eq!(tts.model(), OpenAITTSModel::Tts1Hd);
    }

    #[tokio::test]
    async fn test_openai_tts_creation() {
        let config = TTSConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            voice_id: Some("nova".to_string()),
            model: "tts-1-hd".to_string(),
            audio_format: Some("pcm".to_string()),
            speaking_rate: Some(1.2),
            ..Default::default()
        };

        let tts = OpenAITTS::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert_eq!(tts.model(), OpenAITTSModel::Tts1Hd);
        assert_eq!(tts.voice(), OpenAIVoice::Nova);
        assert_eq!(tts.output_format(), AudioOutputFormat::Pcm);
    }

    #[tokio::test]
    async fn test_openai_tts_default_values() {
        // OpenAI requires `model` and `voice`: the defaults apply only when none is configured.
        let config = TTSConfig {
            provider: "openai".to_string(),
            api_key: "test_key".to_string(),
            voice_id: None,
            ..Default::default()
        };

        let tts = OpenAITTS::new(config).unwrap();
        assert_eq!(tts.model(), OpenAITTSModel::Tts1);
        assert_eq!(tts.model_id(), "tts-1");
        assert_eq!(tts.voice(), OpenAIVoice::Alloy);
        assert_eq!(tts.voice_id(), "alloy");
        assert_eq!(tts.output_format(), AudioOutputFormat::Pcm);
    }

    /// Serialize the body a built OpenAI TTS would POST.
    fn wire_body(tts: &OpenAITTS) -> serde_json::Value {
        let client = reqwest::Client::new();
        let built = tts
            .request_builder
            .build_http_request(&client, "Hello")
            .build()
            .unwrap();
        serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap()
    }

    // Real OpenAI model ids the enum does not list must reach the wire VERBATIM — previously
    // every one of them was silently sent as `tts-1`.
    #[test]
    fn unknown_model_is_sent_verbatim() {
        for id in ["gpt-4o-mini-tts-2025-12-15", "tts-1-1106", "tts-1-hd-1106"] {
            let tts = OpenAITTS::build(
                TTSConfig {
                    api_key: "k".into(),
                    model: id.into(),
                    voice_id: Some("nova".into()),
                    ..Default::default()
                },
                OPENAI_TTS_URL.to_string(),
            );
            assert_eq!(tts.model_id(), id);
            let body = wire_body(&tts);
            assert_eq!(body["model"], id, "model rewritten: {body}");
        }
    }

    // A voice the enum does not list must reach the wire VERBATIM — previously it silently
    // became `alloy`.
    #[test]
    fn unknown_voice_is_sent_verbatim() {
        let tts = OpenAITTS::build(
            TTSConfig {
                api_key: "k".into(),
                model: "gpt-4o-mini-tts".into(),
                voice_id: Some("some-new-voice".into()),
                ..Default::default()
            },
            OPENAI_TTS_URL.to_string(),
        );
        assert_eq!(tts.voice_id(), "some-new-voice");
        let body = wire_body(&tts);
        assert_eq!(body["voice"], "some-new-voice", "voice replaced: {body}");

        // Empty model/voice → OpenAI's required fields get the defaults.
        let tts = OpenAITTS::build(
            TTSConfig {
                api_key: "k".into(),
                model: String::new(),
                voice_id: Some(String::new()),
                ..Default::default()
            },
            OPENAI_TTS_URL.to_string(),
        );
        let body = wire_body(&tts);
        assert_eq!(body["model"], "tts-1");
        assert_eq!(body["voice"], "alloy");
    }

    // `instructions` is unsupported on tts-1/tts-1-hd only: it must reach gpt-4o-mini-tts
    // snapshots (previously dropped because they were not the exact enum variant) and must not
    // reach the tts-1 family.
    #[test]
    fn instructions_gated_on_tts1_family_not_enum_variant() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        // `from_standard` resolves OPENAI_BASE_URL; serialize against tests that mutate it.
        let _env = crate::core::net::ssrf_env_lock();
        let with_model = |model: &str| {
            let std = StandardTTSConfig {
                base: TTSConfig {
                    api_key: "k".into(),
                    model: model.into(),
                    voice_id: Some("coral".into()),
                    ..Default::default()
                },
                features: TtsFeatures {
                    instructions: Some("speak cheerfully".into()),
                    ..Default::default()
                },
                extras: Default::default(),
            };
            OpenAITTS::from_standard(&std).unwrap()
        };
        for model in ["gpt-4o-mini-tts", "gpt-4o-mini-tts-2025-12-15"] {
            let body = wire_body(&with_model(model));
            assert_eq!(body["instructions"], "speak cheerfully", "{model}: {body}");
        }
        for model in ["tts-1", "tts-1-hd", "tts-1-hd-1106"] {
            let body = wire_body(&with_model(model));
            assert!(body.get("instructions").is_none(), "{model}: {body}");
        }
    }

    // Instructions change the audio, so they must be in the cache key (for models that get them).
    #[test]
    fn instructions_are_in_the_cache_key() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        // `from_standard` resolves OPENAI_BASE_URL; serialize against tests that mutate it.
        let _env = crate::core::net::ssrf_env_lock();
        let hash = |instructions: Option<&str>| {
            let std = StandardTTSConfig {
                base: TTSConfig {
                    api_key: "k".into(),
                    model: "gpt-4o-mini-tts".into(),
                    voice_id: Some("coral".into()),
                    ..Default::default()
                },
                features: TtsFeatures {
                    instructions: instructions.map(String::from),
                    ..Default::default()
                },
                extras: Default::default(),
            };
            OpenAITTS::from_standard(&std).unwrap().config_hash
        };
        assert_ne!(hash(Some("speak cheerfully")), hash(Some("whisper")));
        assert_ne!(hash(Some("speak cheerfully")), hash(None));
    }

    #[tokio::test]
    async fn test_http_request_building() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("nova".to_string()),
            model: "tts-1".to_string(),
            audio_format: Some("mp3".to_string()),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let builder = OpenAIRequestBuilder {
            config,
            endpoint_url: OPENAI_TTS_URL.to_string(),
            model: "tts-1".to_string(),
            voice: "nova".to_string(),
            response_format: AudioOutputFormat::Mp3,
            speed: 1.5,
            instructions: None,
            pronunciation_replacer: None,
        };

        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Hello world");
        let built = request.build().unwrap();

        // Verify URL
        assert_eq!(built.url().as_str(), OPENAI_TTS_URL);

        // Verify headers
        let auth_header = built.headers().get("Authorization").unwrap();
        assert_eq!(auth_header, "Bearer test_key");

        let content_type = built.headers().get("Content-Type").unwrap();
        assert_eq!(content_type, "application/json");
    }

    #[test]
    fn test_openai_tts_url_override_is_ssrf_checked() {
        let _env = crate::core::net::ssrf_env_lock();
        assert_eq!(
            openai_tts_url_from_base("OPENAI_BASE_URL", "https://openai-compatible.invalid/")
                .unwrap()
                .unwrap(),
            "https://openai-compatible.invalid/v1/audio/speech"
        );

        let msg = openai_tts_url_from_base("OPENAI_BASE_URL", "http://127.0.0.1:8089/")
            .expect_err("loopback endpoint override must be rejected")
            .to_string();
        assert!(
            msg.contains("SSRF protection"),
            "error names SSRF guard: {msg}"
        );

        let msg = openai_tts_url_from_base("OPENAI_BASE_URL", "file:///tmp/socket")
            .expect_err("non-HTTP endpoint override must be rejected")
            .to_string();
        assert!(msg.contains("scheme"), "error names scheme contract: {msg}");
    }

    #[test]
    fn openai_default_does_not_panic_on_invalid_base_url() {
        let _guard = crate::core::net::test_env_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let previous = std::env::var_os(OPENAI_BASE_URL_ENV);

        // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
        unsafe { std::env::set_var(OPENAI_BASE_URL_ENV, "file:///tmp/socket") };
        let result = std::panic::catch_unwind(OpenAITTS::default);

        match previous {
            Some(value) => {
                // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
                unsafe { std::env::set_var(OPENAI_BASE_URL_ENV, value) };
            }
            None => {
                // SAFETY: test-only env mutation, serialized by core::net::test_env_lock.
                unsafe { std::env::remove_var(OPENAI_BASE_URL_ENV) };
            }
        }

        let tts = result.expect("OpenAITTS::default must not panic on invalid OPENAI_BASE_URL");
        assert!(!tts.is_ready());
        assert_eq!(tts.request_builder.endpoint_url, OPENAI_TTS_URL);
    }

    #[tokio::test]
    async fn test_speed_clamping() {
        // Test speed below minimum
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(0.1), // Below 0.25 minimum
            ..Default::default()
        };
        let tts = OpenAITTS::new(config).unwrap();
        assert!((tts.request_builder.speed - 0.25).abs() < 0.001);

        // Test speed above maximum
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(5.0), // Above 4.0 maximum
            ..Default::default()
        };
        let tts = OpenAITTS::new(config).unwrap();
        assert!((tts.request_builder.speed - 4.0).abs() < 0.001);

        // Test speed within range
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(2.0),
            ..Default::default()
        };
        let tts = OpenAITTS::new(config).unwrap();
        assert!((tts.request_builder.speed - 2.0).abs() < 0.001);
    }

    #[test]
    fn test_provider_info() {
        let tts = OpenAITTS::default();
        let info = tts.get_provider_info();

        assert_eq!(info["provider"], "openai");
        assert!(
            info["supported_models"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("tts-1"))
        );
        assert!(
            info["supported_voices"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("nova"))
        );
    }

    #[test]
    fn test_config_hash_uniqueness() {
        let config1 = TTSConfig {
            api_key: "key".to_string(),
            model: "tts-1".to_string(),
            ..Default::default()
        };

        let config2 = TTSConfig {
            api_key: "key".to_string(),
            model: "tts-1-hd".to_string(),
            ..Default::default()
        };

        let hash1 = compute_tts_config_hash(&config1, "tts-1", "alloy", None);
        let hash2 = compute_tts_config_hash(&config2, "tts-1-hd", "alloy", None);

        assert_ne!(hash1, hash2);

        // Distinct verbatim ids no longer collapse onto one key (they were all `tts-1` before).
        let hash3 = compute_tts_config_hash(&config1, "gpt-4o-mini-tts-2025-12-15", "alloy", None);
        assert_ne!(hash1, hash3);
    }
}
