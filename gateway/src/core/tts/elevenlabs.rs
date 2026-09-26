use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::base::{AudioCallback, BaseTTS, ConnectionState, TTSConfig, TTSResult};
use super::provider::{TTSProvider, TTSRequestBuilder};
use crate::utils::req_manager::ReqManager;

/// Voice settings for ElevenLabs TTS
///
/// Every field is optional, and `Default` sets none of them. ElevenLabs documents
/// `voice_settings` as "overriding stored settings for the given voice": each voice carries its
/// own tuning, saved on the account. The old default sent `stability 0.5, similarity_boost 0.8,
/// style 0.0, use_speaker_boost false, speed 1.0` on every request, so every voice was re-tuned
/// to WaaV's numbers — which are not even ElevenLabs' own defaults (0.75 similarity, speaker boost
/// on) — whatever the account had saved. A field is sent only when the caller or the deployment
/// set it, and the whole object is left off when none is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VoiceSettings {
    /// Voice stability (0.0 to 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stability: Option<f32>,
    /// Similarity boost (0.0 to 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_boost: Option<f32>,
    /// Style strength (0.0 to 1.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<f32>,
    /// Use speaker boost
    #[serde(skip_serializing_if = "Option::is_none")]
    pub use_speaker_boost: Option<bool>,
    /// Speaking rate (0.25 to 4.0)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f32>,
}

impl VoiceSettings {
    /// Nothing set: the request carries no `voice_settings` and the voice's saved ones apply.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Build ElevenLabs voice settings from the standardized TTS config (W1 keystone — TTS analog
    /// of the STT migration). ElevenLabs' voice settings map cleanly onto the canonical voice
    /// features (`stability`, `similarity_boost`, `style`, `use_speaker_boost`, `speed`), which
    /// were previously unreachable through the flat factory. The base's `speaking_rate` seeds
    /// `speed` (preserving `ElevenLabsTTS::new`), then the explicit `speed` feature overrides it.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> Self {
        let f = &std.features;
        let mut settings = VoiceSettings {
            speed: std.base.speaking_rate,
            ..Default::default()
        };
        if let Some(s) = f.stability {
            settings.stability = Some(s);
        }
        if let Some(s) = f.similarity_boost {
            settings.similarity_boost = Some(s);
        }
        if let Some(s) = f.style {
            settings.style = Some(s);
        }
        if let Some(b) = f.use_speaker_boost {
            settings.use_speaker_boost = Some(b);
        }
        if let Some(s) = f.speed {
            settings.speed = Some(s);
        }
        settings
    }
}

pub const ELEVENLABS_TTS_URL: &str = "https://api.elevenlabs.io/v1/text-to-speech";

/// The voice used when nothing names one: George, the voice ElevenLabs' own quickstart uses.
///
/// Must be a voice EVERY account may use, which rules out the long-standing choice, Rachel
/// (`21m00Tcm4TlvDq8ikWAM`). ElevenLabs has since moved Rachel into the shared library, and a
/// free-tier key is refused her outright — `402 Free users cannot use library voices via the
/// API` — so a default meant to always work failed on exactly the accounts least likely to have
/// configured anything. George (and Sarah) synthesise on a free-tier key; verified live against
/// eleven_v3 on 2026-09-24.
pub const DEFAULT_VOICE_ID: &str = "JBFqnCBsd6RMkjVDRZzb";

/// The `output_format` ElevenLabs is sent for a requested WaaV format.
///
/// One function for the request builder and for anyone asking "can ElevenLabs produce this?" —
/// the OpenAI route refuses a format this maps to PCM for (other than `wav`/`pcm` themselves)
/// before spending a vendor call, instead of serving raw samples labelled as a codec.
///
/// ElevenLabs outputs mp3, pcm, ulaw, alaw and opus. `opus` maps to its native `opus_48000_64`;
/// before this it fell through to the PCM default, and `/v1/audio/speech` answered
/// `response_format: opus` with raw samples labelled `audio/opus`. `wav` is PCM here and is given
/// its RIFF header by the OpenAI route, which holds the whole clip. `aac` and `flac` have no
/// ElevenLabs equivalent and still map to PCM — that is the signal the route reads.
pub fn output_format_for(audio_format: Option<&str>, sample_rate: Option<u32>) -> String {
    let Some(format) = audio_format else {
        // Default to PCM 24kHz for consistency with the rest of the system
        return "pcm_24000".to_string();
    };
    match format {
        // ElevenLabs supports PCM at specific sample rates
        // Every PCM rate ElevenLabs offers. 8000 and 48000 were missing, so they fell to the
        // 24 kHz default while the audio was still LABELLED 8000/48000 — played at the label,
        // 3 s of speech became 8.9 s of noise. `new` now reports the rate actually requested.
        "linear16" | "pcm" => match sample_rate.unwrap_or(24000) {
            8000 => "pcm_8000".to_string(),
            16000 => "pcm_16000".to_string(),
            22050 => "pcm_22050".to_string(),
            24000 => "pcm_24000".to_string(),
            44100 => "pcm_44100".to_string(),
            48000 => "pcm_48000".to_string(),
            _ => "pcm_24000".to_string(), // Default to 24kHz
        },
        "mp3" => match sample_rate.unwrap_or(44100) {
            22050 => "mp3_22050_32".to_string(),
            44100 => "mp3_44100_128".to_string(),
            _ => "mp3_44100_128".to_string(),
        },
        "opus" => "opus_48000_64".to_string(),
        // G.711 at its only rate. `mulaw` and `alaw` fell to the unknown-alias branch below and
        // were requested as 24 kHz PCM, although ElevenLabs produces both.
        "ulaw" | "mulaw" => "ulaw_8000".to_string(),
        "alaw" => "alaw_8000".to_string(),
        other => {
            // If the caller already passed a canonical ElevenLabs format string
            // (e.g. "mp3_44100_128", "pcm_16000", "ulaw_8000", "opus_48000_64"),
            // honor it verbatim instead of silently forcing PCM — otherwise a valid
            // explicit selection (e.g. MP3, the only output allowed on lower tiers) was
            // being overridden to the Pro-tier-only `pcm_*` and rejected with HTTP 403.
            const KNOWN_PREFIXES: [&str; 5] = ["mp3_", "pcm_", "ulaw_", "alaw_", "opus_"];
            if KNOWN_PREFIXES.iter().any(|p| other.starts_with(p)) {
                other.to_string()
            } else {
                // Unknown short alias → default to PCM for downstream-pipeline compatibility.
                format!("pcm_{}", sample_rate.unwrap_or(24000))
            }
        }
    }
}

/// ElevenLabs-specific request builder
#[derive(Clone)]
struct ElevenLabsRequestBuilder {
    config: TTSConfig,
    voice_settings: VoiceSettings,
    /// Determinism seed → emitted in the request BODY as `seed` (integer 0..=4294967295).
    /// Confirmed body param on both the convert and stream endpoints
    /// (https://elevenlabs.io/docs/api-reference/text-to-speech/convert).
    seed: Option<u64>,
    /// Streaming latency-optimization tier 0..=4 → emitted as the URL QUERY param
    /// `optimize_streaming_latency`. Confirmed query param on both the convert and stream
    /// endpoints (https://elevenlabs.io/docs/api-reference/text-to-speech/stream).
    optimize_streaming_latency: Option<u8>,
    /// Language enforcement → request BODY `language_code` (ISO-639-1, e.g. "en"). Forces the
    /// model to a language rather than auto-detecting. Confirmed body param on the convert + stream
    /// endpoints (https://elevenlabs.io/docs/api-reference/text-to-speech/convert).
    language_code: Option<String>,
    /// Forward context text → request BODY `next_text`: the text that comes AFTER `text`, giving
    /// the model lookahead for prosody continuity. Confirmed body param (convert + stream).
    next_text: Option<String>,
    /// Stitching: request IDs of PRECEDING generations → request BODY `previous_request_ids`
    /// (array, max 3). Confirmed body param (convert + stream).
    previous_request_ids: Option<Vec<String>>,
    /// Stitching: request IDs of FOLLOWING generations → request BODY `next_request_ids`
    /// (array, max 3). Confirmed body param (convert + stream).
    next_request_ids: Option<Vec<String>>,
    /// Text normalization mode → request BODY `apply_text_normalization` ("auto" | "on" | "off").
    /// Confirmed body param (convert + stream).
    apply_text_normalization: Option<String>,
    /// Language-specific text normalization → request BODY `apply_language_text_normalization`
    /// (bool). Confirmed body param (convert + stream).
    apply_language_text_normalization: Option<bool>,
    /// Use IVC instead of PVC voice version → request BODY `use_pvc_as_ivc` (bool). Confirmed body
    /// param (convert + stream).
    use_pvc_as_ivc: Option<bool>,
    /// Zero-retention / disable logging → URL QUERY `enable_logging` (bool). When `false`, ElevenLabs
    /// runs the request in zero-retention mode (history/logging disabled). Confirmed QUERY param on
    /// the convert + stream endpoints (https://elevenlabs.io/docs/api-reference/text-to-speech/convert).
    enable_logging: Option<bool>,
}

impl TTSRequestBuilder for ElevenLabsRequestBuilder {
    /// Build the ElevenLabs-specific HTTP request with URL, headers and body
    fn build_http_request(&self, client: &reqwest::Client, text: &str) -> reqwest::RequestBuilder {
        // Forward to the context method without previous_text
        self.build_http_request_with_context(client, text, None)
    }

    /// Build the ElevenLabs-specific HTTP request with context support
    fn build_http_request_with_context(
        &self,
        client: &reqwest::Client,
        text: &str,
        previous_text: Option<&str>,
    ) -> reqwest::RequestBuilder {
        // Get voice_id from config, required for ElevenLabs
        let default_voice = DEFAULT_VOICE_ID.to_string();
        let voice_id = self.config.voice_id.as_ref().unwrap_or(&default_voice);

        // Build the URL with voice_id
        let url = format!("{ELEVENLABS_TTS_URL}/{voice_id}");

        // Build query parameters
        let mut query_params = Vec::new();

        // Add output format based on config
        // ElevenLabs expects format like "pcm_24000", "pcm_16000", "pcm_22050", etc.
        // For linear16/pcm format, we need to specify PCM with the correct sample rate
        let output_format =
            output_format_for(self.config.audio_format.as_deref(), self.config.sample_rate);

        query_params.push(format!("output_format={output_format}"));

        // Streaming latency-optimization tier (0..=4) is an ElevenLabs URL QUERY param
        // (`optimize_streaming_latency`), NOT a body field. Confirmed against the live convert
        // and stream endpoint docs. Clamp to the documented 0..=4 range.
        if let Some(tier) = self.optimize_streaming_latency {
            let tier = tier.min(4);
            query_params.push(format!("optimize_streaming_latency={tier}"));
        }

        // Zero-retention / disable logging is an ElevenLabs URL QUERY param (`enable_logging`).
        // When set to `false`, ElevenLabs runs the request with history/logging disabled
        // (zero-retention mode). Confirmed query param on the convert + stream endpoints.
        if let Some(enable_logging) = self.enable_logging {
            query_params.push(format!("enable_logging={enable_logging}"));
        }

        // Build the final URL with query parameters
        let final_url = if !query_params.is_empty() {
            format!("{}?{}", url, query_params.join("&"))
        } else {
            url
        };

        // Build request body
        let mut body = json!({ "text": text });
        if !self.voice_settings.is_empty() {
            body["voice_settings"] = json!(self.voice_settings);
        }

        // Add previous_text for context continuity if available
        if let Some(prev) = previous_text {
            body["previous_text"] = json!(prev);
        }

        // Determinism seed is an ElevenLabs request-BODY param (`seed`, integer 0..=4294967295).
        // Confirmed against the live convert and stream endpoint docs. Clamp to the documented
        // upper bound (u32 max) so an out-of-range standardized seed is not silently rejected.
        if let Some(seed) = self.seed {
            body["seed"] = json!(seed.min(u32::MAX as u64));
        }

        // Language enforcement → BODY `language_code` (ISO-639-1). Forces the synthesis language
        // rather than auto-detecting. Confirmed body param (convert + stream).
        if let Some(language_code) = &self.language_code {
            body["language_code"] = json!(language_code);
        }

        // Forward context text → BODY `next_text` (lookahead text following `text`). Confirmed body
        // param (convert + stream). `previous_text` (the lookback) is already emitted above.
        if let Some(next_text) = &self.next_text {
            body["next_text"] = json!(next_text);
        }

        // Stitching: preceding/following generation request IDs → BODY `previous_request_ids` /
        // `next_request_ids` (each an array, max 3). Confirmed body params (convert + stream).
        if let Some(ids) = &self.previous_request_ids {
            body["previous_request_ids"] = json!(ids);
        }
        if let Some(ids) = &self.next_request_ids {
            body["next_request_ids"] = json!(ids);
        }

        // Text normalization mode → BODY `apply_text_normalization` ("auto" | "on" | "off").
        // Confirmed body param (convert + stream).
        if let Some(mode) = &self.apply_text_normalization {
            body["apply_text_normalization"] = json!(mode);
        }

        // Language-specific text normalization → BODY `apply_language_text_normalization` (bool).
        // Confirmed body param (convert + stream).
        if let Some(flag) = self.apply_language_text_normalization {
            body["apply_language_text_normalization"] = json!(flag);
        }

        // Use IVC instead of PVC voice version → BODY `use_pvc_as_ivc` (bool). Confirmed body param
        // (convert + stream).
        if let Some(flag) = self.use_pvc_as_ivc {
            body["use_pvc_as_ivc"] = json!(flag);
        }

        // `model_id` only when one was chosen. It is optional (ElevenLabs defaults it to
        // eleven_multilingual_v2), and the model is a choice with real trade-offs — latency,
        // languages, which settings apply — that belongs to the caller or the deployment. It
        // used to be filled with eleven_flash_v2_5 whenever none was named.
        if !self.config.model.is_empty() {
            body["model_id"] = json!(self.config.model);
        }

        // Build the request with ElevenLabs-specific headers
        // Set Accept header based on the format
        let accept_header = if output_format.starts_with("pcm") {
            "audio/pcm"
        } else if output_format.starts_with("mp3") {
            "audio/mpeg"
        } else if output_format.starts_with("ulaw") || output_format.starts_with("alaw") {
            "audio/basic"
        } else if output_format.starts_with("opus") {
            "audio/opus"
        } else {
            "audio/pcm"
        };

        client
            .post(final_url)
            .header("xi-api-key", &self.config.api_key)
            .header("Content-Type", "application/json")
            .header("Accept", accept_header)
            .json(&body)
    }

    /// Get the configuration
    fn get_config(&self) -> &TTSConfig {
        &self.config
    }
}

/// ElevenLabs TTS provider implementation using the ElevenLabs HTTP REST API
pub struct ElevenLabsTTS {
    /// Generic HTTP-based TTS provider
    provider: TTSProvider,
    /// Request builder
    request_builder: ElevenLabsRequestBuilder,
}

impl ElevenLabsTTS {
    /// Create a new ElevenLabs TTS instance
    pub fn new(config: TTSConfig) -> TTSResult<Self> {
        // Validate required fields for ElevenLabs
        if config.api_key.is_empty() {
            return Err(super::base::TTSError::InvalidConfiguration(
                "API key is required for ElevenLabs".to_string(),
            ));
        }

        // The rate the audio is LABELLED with must be the rate ElevenLabs is asked for. The
        // shared provider labels chunks with `config.sample_rate`, and `output_format_for` maps
        // any rate it does not know to 24 kHz; left alone, the two disagree and the caller plays
        // the audio at the wrong speed.
        let mut config = config;
        if let Some(rate) = output_format_for(config.audio_format.as_deref(), config.sample_rate)
            .strip_prefix("pcm_")
            .and_then(|r| r.parse::<u32>().ok())
        {
            config.sample_rate = Some(rate);
        }

        // Create voice settings from config
        let voice_settings = VoiceSettings {
            speed: config.speaking_rate,
            ..Default::default()
        };

        let request_builder = ElevenLabsRequestBuilder {
            config,
            voice_settings,
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };

        Ok(Self {
            provider: TTSProvider::new(),
            request_builder,
        })
    }

    /// Build from the standardized config (W1 keystone), mirroring `DeepgramTTS::from_standard`.
    /// ElevenLabs' advanced surface is its voice settings (`stability`, `similarity_boost`,
    /// `style`, `use_speaker_boost`, `speed`); these are mapped by
    /// [`VoiceSettings::from_standard`] and installed on the request builder so they reach the
    /// `voice_settings` request body — previously unreachable through the flat factory. The base
    /// `TTSConfig` (api_key, voice_id, audio_format, sample_rate, …) is carried through `new`.
    /// Sample-rate-dependent output format is already derived from `base.sample_rate` in the
    /// request builder. Two more standardized features are wired here onto the request builder so
    /// they reach the live wire (both confirmed against the current ElevenLabs API docs):
    ///   - `features.seed` → request BODY `seed` (integer 0..=4294967295) for best-effort
    ///     deterministic sampling (convert + stream endpoints).
    ///   - `features.optimize_streaming_latency` → URL QUERY `optimize_streaming_latency` (0..=4),
    ///     trading quality for lower TTFB (convert + stream endpoints).
    ///
    /// Eight further ElevenLabs synth knobs are wired here (all confirmed on the convert + stream
    /// endpoints; params are identical on `/stream`). The typed `language` feature carries the
    /// language enforcement; the rest have no canonical `TtsFeatures` slot and ride the open
    /// `extras` passthrough under their exact ElevenLabs param names:
    ///   - `features.language` → BODY `language_code` (language enforcement).
    ///   - extras `next_text` (string) → BODY `next_text` (forward context / lookahead).
    ///   - extras `previous_request_ids` (string | string[]) → BODY `previous_request_ids`
    ///     (stitching; array, max 3).
    ///   - extras `next_request_ids` (string | string[]) → BODY `next_request_ids` (stitching).
    ///   - extras `apply_text_normalization` (string "auto"|"on"|"off") → BODY same.
    ///   - extras `apply_language_text_normalization` (bool) → BODY same.
    ///   - extras `use_pvc_as_ivc` (bool) → BODY `use_pvc_as_ivc` (use IVC instead of PVC).
    ///   - extras `enable_logging` (bool) → URL QUERY `enable_logging` (zero-retention when false).
    ///
    /// Emotion, instructions, SSML, word timestamps and streaming have no ElevenLabs request
    /// parameter on this synth path and are skipped (capability gaps).
    ///
    /// Cache note: `language_code`, `next_text`, the stitching IDs, the two normalization knobs and
    /// `use_pvc_as_ivc` all change the produced audio. ElevenLabs uses the generic [`TTSProvider`],
    /// whose cache key is computed externally (`voice_manager`) — there is no per-provider config
    /// hash to extend in this file. (`enable_logging` is a retention/policy flag, not audio-changing.)
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let voice_settings = VoiceSettings::from_standard(std);
        let mut base = std.base.clone();
        // Honor an explicit features.sample_rate (the output-format derivation reads
        // base.sample_rate) — previously features.sample_rate was ignored for ElevenLabs. (S7.)
        if let Some(sr) = std.features.sample_rate {
            base.sample_rate = Some(sr);
        }
        let mut tts = Self::new(base)?;
        tts.request_builder.voice_settings = voice_settings;
        tts.request_builder.seed = std.features.seed;
        tts.request_builder.optimize_streaming_latency = std.features.optimize_streaming_latency;

        // Language enforcement → BODY `language_code` (the typed feature).
        tts.request_builder.language_code = std.features.language.clone();

        // The remaining ElevenLabs-unique knobs ride the open `extras` passthrough under their
        // exact API param names.
        let extras = &std.extras.0;
        if let Some(s) = extras.get("next_text").and_then(|v| v.as_str()) {
            tts.request_builder.next_text = Some(s.to_string());
        }
        tts.request_builder.previous_request_ids =
            Self::extract_request_ids(extras.get("previous_request_ids"));
        tts.request_builder.next_request_ids =
            Self::extract_request_ids(extras.get("next_request_ids"));
        if let Some(s) = extras
            .get("apply_text_normalization")
            .and_then(|v| v.as_str())
        {
            tts.request_builder.apply_text_normalization = Some(s.to_string());
        }
        if let Some(b) = extras
            .get("apply_language_text_normalization")
            .and_then(|v| v.as_bool())
        {
            tts.request_builder.apply_language_text_normalization = Some(b);
        }
        if let Some(b) = extras.get("use_pvc_as_ivc").and_then(|v| v.as_bool()) {
            tts.request_builder.use_pvc_as_ivc = Some(b);
        }
        if let Some(b) = extras.get("enable_logging").and_then(|v| v.as_bool()) {
            tts.request_builder.enable_logging = Some(b);
        }

        Ok(tts)
    }

    /// Normalize a stitching-IDs extras value into `Vec<String>`. Accepts either a JSON array of
    /// strings (`["id1","id2"]`) or a single string (`"id1"`, normalized to a one-element vec) so
    /// the open passthrough is forgiving about either shape.
    fn extract_request_ids(v: Option<&serde_json::Value>) -> Option<Vec<String>> {
        match v {
            Some(serde_json::Value::Array(arr)) => Some(
                arr.iter()
                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                    .collect(),
            ),
            Some(serde_json::Value::String(s)) => Some(vec![s.clone()]),
            _ => None,
        }
    }

    /// Set the request manager for this instance
    pub async fn set_req_manager(&mut self, req_manager: Arc<ReqManager>) {
        self.provider.set_req_manager(req_manager).await;
    }
}

impl Default for ElevenLabsTTS {
    fn default() -> Self {
        let config = TTSConfig {
            api_key: "__waav_default_elevenlabs_unused__".to_string(),
            ..TTSConfig::default()
        };
        match Self::new(config) {
            Ok(tts) => tts,
            Err(_) => Self {
                provider: TTSProvider::new(),
                request_builder: ElevenLabsRequestBuilder {
                    config: TTSConfig::default(),
                    voice_settings: VoiceSettings::default(),
                    seed: None,
                    optimize_streaming_latency: None,
                    language_code: None,
                    next_text: None,
                    previous_request_ids: None,
                    next_request_ids: None,
                    apply_text_normalization: None,
                    apply_language_text_normalization: None,
                    use_pvc_as_ivc: None,
                    enable_logging: None,
                },
            },
        }
    }
}

#[async_trait]
impl BaseTTS for ElevenLabsTTS {
    fn new(config: TTSConfig) -> TTSResult<Self> {
        ElevenLabsTTS::new(config)
    }

    fn get_provider(&mut self) -> Option<&mut TTSProvider> {
        Some(&mut self.provider)
    }

    async fn connect(&mut self) -> TTSResult<()> {
        // Use the base URL for ElevenLabs API with config-based request manager
        self.provider
            .generic_connect_with_config("https://api.elevenlabs.io", &self.request_builder.config)
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
            tracing::info!("ElevenLabs TTS not ready, attempting to connect...");
            self.connect().await?;
        }
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
            "provider": "elevenlabs",
            "version": "2.0.0",
            "api_type": "HTTP REST",
            "connection_pooling": true,
            "supported_formats": ["mp3", "pcm", "ulaw"],
            "supported_sample_rates": [8000, 11025, 16000, 22050, 24000, 44100, 48000],
            // Note: eleven_multilingual_v1 and eleven_monolingual_v1 were deprecated
            // and removed by ElevenLabs on December 15, 2025
            "supported_models": [
                "eleven_v3",
                "eleven_multilingual_v2",
                "eleven_turbo_v2",
                "eleven_turbo_v2_5",
                "eleven_flash_v2",
                "eleven_flash_v2_5"
            ],
            "endpoint": "https://api.elevenlabs.io/v1/text-to-speech",
            "documentation": "https://elevenlabs.io/docs/api-reference/text-to-speech",
            "features": {
                "voice_settings": true,
                "pronunciation_dictionaries": true,
                "streaming_optimization": true,
                "text_normalization": true
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (TTS analog): the standardized voice features (stability, similarity_boost,
    // style, speaker boost, speed) flow into ElevenLabs `VoiceSettings` — previously unreachable
    // via the flat factory. The base `speaking_rate` seeds speed unless the `speed` feature wins.
    #[test]
    fn from_standard_maps_elevenlabs_voice_settings() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                speaking_rate: Some(1.25),
                ..Default::default()
            },
            features: TtsFeatures {
                stability: Some(0.7),
                similarity_boost: Some(0.9),
                style: Some(0.3),
                use_speaker_boost: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let settings = VoiceSettings::from_standard(&std);
        assert_eq!(settings.stability, Some(0.7));
        assert_eq!(settings.similarity_boost, Some(0.9));
        assert_eq!(settings.style, Some(0.3));
        assert_eq!(settings.use_speaker_boost, Some(true));
        // base.speaking_rate seeds speed when no explicit speed feature is set
        assert_eq!(settings.speed, Some(1.25));

        // the explicit speed feature overrides base.speaking_rate
        let mut std2 = std.clone();
        std2.features.speed = Some(2.0);
        assert_eq!(VoiceSettings::from_standard(&std2).speed, Some(2.0));
    }

    // W1 keystone (struct-level, mirrors DeepgramTTS::from_standard): the standardized voice
    // features reach the live request builder's `voice_settings` through the provider STRUCT's
    // `from_standard` — the path the dispatch helper actually constructs.
    #[tokio::test]
    async fn from_standard_struct_installs_voice_settings() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                voice_id: Some("test_voice".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                stability: Some(0.71),
                similarity_boost: Some(0.91),
                style: Some(0.33),
                use_speaker_boost: Some(true),
                speed: Some(1.4),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = ElevenLabsTTS::from_standard(&std).unwrap();
        let vs = &tts.request_builder.voice_settings;
        assert_eq!(vs.stability, Some(0.71));
        assert_eq!(vs.similarity_boost, Some(0.91));
        assert_eq!(vs.style, Some(0.33));
        assert_eq!(vs.use_speaker_boost, Some(true));
        assert_eq!(vs.speed, Some(1.4));
        // base carried through
        assert_eq!(
            tts.request_builder.config.voice_id.as_deref(),
            Some("test_voice")
        );
    }

    #[tokio::test]
    async fn test_elevenlabs_tts_creation() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("test_voice_id".to_string()),
            ..Default::default()
        };
        let tts = ElevenLabsTTS::new(config).unwrap();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_elevenlabs_tts_invalid_config() {
        let config = TTSConfig {
            api_key: "".to_string(), // Missing API key
            ..Default::default()
        };
        let result = ElevenLabsTTS::new(config);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn elevenlabs_default_is_disconnected_and_does_not_panic_on_empty_base_config() {
        let tts = ElevenLabsTTS::default();
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);
        assert!(
            !tts.request_builder.config.api_key.is_empty(),
            "Default must not call ElevenLabsTTS::new with the empty base API key"
        );
    }

    #[tokio::test]
    async fn test_voice_settings_creation() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let default_voice_settings = VoiceSettings::default();

        let tts = ElevenLabsTTS::new(config).unwrap();
        assert_eq!(tts.request_builder.voice_settings.speed, Some(1.5));
        assert_eq!(
            tts.request_builder.voice_settings.stability,
            default_voice_settings.stability
        );
        assert_eq!(
            tts.request_builder.voice_settings.similarity_boost,
            default_voice_settings.similarity_boost
        );
    }

    #[tokio::test]
    async fn test_http_request_building() {
        let config = TTSConfig {
            voice_id: Some("test_voice_id".to_string()),
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
            api_key: "test_key".to_string(),
            model: "eleven_multilingual_v2".to_string(),
            ..Default::default()
        };

        let voice_settings = VoiceSettings::default();
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings,
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Test text");

        // Get the request as built
        let built_request = request.build().unwrap();
        let url = built_request.url().to_string();

        assert!(url.contains("test_voice_id"));
        assert!(url.contains("output_format=pcm_24000"));
        assert!(url.starts_with("https://api.elevenlabs.io/v1/text-to-speech/"));

        // Check headers
        let headers = built_request.headers();
        assert_eq!(headers.get("xi-api-key").unwrap(), "test_key");
        assert_eq!(headers.get("content-type").unwrap(), "application/json");
        assert_eq!(headers.get("accept").unwrap(), "audio/pcm");
    }

    // No model named → no `model_id` on the wire, and no `voice_settings` either: both are
    // optional, and ElevenLabs then applies its own default model and the voice's SAVED settings.
    // WaaV used to send eleven_flash_v2_5 and its own 0.5/0.8/0/false tuning on every request.
    #[tokio::test]
    async fn nothing_chosen_sends_neither_model_nor_voice_settings() {
        let config = TTSConfig {
            voice_id: Some("test_voice_id".to_string()),
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
            api_key: "test_key".to_string(),
            model: String::new(), // no model supplied by the caller
            ..Default::default()
        };

        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings: VoiceSettings::default(),
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let built_request = builder
            .build_http_request(&client, "Test text")
            .build()
            .unwrap();

        let body_bytes = built_request.body().and_then(|b| b.as_bytes()).unwrap();
        let body_json: serde_json::Value = serde_json::from_slice(body_bytes).unwrap();
        assert!(body_json.get("model_id").is_none(), "{body_json}");
        assert!(body_json.get("voice_settings").is_none(), "{body_json}");
    }

    /// A setting the caller chose is sent, and ONLY that one — the others stay the voice's own.
    #[tokio::test]
    async fn only_the_chosen_voice_settings_are_sent() {
        let builder = ElevenLabsRequestBuilder {
            config: TTSConfig {
                voice_id: Some("test_voice_id".to_string()),
                api_key: "test_key".to_string(),
                model: "eleven_multilingual_v2".to_string(),
                ..Default::default()
            },
            voice_settings: VoiceSettings {
                stability: Some(0.5),
                ..Default::default()
            },
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let built = builder
            .build_http_request(&reqwest::Client::new(), "hi")
            .build()
            .unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(built.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
        assert_eq!(body["model_id"], "eleven_multilingual_v2");
        assert_eq!(
            body["voice_settings"],
            serde_json::json!({ "stability": 0.5 })
        );
    }

    #[test]
    fn g711_formats_map_to_elevenlabs_native_outputs() {
        assert_eq!(output_format_for(Some("mulaw"), None), "ulaw_8000");
        assert_eq!(output_format_for(Some("ulaw"), None), "ulaw_8000");
        assert_eq!(output_format_for(Some("alaw"), None), "alaw_8000");
    }

    #[tokio::test]
    async fn test_mp3_format_request() {
        let config = TTSConfig {
            voice_id: Some("test_voice_id".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(44100),
            api_key: "test_key".to_string(),
            model: "eleven_multilingual_v2".to_string(),
            ..Default::default()
        };

        let voice_settings = VoiceSettings::default();
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings,
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let request = builder.build_http_request(&client, "Test text");

        // Get the request as built
        let built_request = request.build().unwrap();
        let url = built_request.url().to_string();

        assert!(url.contains("output_format=mp3_44100_128"));

        // Check headers
        let headers = built_request.headers();
        assert_eq!(headers.get("accept").unwrap(), "audio/mpeg");
    }

    #[tokio::test]
    async fn test_elevenlabs_lifecycle() {
        let config = TTSConfig {
            api_key: "test_key".to_string(),
            voice_id: Some("test_voice_id".to_string()),
            ..Default::default()
        };

        let tts = ElevenLabsTTS::new(config).unwrap();

        // Initially disconnected
        assert!(!tts.is_ready());
        assert_eq!(tts.get_connection_state(), ConnectionState::Disconnected);

        // Nothing was chosen, so nothing is set: the voice's saved settings apply.
        assert!(tts.request_builder.voice_settings.is_empty());
    }

    #[tokio::test]
    async fn test_previous_text_included_when_provided() {
        let config = TTSConfig {
            voice_id: Some("test_voice_id".to_string()),
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
            api_key: "test_key".to_string(),
            model: "eleven_multilingual_v2".to_string(),
            ..Default::default()
        };

        let voice_settings = VoiceSettings::default();
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings,
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();

        // Build request with previous_text
        let request = builder.build_http_request_with_context(
            &client,
            "Second utterance",
            Some("First utterance"),
        );

        // Get the request as built
        let built_request = request.build().unwrap();

        // Extract and verify the body contains previous_text
        let body_bytes = built_request.body().and_then(|b| b.as_bytes());
        assert!(body_bytes.is_some());

        let body_str = std::str::from_utf8(body_bytes.unwrap()).unwrap();
        let body_json: serde_json::Value = serde_json::from_str(body_str).unwrap();

        assert_eq!(body_json["text"], "Second utterance");
        assert_eq!(body_json["previous_text"], "First utterance");
        assert!(body_json["model_id"].is_string());
    }

    #[tokio::test]
    async fn test_previous_text_omitted_when_none() {
        let config = TTSConfig {
            voice_id: Some("test_voice_id".to_string()),
            audio_format: Some("linear16".to_string()),
            sample_rate: Some(24000),
            api_key: "test_key".to_string(),
            model: "eleven_multilingual_v2".to_string(),
            ..Default::default()
        };

        let voice_settings = VoiceSettings::default();
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings,
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();

        // Build request without previous_text
        let request = builder.build_http_request_with_context(&client, "First utterance", None);

        // Get the request as built
        let built_request = request.build().unwrap();

        // Extract and verify the body does NOT contain previous_text
        let body_bytes = built_request.body().and_then(|b| b.as_bytes());
        assert!(body_bytes.is_some());

        let body_str = std::str::from_utf8(body_bytes.unwrap()).unwrap();
        let body_json: serde_json::Value = serde_json::from_str(body_str).unwrap();

        assert_eq!(body_json["text"], "First utterance");
        assert!(body_json["previous_text"].is_null());
        assert!(body_json["model_id"].is_string());
    }

    // ---- WIRE-LEVEL: seed reaches the serialized request BODY -----------------------------------
    // Confirmed body param on the convert + stream endpoints
    // (https://elevenlabs.io/docs/api-reference/text-to-speech/convert). This asserts the param in
    // the actual serialized JSON body, not just the builder field — the bug class the review caught.
    #[tokio::test]
    async fn test_seed_reaches_request_body() {
        let config = TTSConfig {
            voice_id: Some("test_voice_id".to_string()),
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings: VoiceSettings::default(),
            seed: Some(12345),
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let built = builder
            .build_http_request(&client, "hello")
            .build()
            .unwrap();

        // seed is a BODY param, not a URL param.
        assert!(
            !built.url().to_string().contains("seed"),
            "seed must not leak into the URL"
        );
        let body_str =
            std::str::from_utf8(built.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
        let body_json: serde_json::Value = serde_json::from_str(body_str).unwrap();
        assert_eq!(body_json["seed"], 12345);
    }

    // seed clamps to the documented upper bound (u32::MAX = 4294967295).
    #[tokio::test]
    async fn test_seed_clamped_to_documented_max() {
        let config = TTSConfig {
            voice_id: Some("v".to_string()),
            api_key: "k".to_string(),
            ..Default::default()
        };
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings: VoiceSettings::default(),
            seed: Some(u64::MAX),
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let built = builder.build_http_request(&client, "x").build().unwrap();
        let body_str =
            std::str::from_utf8(built.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
        let body_json: serde_json::Value = serde_json::from_str(body_str).unwrap();
        assert_eq!(body_json["seed"], u32::MAX as u64);
    }

    // seed is OMITTED from the body when unset (no spurious null/0).
    #[tokio::test]
    async fn test_seed_omitted_when_unset() {
        let config = TTSConfig {
            voice_id: Some("v".to_string()),
            api_key: "k".to_string(),
            ..Default::default()
        };
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings: VoiceSettings::default(),
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let built = builder.build_http_request(&client, "x").build().unwrap();
        let body_str =
            std::str::from_utf8(built.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
        let body_json: serde_json::Value = serde_json::from_str(body_str).unwrap();
        assert!(body_json.get("seed").is_none());
    }

    // ---- WIRE-LEVEL: optimize_streaming_latency reaches the request URL QUERY -------------------
    // Confirmed query param on the convert + stream endpoints
    // (https://elevenlabs.io/docs/api-reference/text-to-speech/stream).
    #[tokio::test]
    async fn test_optimize_streaming_latency_reaches_url() {
        let config = TTSConfig {
            voice_id: Some("test_voice_id".to_string()),
            api_key: "test_key".to_string(),
            ..Default::default()
        };
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings: VoiceSettings::default(),
            seed: None,
            optimize_streaming_latency: Some(3),
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let built = builder.build_http_request(&client, "hi").build().unwrap();
        let url = built.url().to_string();
        assert!(
            url.contains("optimize_streaming_latency=3"),
            "expected latency tier in URL, got: {url}"
        );
    }

    // optimize_streaming_latency clamps to the documented 0..=4 range.
    #[tokio::test]
    async fn test_optimize_streaming_latency_clamped() {
        let config = TTSConfig {
            voice_id: Some("v".to_string()),
            api_key: "k".to_string(),
            ..Default::default()
        };
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings: VoiceSettings::default(),
            seed: None,
            optimize_streaming_latency: Some(9),
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let built = builder.build_http_request(&client, "x").build().unwrap();
        let url = built.url().to_string();
        assert!(
            url.contains("optimize_streaming_latency=4"),
            "tier should clamp to 4, got: {url}"
        );
    }

    // optimize_streaming_latency is OMITTED from the URL when unset.
    #[tokio::test]
    async fn test_optimize_streaming_latency_omitted_when_unset() {
        let config = TTSConfig {
            voice_id: Some("v".to_string()),
            api_key: "k".to_string(),
            ..Default::default()
        };
        let builder = ElevenLabsRequestBuilder {
            config,
            voice_settings: VoiceSettings::default(),
            seed: None,
            optimize_streaming_latency: None,
            language_code: None,
            next_text: None,
            previous_request_ids: None,
            next_request_ids: None,
            apply_text_normalization: None,
            apply_language_text_normalization: None,
            use_pvc_as_ivc: None,
            enable_logging: None,
        };
        let client = reqwest::Client::new();
        let built = builder.build_http_request(&client, "x").build().unwrap();
        assert!(
            !built
                .url()
                .to_string()
                .contains("optimize_streaming_latency")
        );
    }

    // ---- WIRE-LEVEL through the STRUCT's from_standard (the dispatch-constructed path) ----------
    // Asserts that features.seed and features.optimize_streaming_latency, when set on the
    // StandardTTSConfig, actually reach the serialized body/URL of a request built by the provider
    // struct's from_standard — not merely the builder fields.
    #[tokio::test]
    async fn from_standard_wires_seed_and_latency_to_the_wire() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                voice_id: Some("test_voice".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                seed: Some(777),
                optimize_streaming_latency: Some(2),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let tts = ElevenLabsTTS::from_standard(&std).unwrap();
        let client = reqwest::Client::new();
        let built = tts
            .request_builder
            .build_http_request(&client, "go")
            .build()
            .unwrap();

        // latency tier on the URL
        assert!(
            built
                .url()
                .to_string()
                .contains("optimize_streaming_latency=2"),
            "latency tier must reach the URL via from_standard"
        );
        // seed in the body
        let body_str =
            std::str::from_utf8(built.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
        let body_json: serde_json::Value = serde_json::from_str(body_str).unwrap();
        assert_eq!(body_json["seed"], 777);
    }

    // ---- WIRE-LEVEL: language enforcement + stitching/context/normalization/IVC + logging -------
    // All confirmed on the convert + stream endpoints; params identical on `/stream`. These assert
    // the params reach the actual serialized request BODY / URL of the request built by the provider
    // struct's from_standard — not merely the builder/config fields (the recurring bug class).
    #[tokio::test]
    async fn from_standard_wires_elevenlabs_extended_features_to_the_wire() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert(
            "next_text".into(),
            serde_json::json!("the following sentence"),
        );
        extras.insert(
            "previous_request_ids".into(),
            serde_json::json!(["req-prev-1", "req-prev-2"]),
        );
        extras.insert("next_request_ids".into(), serde_json::json!(["req-next-1"]));
        extras.insert("apply_text_normalization".into(), serde_json::json!("on"));
        extras.insert(
            "apply_language_text_normalization".into(),
            serde_json::json!(true),
        );
        extras.insert("use_pvc_as_ivc".into(), serde_json::json!(true));
        // Zero-retention: enable_logging=false → URL query param.
        extras.insert("enable_logging".into(), serde_json::json!(false));

        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                voice_id: Some("test_voice".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("es".into()), // typed → language_code
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let tts = ElevenLabsTTS::from_standard(&std).unwrap();
        let built = tts
            .request_builder
            .build_http_request(&reqwest::Client::new(), "hola")
            .build()
            .unwrap();

        // enable_logging is a URL QUERY param (zero-retention), NOT a body field.
        let url = built.url().to_string();
        assert!(
            url.contains("enable_logging=false"),
            "enable_logging must reach the URL query, got: {url}"
        );
        assert!(
            !url.contains("language_code"),
            "language_code must NOT leak into the URL (it is a body param)"
        );

        // The rest are BODY params.
        let body_str =
            std::str::from_utf8(built.body().and_then(|b| b.as_bytes()).unwrap()).unwrap();
        let body: serde_json::Value = serde_json::from_str(body_str).unwrap();
        assert_eq!(body["language_code"], "es");
        assert_eq!(body["next_text"], "the following sentence");
        assert_eq!(
            body["previous_request_ids"],
            serde_json::json!(["req-prev-1", "req-prev-2"])
        );
        assert_eq!(body["next_request_ids"], serde_json::json!(["req-next-1"]));
        assert_eq!(body["apply_text_normalization"], "on");
        assert_eq!(body["apply_language_text_normalization"], true);
        assert_eq!(body["use_pvc_as_ivc"], true);
    }

    // Single-string stitching ID is normalized to a one-element wire array, and unset extended
    // features are OMITTED from the body (no spurious nulls).
    #[tokio::test]
    async fn from_standard_elevenlabs_request_ids_single_string_and_omitted_fields() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("previous_request_ids".into(), serde_json::json!("only-one"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                voice_id: Some("v".into()),
                ..Default::default()
            },
            features: TtsFeatures::default(),
            extras: ProviderExtras(extras),
        };
        let tts = ElevenLabsTTS::from_standard(&std).unwrap();
        let built = tts
            .request_builder
            .build_http_request(&reqwest::Client::new(), "x")
            .build()
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(
            std::str::from_utf8(built.body().and_then(|b| b.as_bytes()).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            body["previous_request_ids"],
            serde_json::json!(["only-one"])
        );
        // Unset extended features are omitted entirely.
        assert!(body.get("language_code").is_none());
        assert!(body.get("next_text").is_none());
        assert!(body.get("next_request_ids").is_none());
        assert!(body.get("apply_text_normalization").is_none());
        assert!(body.get("use_pvc_as_ivc").is_none());
        assert!(!built.url().to_string().contains("enable_logging"));
    }

    #[test]
    fn output_format_for_maps_each_openai_format() {
        assert_eq!(output_format_for(Some("mp3"), None), "mp3_44100_128");
        assert_eq!(output_format_for(Some("opus"), None), "opus_48000_64");
        assert_eq!(output_format_for(Some("pcm"), None), "pcm_24000");
        assert_eq!(
            output_format_for(Some("linear16"), Some(16000)),
            "pcm_16000"
        );
        // No ElevenLabs equivalent: these fall to PCM, which is what the OpenAI route reads as
        // "cannot produce this" — and `wav`, which the route wraps itself.
        assert_eq!(output_format_for(Some("wav"), None), "pcm_24000");
        assert_eq!(output_format_for(Some("aac"), None), "pcm_24000");
        assert_eq!(output_format_for(Some("flac"), None), "pcm_24000");
        assert_eq!(
            output_format_for(Some("opus_48000_128"), None),
            "opus_48000_128"
        );
        assert_eq!(output_format_for(None, None), "pcm_24000");
        assert_eq!(output_format_for(Some("pcm"), Some(8000)), "pcm_8000");
        assert_eq!(output_format_for(Some("pcm"), Some(48000)), "pcm_48000");
    }

    #[test]
    fn the_reported_rate_is_the_rate_requested_from_elevenlabs() {
        let mk = |format: &str, rate: Option<u32>| {
            ElevenLabsTTS::new(TTSConfig {
                provider: "elevenlabs".into(),
                api_key: "k".into(),
                audio_format: Some(format.into()),
                sample_rate: rate,
                ..Default::default()
            })
            .unwrap()
        };
        // A rate ElevenLabs has no PCM for is served at 24 kHz, and must be LABELLED 24 kHz.
        assert_eq!(
            mk("pcm", Some(12345)).request_builder.config.sample_rate,
            Some(24000)
        );
        assert_eq!(
            mk("pcm", Some(8000)).request_builder.config.sample_rate,
            Some(8000)
        );
        assert_eq!(
            mk("wav", None).request_builder.config.sample_rate,
            Some(24000)
        );
        // A codec keeps whatever it was given; only PCM output is relabelled.
        assert_eq!(
            mk("mp3", Some(22050)).request_builder.config.sample_rate,
            Some(22050)
        );
    }
}
