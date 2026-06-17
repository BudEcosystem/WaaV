//! Tinkoff VoiceKit STT Configuration
//!
//! Configuration types for Tinkoff's Speech-to-Text gRPC API with support for
//! Russian language, multiple audio encodings, and configurable VAD.

use crate::core::stt::base::STTConfig;
use serde::{Deserialize, Serialize};

/// Tinkoff STT provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TinkoffSttConfig {
    /// Base STT configuration (api_key, language, sample_rate, etc.)
    #[serde(flatten)]
    pub base: STTConfig,

    /// Tinkoff API key (from console)
    #[serde(default, skip_serializing)]
    pub api_key: String,

    /// Tinkoff secret key (from console)
    #[serde(default, skip_serializing)]
    pub secret_key: String,

    /// Override gRPC endpoint (plaintext, no TLS) — used by mock e2e tests.
    #[serde(default, skip_serializing)]
    pub endpoint_override: Option<String>,

    /// Audio encoding format
    #[serde(default)]
    pub encoding: TinkoffAudioEncoding,

    /// Maximum alternatives to return
    #[serde(default = "default_max_alternatives")]
    pub max_alternatives: u32,

    /// Enable automatic punctuation
    #[serde(default = "default_punctuation")]
    pub enable_punctuation: bool,

    /// Enable interim (partial) results
    #[serde(default = "default_interim_results")]
    pub interim_results: bool,

    /// Single utterance mode (stop after first pause)
    #[serde(default)]
    pub single_utterance: bool,

    /// Enable profanity filter (`RecognitionConfig.profanity_filter`).
    #[serde(default)]
    pub profanity_filter: bool,

    /// Phrase-boosting speech contexts (`RecognitionConfig.speech_contexts`).
    #[serde(default)]
    pub speech_contexts: Vec<SpeechContextConfig>,

    /// Enable denormalization — convert spoken numerals to numeric form
    /// (`RecognitionConfig.enable_denormalization`).
    #[serde(default)]
    pub enable_denormalization: bool,

    /// Enable gender identification (`RecognitionConfig.enable_gender_identification`).
    #[serde(default)]
    pub enable_gender_identification: bool,

    /// Disable phrase range detection — all speech recognised as one phrase
    /// (`RecognitionConfig.do_not_perform_vad`). Mutually exclusive with `vad_config`.
    #[serde(default)]
    pub do_not_perform_vad: bool,

    /// Desired interval (seconds) between interim results
    /// (`InterimResultsConfig.interval`). `None` → service default cadence.
    #[serde(default)]
    pub interim_results_interval: Option<f32>,

    /// VAD (Voice Activity Detection) configuration
    #[serde(default)]
    pub vad_config: Option<VadConfig>,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,
}

fn default_max_alternatives() -> u32 {
    1
}

fn default_punctuation() -> bool {
    true
}

fn default_interim_results() -> bool {
    true
}

fn default_connection_timeout() -> u64 {
    10
}

fn default_request_timeout() -> u64 {
    60
}

/// Coerce a JSON extras value (number or numeric string) into an `f32`.
fn value_as_f32(v: &serde_json::Value) -> Option<f32> {
    v.as_f64()
        .map(|n| n as f32)
        .or_else(|| v.as_str().and_then(|s| s.parse::<f32>().ok()))
}

/// Read a string-valued `ProviderExtras` key.
fn extras_str(
    std: &crate::core::stt::standard::StandardSTTConfig,
    key: &str,
) -> Option<String> {
    std.extras
        .0
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

impl Default for TinkoffSttConfig {
    fn default() -> Self {
        Self {
            base: STTConfig {
                provider: "tinkoff".to_string(),
                api_key: String::new(),
                language: "ru-RU".to_string(),
                sample_rate: 16000,
                channels: 1,
                punctuation: true,
                encoding: "linear16".to_string(),
                model: "default".to_string(),
            },
            api_key: String::new(),
            secret_key: String::new(),
            endpoint_override: None,
            encoding: TinkoffAudioEncoding::default(),
            max_alternatives: default_max_alternatives(),
            enable_punctuation: default_punctuation(),
            interim_results: default_interim_results(),
            single_utterance: false,
            profanity_filter: false,
            speech_contexts: Vec::new(),
            enable_denormalization: false,
            enable_gender_identification: false,
            do_not_perform_vad: false,
            interim_results_interval: None,
            vad_config: None,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
        }
    }
}

impl TinkoffSttConfig {
    /// Build from the standardized config (W1 keystone). Tinkoff's VoiceKit gRPC
    /// `RecognitionConfig`/`StreamingRecognitionConfig` is richer than previously wired; this maps
    /// the canonical typed features it CAN express and threads the remaining provider-specific
    /// knobs through `ProviderExtras`:
    ///
    /// Typed → wire:
    /// - `interim_results`  → `StreamingRecognitionConfig.interim_results_config.enable_interim_results`
    /// - `profanity_filter` → `RecognitionConfig.profanity_filter` (field 5)
    /// - `numerals`         → `RecognitionConfig.enable_denormalization` (field 16; spoken→digits)
    /// - `keyterms`         → `RecognitionConfig.speech_contexts[].phrases[].text` (phrase boosting, field 6)
    ///
    /// Extras → wire (provider-specific, no canonical field):
    /// - `interim_results_config.interval` (f32)        → `InterimResultsConfig.interval` (field 2)
    /// - `do_not_perform_vad` (bool)                    → `RecognitionConfig.do_not_perform_vad` (field 13)
    /// - `vad_config.silence_max` (f32)                 → `VoiceActivityDetectionConfig.silence_max` (field 6)
    /// - `vad_config.silence_min` (f32)                 → `VoiceActivityDetectionConfig.silence_min` (field 7)
    /// - `enable_gender_identification` (bool)          → `RecognitionConfig.enable_gender_identification` (field 18)
    /// - `speech_context_dictionary_id` (string)        → `SpeechContext.speech_context_dictionary_id` (field 4)
    ///
    /// Word timestamps, smart formatting, redaction, entity/language detection have no field on
    /// this provider and stay at defaults (capability gaps).
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, String> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        // ---- typed features ----
        if let Some(i) = f.interim_results {
            cfg.interim_results = i;
        }
        if let Some(p) = f.profanity_filter {
            cfg.profanity_filter = p;
        }
        // Canonical `numerals` = "convert spoken numbers to digits" == Tinkoff denormalization.
        if let Some(n) = f.numerals {
            cfg.enable_denormalization = n;
        }
        // Phrase boosting: canonical keyterms map to a single SpeechContext of phrases.
        if let Some(terms) = &f.keyterms {
            let phrases: Vec<SpeechContextPhraseConfig> = terms
                .iter()
                .filter(|t| !t.is_empty())
                .map(|t| SpeechContextPhraseConfig {
                    text: t.clone(),
                    score: None,
                })
                .collect();
            if !phrases.is_empty() {
                cfg.speech_contexts.push(SpeechContextConfig {
                    phrases,
                    speech_context_dictionary_id: extras_str(
                        std,
                        "speech_context_dictionary_id",
                    ),
                });
            }
        }

        // ---- ProviderExtras passthrough ----
        let extras = &std.extras.0;
        if let Some(v) = extras.get("interim_results_config.interval").and_then(value_as_f32) {
            cfg.interim_results_interval = Some(v);
        }
        if let Some(b) = extras.get("do_not_perform_vad").and_then(|v| v.as_bool()) {
            cfg.do_not_perform_vad = b;
        }
        if let Some(b) = extras
            .get("enable_gender_identification")
            .and_then(|v| v.as_bool())
        {
            cfg.enable_gender_identification = b;
        }
        let silence_max = extras.get("vad_config.silence_max").and_then(value_as_f32);
        let silence_min = extras.get("vad_config.silence_min").and_then(value_as_f32);
        if silence_max.is_some() || silence_min.is_some() {
            let mut vad = cfg.vad_config.take().unwrap_or_default();
            if let Some(v) = silence_max {
                vad.silence_max = v;
            }
            if let Some(v) = silence_min {
                vad.silence_min = v;
            }
            cfg.vad_config = Some(vad);
        }

        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());

        Ok(cfg)
    }

    /// Create TinkoffSttConfig from base STTConfig
    ///
    /// Extracts Tinkoff-specific credentials from environment variables if not
    /// provided in the base config.
    pub fn from_base(base: STTConfig) -> Result<Self, String> {
        // The standardized `api_key` may carry both credentials as `api_key|secret_key` (Tinkoff
        // needs both to sign the JWT, but the keystone exposes a single field); env vars win.
        let (base_api_key, base_secret_key) = match base.api_key.split_once('|') {
            Some((a, s)) => (a.to_string(), s.to_string()),
            None => (base.api_key.clone(), String::new()),
        };
        let api_key = if base_api_key.is_empty() {
            std::env::var("TINKOFF_API_KEY").unwrap_or_default()
        } else {
            base_api_key
        };
        let secret_key = std::env::var("TINKOFF_SECRET_KEY").unwrap_or(base_secret_key);

        // Parse audio encoding
        let encoding = TinkoffAudioEncoding::from_str(&base.encoding)?;

        // Normalize language code
        let language = if base.language.is_empty() || base.language.to_lowercase() == "ru" {
            "ru-RU".to_string()
        } else {
            base.language.clone()
        };

        Ok(Self {
            base: STTConfig { language, ..base },
            api_key,
            secret_key,
            endpoint_override: None,
            encoding,
            max_alternatives: default_max_alternatives(),
            enable_punctuation: default_punctuation(),
            interim_results: default_interim_results(),
            single_utterance: false,
            profanity_filter: false,
            speech_contexts: Vec::new(),
            enable_denormalization: false,
            enable_gender_identification: false,
            do_not_perform_vad: false,
            interim_results_interval: None,
            vad_config: None,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
        })
    }

    /// Validate that all required credentials are present
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err(
                "Tinkoff API key is required. Set TINKOFF_API_KEY environment variable or provide in config.".to_string(),
            );
        }

        if self.secret_key.is_empty() {
            return Err(
                "Tinkoff secret key is required. Set TINKOFF_SECRET_KEY environment variable."
                    .to_string(),
            );
        }

        // Validate sample rate
        if !SUPPORTED_SAMPLE_RATES.contains(&self.base.sample_rate) {
            return Err(format!(
                "Unsupported sample rate: {}. Supported: {:?}",
                self.base.sample_rate, SUPPORTED_SAMPLE_RATES
            ));
        }

        Ok(())
    }

    /// Get the gRPC endpoint URL
    pub fn endpoint(&self) -> &'static str {
        "https://api.tinkoff.ai:443"
    }
}

/// Supported sample rates for Tinkoff STT
pub const SUPPORTED_SAMPLE_RATES: &[u32] = &[8000, 16000, 22050, 24000, 44100, 48000];

/// Audio encoding formats for Tinkoff STT
///
/// Maps to protobuf enum AudioEncoding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TinkoffAudioEncoding {
    /// Uncompressed 16-bit signed little-endian PCM
    #[default]
    #[serde(rename = "linear16")]
    Linear16,
    /// Opus encoded audio without container
    #[serde(rename = "raw_opus")]
    RawOpus,
    /// 8-bit G.711 mu-law
    #[serde(rename = "mulaw")]
    Mulaw,
    /// 8-bit G.711 A-law
    #[serde(rename = "alaw")]
    Alaw,
    /// FLAC lossless audio
    #[serde(rename = "flac")]
    Flac,
    /// MP3 audio
    #[serde(rename = "mp3")]
    MpegAudio,
}

impl TinkoffAudioEncoding {
    /// Get the protobuf enum value
    pub fn as_i32(&self) -> i32 {
        match self {
            Self::Linear16 => 1,
            Self::RawOpus => 2,
            Self::Mulaw => 5,
            Self::Alaw => 6,
            Self::Flac => 16,
            Self::MpegAudio => 8,
        }
    }

    /// Get the string representation for API
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Linear16 => "LINEAR16",
            Self::RawOpus => "RAW_OPUS",
            Self::Mulaw => "MULAW",
            Self::Alaw => "ALAW",
            Self::Flac => "FLAC",
            Self::MpegAudio => "MPEG_AUDIO",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "linear16" | "pcm16" | "pcm" => Ok(Self::Linear16),
            "raw_opus" | "opus" => Ok(Self::RawOpus),
            "mulaw" | "ulaw" => Ok(Self::Mulaw),
            "alaw" => Ok(Self::Alaw),
            "flac" => Ok(Self::Flac),
            "mp3" | "mpeg" | "mpeg_audio" => Ok(Self::MpegAudio),
            _ => Err(format!(
                "Unsupported Tinkoff encoding: {}. Supported: linear16, raw_opus, mulaw, alaw, flac, mp3",
                s
            )),
        }
    }
}

/// A phrase to boost (or suppress) during recognition (`SpeechContextPhrase`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SpeechContextPhraseConfig {
    /// Phrase text.
    pub text: String,
    /// Phrase score (recommended `[1.0, 10.0]`). `None` → service default of `1.0`.
    #[serde(default)]
    pub score: Option<f32>,
}

/// A set of phrases to be recognised with higher (or lower) probability (`SpeechContext`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SpeechContextConfig {
    /// Phrases to recognise with higher (or lower) probability.
    #[serde(default)]
    pub phrases: Vec<SpeechContextPhraseConfig>,
    /// Use a cloud-stored speech-context object by dictionary id.
    #[serde(default)]
    pub speech_context_dictionary_id: Option<String>,
}

/// Voice Activity Detection configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VadConfig {
    /// Minimum speech duration in seconds
    #[serde(default)]
    pub min_speech_duration: f32,
    /// Maximum speech duration in seconds
    #[serde(default)]
    pub max_speech_duration: f32,
    /// Silence duration threshold in seconds
    #[serde(default)]
    pub silence_duration_threshold: f32,
    /// Silence probability threshold (0.0 to 1.0)
    #[serde(default)]
    pub silence_prob_threshold: f32,
    /// Maximum duration of silence (seconds) required to end a phrase
    /// (`VoiceActivityDetectionConfig.silence_max`).
    #[serde(default)]
    pub silence_max: f32,
    /// Minimum duration of silence (seconds) required to end a phrase
    /// (`VoiceActivityDetectionConfig.silence_min`).
    #[serde(default)]
    pub silence_min: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: Tinkoff has a narrow surface, so the standardized config can only unlock
    // interim results here; diarization (set below) is a documented capability gap and must stay
    // at the provider default rather than silently mapping to an unrelated field.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tinkoff".into(),
                api_key: "test-api-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                interim_results: Some(false),
                diarization: Some(true), // capability gap: no field to map to
                ..Default::default()
            },
            ..StandardSTTConfig::from_base(STTConfig::default())
        };
        let cfg = TinkoffSttConfig::from_standard(&std).unwrap();
        assert!(!cfg.interim_results); // mapped from the standardized feature
        assert_eq!(cfg.api_key, "test-api-key"); // base carried through from_base
    }

    #[test]
    fn test_tinkoff_encoding_from_str() {
        assert_eq!(
            TinkoffAudioEncoding::from_str("linear16").unwrap(),
            TinkoffAudioEncoding::Linear16
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("pcm16").unwrap(),
            TinkoffAudioEncoding::Linear16
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("opus").unwrap(),
            TinkoffAudioEncoding::RawOpus
        );
        assert_eq!(
            TinkoffAudioEncoding::from_str("mulaw").unwrap(),
            TinkoffAudioEncoding::Mulaw
        );
        assert!(TinkoffAudioEncoding::from_str("invalid").is_err());
    }

    #[test]
    fn test_tinkoff_encoding_as_i32() {
        assert_eq!(TinkoffAudioEncoding::Linear16.as_i32(), 1);
        assert_eq!(TinkoffAudioEncoding::RawOpus.as_i32(), 2);
        assert_eq!(TinkoffAudioEncoding::Mulaw.as_i32(), 5);
        assert_eq!(TinkoffAudioEncoding::Alaw.as_i32(), 6);
        assert_eq!(TinkoffAudioEncoding::Flac.as_i32(), 16);
        assert_eq!(TinkoffAudioEncoding::MpegAudio.as_i32(), 8);
    }

    #[test]
    fn test_tinkoff_config_from_base() {
        let base = STTConfig {
            provider: "tinkoff".to_string(),
            api_key: "test-api-key".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            channels: 1,
            punctuation: true,
            encoding: "linear16".to_string(),
            model: "default".to_string(),
        };

        let config = TinkoffSttConfig::from_base(base).unwrap();
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.base.language, "ru-RU");
        assert_eq!(config.encoding, TinkoffAudioEncoding::Linear16);
    }

    #[test]
    fn test_tinkoff_config_validation_missing_api_key() {
        let config = TinkoffSttConfig::default();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn test_tinkoff_config_validation_missing_secret_key() {
        let mut config = TinkoffSttConfig::default();
        config.api_key = "test-key".to_string();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("secret key"));
    }

    #[test]
    fn test_tinkoff_config_validation_invalid_sample_rate() {
        let mut config = TinkoffSttConfig::default();
        config.api_key = "test-key".to_string();
        config.secret_key = "test-secret".to_string();
        config.base.sample_rate = 12345;
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("sample rate"));
    }

    #[test]
    fn test_vad_config_default() {
        let vad = VadConfig::default();
        assert_eq!(vad.min_speech_duration, 0.0);
        assert_eq!(vad.max_speech_duration, 0.0);
        assert_eq!(vad.silence_duration_threshold, 0.0);
        assert_eq!(vad.silence_prob_threshold, 0.0);
        assert_eq!(vad.silence_max, 0.0);
        assert_eq!(vad.silence_min, 0.0);
    }

    // from_standard mapping guard: the typed + extras features land on the provider config the
    // wire encoder reads. (That they reach the ENCODED bytes is asserted in provider.rs's
    // wire-level test via `create_streaming_config(..).encode()`.)
    #[test]
    fn from_standard_maps_new_features_onto_config() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("interim_results_config.interval".into(), serde_json::json!(0.3));
        extras.insert("do_not_perform_vad".into(), serde_json::json!(true));
        extras.insert("enable_gender_identification".into(), serde_json::json!(true));
        extras.insert("vad_config.silence_max".into(), serde_json::json!(1.5));
        extras.insert("vad_config.silence_min".into(), serde_json::json!(0.2));
        extras.insert(
            "speech_context_dictionary_id".into(),
            serde_json::json!("dict-7"),
        );

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "tinkoff".into(),
                api_key: "test-api-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                profanity_filter: Some(true),
                numerals: Some(true),
                keyterms: Some(vec!["Тинькофф".into(), "WaaV".into()]),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = TinkoffSttConfig::from_standard(&std).unwrap();

        assert!(cfg.profanity_filter);
        assert!(cfg.enable_denormalization); // from numerals
        assert!(cfg.enable_gender_identification);
        assert!(cfg.do_not_perform_vad);
        assert_eq!(cfg.interim_results_interval, Some(0.3));
        assert_eq!(cfg.speech_contexts.len(), 1);
        assert_eq!(cfg.speech_contexts[0].phrases.len(), 2);
        assert_eq!(cfg.speech_contexts[0].phrases[0].text, "Тинькофф");
        assert_eq!(
            cfg.speech_contexts[0].speech_context_dictionary_id.as_deref(),
            Some("dict-7")
        );
        let vad = cfg.vad_config.as_ref().expect("vad config built from extras");
        assert_eq!(vad.silence_max, 1.5);
        assert_eq!(vad.silence_min, 0.2);
    }
}
