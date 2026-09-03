//! Configuration types for Bhashini (ULCA) STT API.
//!
//! This module contains all configuration-related types including:
//! - Language support for 22+ Indian languages
//! - Audio format specifications
//! - Pipeline configuration
//! - Service ID resolution
//!
//! # Bhashini API Overview
//!
//! Bhashini is a Government of India (MeitY) initiative providing AI-powered
//! speech recognition for Indian languages through ULCA (Universal Language
//! Contribution APIs).
//!
//! ## Authentication
//!
//! Bhashini uses a 2-step authentication:
//! 1. Pipeline Config: userID + ulcaApiKey
//! 2. Pipeline Compute: inferenceApiKey (from config response)
//!
//! ## Supported Languages
//!
//! All 22 scheduled Indian languages plus English.

use super::super::base::STTConfig;
use serde::{Deserialize, Serialize};

fn validate_bhashini_stt_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// Constants
// =============================================================================

/// Bhashini ULCA base URL for pipeline configuration.
pub const BHASHINI_CONFIG_URL: &str =
    "https://meity-auth.ulcacontrib.org/ulca/apis/v0/model/getModelsPipeline";

/// Bhashini Dhruva API URL for inference/compute calls.
pub const BHASHINI_COMPUTE_URL: &str =
    "https://dhruva-api.bhashini.gov.in/services/inference/pipeline";

/// MeitY Pipeline ID (official government pipeline).
pub const MEITY_PIPELINE_ID: &str = "64392f96daac500b55c543cd";

/// AI4Bharat Pipeline ID (research partner pipeline).
pub const AI4BHARAT_PIPELINE_ID: &str = "643930aa521a4b1ba0f4c41d";

/// Default sample rate for ASR.
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

/// Minimum sample rate supported.
pub const MIN_SAMPLE_RATE: u32 = 8000;

/// Maximum audio duration in seconds (5 minutes).
pub const MAX_AUDIO_DURATION_SECS: u32 = 300;

/// Maximum audio size in bytes (approximately 5 minutes at 16kHz 16-bit mono).
pub const MAX_AUDIO_SIZE_BYTES: usize = 10 * 1024 * 1024;

// =============================================================================
// Language Enum
// =============================================================================

/// Supported languages for Bhashini ASR.
///
/// Bhashini supports all 22 scheduled Indian languages plus English.
/// Languages are identified by ISO-639 language codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BhashiniLanguage {
    /// Hindi (default)
    #[default]
    Hindi,
    /// Tamil
    Tamil,
    /// Telugu
    Telugu,
    /// Kannada
    Kannada,
    /// Malayalam
    Malayalam,
    /// Bengali
    Bengali,
    /// Marathi
    Marathi,
    /// Gujarati
    Gujarati,
    /// Punjabi
    Punjabi,
    /// Odia
    Odia,
    /// Urdu
    Urdu,
    /// Assamese
    Assamese,
    /// Sanskrit
    Sanskrit,
    /// English
    English,
    /// Nepali
    Nepali,
    /// Bodo
    Bodo,
    /// Dogri
    Dogri,
    /// Konkani
    Konkani,
    /// Maithili
    Maithili,
    /// Manipuri
    Manipuri,
    /// Santali
    Santali,
    /// Sindhi
    Sindhi,
    /// Bhojpuri
    Bhojpuri,
    /// Kashmiri
    Kashmiri,
}

impl BhashiniLanguage {
    /// Get the ISO-639 language code.
    #[inline]
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::Hindi => "hi",
            Self::Tamil => "ta",
            Self::Telugu => "te",
            Self::Kannada => "kn",
            Self::Malayalam => "ml",
            Self::Bengali => "bn",
            Self::Marathi => "mr",
            Self::Gujarati => "gu",
            Self::Punjabi => "pa",
            Self::Odia => "or",
            Self::Urdu => "ur",
            Self::Assamese => "as",
            Self::Sanskrit => "sa",
            Self::English => "en",
            Self::Nepali => "ne",
            Self::Bodo => "brx",
            Self::Dogri => "doi",
            Self::Konkani => "kok",
            Self::Maithili => "mai",
            Self::Manipuri => "mni",
            Self::Santali => "sat",
            Self::Sindhi => "sd",
            Self::Bhojpuri => "bho",
            Self::Kashmiri => "ks",
        }
    }

    /// Get the display name of the language.
    #[inline]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Hindi => "Hindi",
            Self::Tamil => "Tamil",
            Self::Telugu => "Telugu",
            Self::Kannada => "Kannada",
            Self::Malayalam => "Malayalam",
            Self::Bengali => "Bengali",
            Self::Marathi => "Marathi",
            Self::Gujarati => "Gujarati",
            Self::Punjabi => "Punjabi",
            Self::Odia => "Odia",
            Self::Urdu => "Urdu",
            Self::Assamese => "Assamese",
            Self::Sanskrit => "Sanskrit",
            Self::English => "English",
            Self::Nepali => "Nepali",
            Self::Bodo => "Bodo",
            Self::Dogri => "Dogri",
            Self::Konkani => "Konkani",
            Self::Maithili => "Maithili",
            Self::Manipuri => "Manipuri",
            Self::Santali => "Santali",
            Self::Sindhi => "Sindhi",
            Self::Bhojpuri => "Bhojpuri",
            Self::Kashmiri => "Kashmiri",
        }
    }

    /// Get the language family.
    pub fn family(&self) -> LanguageFamily {
        match self {
            Self::Tamil | Self::Telugu | Self::Kannada | Self::Malayalam => {
                LanguageFamily::Dravidian
            }
            Self::Hindi
            | Self::Bengali
            | Self::Marathi
            | Self::Gujarati
            | Self::Punjabi
            | Self::Odia
            | Self::Urdu
            | Self::Nepali
            | Self::Sindhi
            | Self::Konkani
            | Self::Maithili
            | Self::Bhojpuri
            | Self::Kashmiri
            | Self::Dogri => LanguageFamily::IndoAryan,
            Self::Assamese
            | Self::Sanskrit
            | Self::English
            | Self::Bodo
            | Self::Manipuri
            | Self::Santali => LanguageFamily::Misc,
        }
    }

    /// Get the recommended ASR service ID for this language.
    pub fn asr_service_id(&self) -> &'static str {
        match self.family() {
            LanguageFamily::Dravidian => "ai4bharat/conformer-multilingual-dravidian-gpu--t4",
            LanguageFamily::IndoAryan => {
                if matches!(self, Self::Hindi) {
                    "ai4bharat/conformer-hi-gpu--t4"
                } else {
                    "ai4bharat/conformer-multilingual-indo_aryan-gpu--t4"
                }
            }
            LanguageFamily::Misc => {
                if matches!(self, Self::English) {
                    "ai4bharat/whisper-medium-en--gpu--t4"
                } else if matches!(self, Self::Bhojpuri) {
                    "bhashini/iisc/asr-bho-t4"
                } else if matches!(self, Self::Maithili) {
                    "bhashini/iisc/asr-mai-t4"
                } else {
                    "bhashini/iitm/asr-misc--gpu--t4"
                }
            }
        }
    }

    /// Parse language from string (ISO-639 code or name).
    pub fn from_str_or_default(s: &str) -> Self {
        Self::from_code(s).unwrap_or_default()
    }

    /// Parse language from code, returns None if not recognized.
    pub fn from_code(s: &str) -> Option<Self> {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "hi" | "hindi" | "hin" => Some(Self::Hindi),
            "ta" | "tamil" | "tam" => Some(Self::Tamil),
            "te" | "telugu" | "tel" => Some(Self::Telugu),
            "kn" | "kannada" | "kan" => Some(Self::Kannada),
            "ml" | "malayalam" | "mal" => Some(Self::Malayalam),
            "bn" | "bengali" | "bangla" | "ben" => Some(Self::Bengali),
            "mr" | "marathi" | "mar" => Some(Self::Marathi),
            "gu" | "gujarati" | "guj" => Some(Self::Gujarati),
            "pa" | "punjabi" | "panjabi" | "pan" => Some(Self::Punjabi),
            "or" | "odia" | "oriya" | "ori" => Some(Self::Odia),
            "ur" | "urdu" | "urd" => Some(Self::Urdu),
            "as" | "assamese" | "asm" => Some(Self::Assamese),
            "sa" | "sanskrit" | "san" => Some(Self::Sanskrit),
            "en" | "english" | "eng" => Some(Self::English),
            "ne" | "nepali" | "nep" => Some(Self::Nepali),
            "brx" | "bodo" => Some(Self::Bodo),
            "doi" | "dogri" => Some(Self::Dogri),
            "kok" | "konkani" => Some(Self::Konkani),
            "mai" | "maithili" => Some(Self::Maithili),
            "mni" | "manipuri" => Some(Self::Manipuri),
            "sat" | "santali" => Some(Self::Santali),
            "sd" | "sindhi" => Some(Self::Sindhi),
            "bho" | "bhojpuri" => Some(Self::Bhojpuri),
            "ks" | "kashmiri" => Some(Self::Kashmiri),
            "gom" | "goan" | "konkani-goan" => Some(Self::Konkani), // Goan Konkani variant
            _ => None,
        }
    }

    /// Get all supported languages.
    pub fn all() -> &'static [Self] {
        &[
            Self::Hindi,
            Self::Tamil,
            Self::Telugu,
            Self::Kannada,
            Self::Malayalam,
            Self::Bengali,
            Self::Marathi,
            Self::Gujarati,
            Self::Punjabi,
            Self::Odia,
            Self::Urdu,
            Self::Assamese,
            Self::Sanskrit,
            Self::English,
            Self::Nepali,
            Self::Bodo,
            Self::Dogri,
            Self::Konkani,
            Self::Maithili,
            Self::Manipuri,
            Self::Santali,
            Self::Sindhi,
            Self::Bhojpuri,
            Self::Kashmiri,
        ]
    }
}

impl std::fmt::Display for BhashiniLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_code())
    }
}

// =============================================================================
// Language Family
// =============================================================================

/// Language family classification for service ID selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LanguageFamily {
    /// Dravidian languages: Tamil, Telugu, Kannada, Malayalam
    Dravidian,
    /// Indo-Aryan languages: Hindi, Bengali, Marathi, etc.
    IndoAryan,
    /// Miscellaneous: Assamese, Sanskrit, English, etc.
    Misc,
}

// =============================================================================
// Audio Format
// =============================================================================

/// Supported audio formats for Bhashini ASR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BhashiniAudioFormat {
    /// WAV format (recommended for Android/Web)
    #[default]
    Wav,
    /// FLAC format (recommended for iOS)
    Flac,
    /// MP3 format
    Mp3,
}

impl BhashiniAudioFormat {
    /// Get the format string for API requests.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
        }
    }

    /// Get the MIME type for this format.
    #[inline]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Flac => "audio/flac",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Parse from string.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "wav" | "wave" | "pcm" => Self::Wav,
            "flac" => Self::Flac,
            "mp3" | "mpeg" => Self::Mp3,
            _ => Self::default(),
        }
    }
}

impl std::fmt::Display for BhashiniAudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Pipeline Provider
// =============================================================================

/// Pipeline provider selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BhashiniPipelineProvider {
    /// MeitY official pipeline (government)
    #[default]
    MeitY,
    /// AI4Bharat research pipeline
    AI4Bharat,
}

impl BhashiniPipelineProvider {
    /// Get the pipeline ID.
    #[inline]
    pub fn pipeline_id(&self) -> &'static str {
        match self {
            Self::MeitY => MEITY_PIPELINE_ID,
            Self::AI4Bharat => AI4BHARAT_PIPELINE_ID,
        }
    }

    /// Get the display name.
    #[inline]
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::MeitY => "MeitY",
            Self::AI4Bharat => "AI4Bharat",
        }
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to Bhashini STT (ASR) API.
///
/// This configuration extends the base `STTConfig` with Bhashini-specific
/// parameters for the pipeline-based REST API.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::stt::bhashini::{BhashiniSttConfig, BhashiniLanguage};
/// use waav_gateway::core::stt::STTConfig;
///
/// let config = BhashiniSttConfig::from_base(STTConfig {
///     api_key: "userId|ulcaApiKey|inferenceApiKey".to_string(),
///     language: "hi".to_string(),
///     sample_rate: 16000,
///     ..Default::default()
/// }).unwrap();
/// ```
#[derive(Debug, Clone)]
pub struct BhashiniSttConfig {
    /// User ID from ULCA profile.
    pub user_id: String,

    /// ULCA API key for pipeline config calls.
    pub ulca_api_key: String,

    /// Inference API key (obtained from pipeline config response, or pre-configured).
    pub inference_api_key: Option<String>,

    /// Source language for ASR.
    pub language: BhashiniLanguage,

    /// Audio format.
    pub audio_format: BhashiniAudioFormat,

    /// Sample rate in Hz.
    pub sample_rate: u32,

    /// Pipeline provider to use.
    pub pipeline_provider: BhashiniPipelineProvider,

    /// Custom service ID (overrides auto-detection).
    pub custom_service_id: Option<String>,

    /// Custom callback URL (overrides pipeline config response).
    pub custom_callback_url: Option<String>,

    /// Source script code (ULCA `sourceScriptCode`, ISO-15924). When set, the ASR
    /// pipeline-config request asks for output transliterated into this script
    /// (e.g. `"Latn"` for Latin script). Carried via the standardized extras passthrough.
    pub source_script_code: Option<String>,

    /// ASR post-processors (ULCA `postProcessors`, e.g. `["itn"]` for inverse text
    /// normalization). Applied server-side to the recognized text. Carried via extras.
    pub post_processors: Option<Vec<String>>,

    /// Maximum audio buffer size in bytes.
    pub max_buffer_size: usize,

    /// Test-only base-URL override for the pipeline CONFIG POST (scheme+host swap; path/query kept).
    pub endpoint_override: Option<String>,
}

impl Default for BhashiniSttConfig {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            ulca_api_key: String::new(),
            inference_api_key: None,
            language: BhashiniLanguage::default(),
            audio_format: BhashiniAudioFormat::default(),
            sample_rate: DEFAULT_SAMPLE_RATE,
            pipeline_provider: BhashiniPipelineProvider::default(),
            custom_service_id: None,
            custom_callback_url: None,
            source_script_code: None,
            post_processors: None,
            max_buffer_size: MAX_AUDIO_SIZE_BYTES,
            endpoint_override: None,
        }
    }
}

impl BhashiniSttConfig {
    /// Create configuration from base STTConfig.
    ///
    /// The API key should be in format: `userId|ulcaApiKey` or `userId|ulcaApiKey|inferenceApiKey`
    pub fn from_base(base: STTConfig) -> Result<Self, String> {
        // Parse credentials from api_key field
        // Format: "userId|ulcaApiKey" or "userId|ulcaApiKey|inferenceApiKey"
        let parts: Vec<&str> = base.api_key.split('|').collect();
        if parts.len() < 2 {
            return Err(
                "Invalid API key format. Expected: userId|ulcaApiKey or userId|ulcaApiKey|inferenceApiKey".to_string()
            );
        }

        let user_id = parts[0].trim().to_string();
        let ulca_api_key = parts[1].trim().to_string();
        let inference_api_key = if parts.len() >= 3 {
            Some(parts[2].trim().to_string())
        } else {
            None
        };

        if user_id.is_empty() || ulca_api_key.is_empty() {
            return Err("User ID and ULCA API key are required".to_string());
        }

        // Parse language
        let language = if base.language.is_empty() {
            BhashiniLanguage::default()
        } else {
            BhashiniLanguage::from_str_or_default(&base.language)
        };

        // Parse audio format from encoding
        let audio_format = if base.encoding.is_empty() {
            BhashiniAudioFormat::default()
        } else {
            BhashiniAudioFormat::from_str_or_default(&base.encoding)
        };

        // Parse sample rate
        let sample_rate = if base.sample_rate == 0 {
            DEFAULT_SAMPLE_RATE
        } else {
            base.sample_rate
        };

        // Parse pipeline provider from model field
        let pipeline_provider = match base.model.to_lowercase().as_str() {
            "ai4bharat" | "a4b" => BhashiniPipelineProvider::AI4Bharat,
            _ => BhashiniPipelineProvider::MeitY,
        };

        let config = Self {
            user_id,
            ulca_api_key,
            inference_api_key,
            language,
            audio_format,
            sample_rate,
            pipeline_provider,
            custom_service_id: None,
            custom_callback_url: None,
            source_script_code: None,
            post_processors: None,
            max_buffer_size: MAX_AUDIO_SIZE_BYTES,
            endpoint_override: None,
        };

        config.validate()?;
        Ok(config)
    }

    /// Build from the standardized config (W1 keystone). None of the *typed* `SttFeatures`
    /// (interim_results, diarization, word_timestamps, smart_format, profanity_filter,
    /// filler_words, vad_events, endpointing, utterance_end, keyterms, redaction,
    /// entity_detection, language_detection) maps to a real Bhashini knob — those stay at
    /// provider defaults (capability gaps for a batch ULCA ASR pipeline).
    ///
    /// Two genuinely Bhashini-specific ULCA pipeline knobs that DO exist are carried through the
    /// open [`ProviderExtras`] passthrough and reach `pipelineTasks[].config` on the wire:
    /// - `sourceScriptCode` (ISO-15924) → `config.language.sourceScriptCode`
    /// - `postProcessors` (e.g. `["itn"]`) → `config.postProcessors`
    ///
    /// snake_case aliases (`source_script_code`, `post_processors`) are also accepted.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, String> {
        let mut cfg = Self::from_base(std.base.clone())?;
        let extras = &std.extras.0;

        if let Some(code) = extras
            .get("sourceScriptCode")
            .or_else(|| extras.get("source_script_code"))
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            cfg.source_script_code = Some(code.to_string());
        }

        if let Some(arr) = extras
            .get("postProcessors")
            .or_else(|| extras.get("post_processors"))
            .and_then(|v| v.as_array())
        {
            let procs: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            if !procs.is_empty() {
                cfg.post_processors = Some(procs);
            }
        }

        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());

        cfg.validate()?;

        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        if self.user_id.is_empty() {
            return Err("User ID is required".to_string());
        }

        if self.ulca_api_key.is_empty() {
            return Err("ULCA API key is required".to_string());
        }

        if self.sample_rate < MIN_SAMPLE_RATE {
            return Err(format!(
                "Sample rate must be at least {} Hz, got {}",
                MIN_SAMPLE_RATE, self.sample_rate
            ));
        }

        if let Some(endpoint) = &self.endpoint_override {
            validate_bhashini_stt_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Get the service ID for ASR (auto-detect based on language or use custom).
    pub fn service_id(&self) -> &str {
        self.custom_service_id
            .as_deref()
            .unwrap_or_else(|| self.language.asr_service_id())
    }

    /// Get the pipeline ID.
    pub fn pipeline_id(&self) -> &str {
        self.pipeline_provider.pipeline_id()
    }

    /// Get the compute URL (custom or default).
    pub fn compute_url(&self) -> &str {
        self.custom_callback_url
            .as_deref()
            .unwrap_or(BHASHINI_COMPUTE_URL)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: Bhashini maps zero standardized features (batch ULCA provider with no advanced
    // knobs), so from_standard is a pure from_base passthrough — assert it succeeds and the base
    // credentials (parsed from api_key) carry through unchanged.
    #[test]
    fn from_standard_passthrough_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig, SttFeatures};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "bhashini".into(),
                api_key: "user123|apikey456".into(),
                language: "hi".into(),
                ..Default::default()
            },
            features: SttFeatures {
                // None of these can map to a real Bhashini field; they must be ignored.
                diarization: Some(true),
                word_timestamps: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = BhashiniSttConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.user_id, "user123");
        assert_eq!(cfg.ulca_api_key, "apikey456");
        assert_eq!(cfg.language, BhashiniLanguage::Hindi);
        // No extras -> advanced knobs stay unset.
        assert!(cfg.source_script_code.is_none());
        assert!(cfg.post_processors.is_none());
    }

    // WIRE-LEVEL (end-to-end): the standardized extras (`sourceScriptCode`, `postProcessors`)
    // map onto the Bhashini config AND reach the serialized pipeline-config request body — the
    // same `PipelineConfigRequest::new_asr_with` call the live `fetch_pipeline_config` makes.
    #[test]
    fn extras_reach_pipeline_config_request_body() {
        use super::super::messages::PipelineConfigRequest;
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};

        let mut extras = serde_json::Map::new();
        extras.insert("sourceScriptCode".into(), serde_json::json!("Latn"));
        extras.insert("postProcessors".into(), serde_json::json!(["itn"]));

        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "bhashini".into(),
                api_key: "user123|apikey456".into(),
                language: "hi".into(),
                ..Default::default()
            },
            features: Default::default(),
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = BhashiniSttConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.source_script_code.as_deref(), Some("Latn"));
        assert_eq!(cfg.post_processors, Some(vec!["itn".to_string()]));

        // Build the actual request the client sends and assert the wire body.
        let request = PipelineConfigRequest::new_asr_with(
            cfg.language.as_code(),
            cfg.pipeline_id(),
            cfg.source_script_code.clone(),
            cfg.post_processors.clone(),
        );
        let json = serde_json::to_string(&request).unwrap();
        assert!(
            json.contains("\"sourceScriptCode\":\"Latn\""),
            "sourceScriptCode missing from request body: {json}"
        );
        assert!(
            json.contains("\"postProcessors\":[\"itn\"]"),
            "postProcessors missing from request body: {json}"
        );
    }

    // snake_case aliases are also accepted (ergonomic passthrough).
    #[test]
    fn from_standard_accepts_snake_case_extras() {
        use crate::core::stt::standard::{ProviderExtras, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("source_script_code".into(), serde_json::json!("Deva"));
        extras.insert("post_processors".into(), serde_json::json!(["itn"]));
        let std = StandardSTTConfig {
            base: STTConfig {
                api_key: "u|k".into(),
                language: "hi".into(),
                ..Default::default()
            },
            features: Default::default(),
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = BhashiniSttConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.source_script_code.as_deref(), Some("Deva"));
        assert_eq!(cfg.post_processors, Some(vec!["itn".to_string()]));
    }

    #[test]
    fn test_language_codes() {
        assert_eq!(BhashiniLanguage::Hindi.as_code(), "hi");
        assert_eq!(BhashiniLanguage::Tamil.as_code(), "ta");
        assert_eq!(BhashiniLanguage::Telugu.as_code(), "te");
        assert_eq!(BhashiniLanguage::Bengali.as_code(), "bn");
        assert_eq!(BhashiniLanguage::English.as_code(), "en");
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(
            BhashiniLanguage::from_str_or_default("hi"),
            BhashiniLanguage::Hindi
        );
        assert_eq!(
            BhashiniLanguage::from_str_or_default("hindi"),
            BhashiniLanguage::Hindi
        );
        assert_eq!(
            BhashiniLanguage::from_str_or_default("ta"),
            BhashiniLanguage::Tamil
        );
        assert_eq!(
            BhashiniLanguage::from_str_or_default("tamil"),
            BhashiniLanguage::Tamil
        );
        assert_eq!(
            BhashiniLanguage::from_str_or_default("unknown"),
            BhashiniLanguage::default()
        );
    }

    #[test]
    fn test_language_families() {
        assert_eq!(BhashiniLanguage::Tamil.family(), LanguageFamily::Dravidian);
        assert_eq!(BhashiniLanguage::Telugu.family(), LanguageFamily::Dravidian);
        assert_eq!(BhashiniLanguage::Hindi.family(), LanguageFamily::IndoAryan);
        assert_eq!(
            BhashiniLanguage::Bengali.family(),
            LanguageFamily::IndoAryan
        );
        assert_eq!(BhashiniLanguage::English.family(), LanguageFamily::Misc);
    }

    #[test]
    fn test_service_id_selection() {
        // Dravidian languages
        assert!(
            BhashiniLanguage::Tamil
                .asr_service_id()
                .contains("dravidian")
        );
        assert!(
            BhashiniLanguage::Telugu
                .asr_service_id()
                .contains("dravidian")
        );

        // Indo-Aryan languages
        assert!(BhashiniLanguage::Hindi.asr_service_id().contains("hi-gpu"));
        assert!(
            BhashiniLanguage::Bengali
                .asr_service_id()
                .contains("indo_aryan")
        );

        // English
        assert!(
            BhashiniLanguage::English
                .asr_service_id()
                .contains("whisper")
        );
    }

    #[test]
    fn test_audio_format() {
        assert_eq!(BhashiniAudioFormat::Wav.as_str(), "wav");
        assert_eq!(BhashiniAudioFormat::Flac.as_str(), "flac");
        assert_eq!(BhashiniAudioFormat::Mp3.as_str(), "mp3");

        assert_eq!(BhashiniAudioFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(BhashiniAudioFormat::Flac.mime_type(), "audio/flac");
        assert_eq!(BhashiniAudioFormat::Mp3.mime_type(), "audio/mpeg");
    }

    #[test]
    fn test_pipeline_provider() {
        assert_eq!(
            BhashiniPipelineProvider::MeitY.pipeline_id(),
            MEITY_PIPELINE_ID
        );
        assert_eq!(
            BhashiniPipelineProvider::AI4Bharat.pipeline_id(),
            AI4BHARAT_PIPELINE_ID
        );
    }

    #[test]
    fn test_config_from_base_valid() {
        let base = STTConfig {
            api_key: "user123|apikey456".to_string(),
            language: "hi".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let config = BhashiniSttConfig::from_base(base).unwrap();
        assert_eq!(config.user_id, "user123");
        assert_eq!(config.ulca_api_key, "apikey456");
        assert!(config.inference_api_key.is_none());
        assert_eq!(config.language, BhashiniLanguage::Hindi);
    }

    #[test]
    fn test_config_from_base_with_inference_key() {
        let base = STTConfig {
            api_key: "user123|apikey456|inferkey789".to_string(),
            language: "ta".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let config = BhashiniSttConfig::from_base(base).unwrap();
        assert_eq!(config.user_id, "user123");
        assert_eq!(config.ulca_api_key, "apikey456");
        assert_eq!(config.inference_api_key, Some("inferkey789".to_string()));
        assert_eq!(config.language, BhashiniLanguage::Tamil);
    }

    #[test]
    fn test_config_from_base_invalid_format() {
        let base = STTConfig {
            api_key: "invalid_no_separator".to_string(),
            ..Default::default()
        };

        let result = BhashiniSttConfig::from_base(base);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid API key format"));
    }

    #[test]
    fn test_config_from_base_empty_user_id() {
        let base = STTConfig {
            api_key: "|apikey456".to_string(),
            ..Default::default()
        };

        let result = BhashiniSttConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validation() {
        let mut config = BhashiniSttConfig::default();
        assert!(config.validate().is_err()); // Empty user_id

        config.user_id = "user123".to_string();
        assert!(config.validate().is_err()); // Empty ulca_api_key

        config.ulca_api_key = "apikey456".to_string();
        assert!(config.validate().is_ok());

        config.sample_rate = 1000;
        assert!(config.validate().is_err()); // Below minimum
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = BhashiniSttConfig {
            user_id: "user123".to_string(),
            ulca_api_key: "apikey456".to_string(),
            ..Default::default()
        };

        config.endpoint_override = Some("https://bhashini-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("non-HTTP endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"), "{err}");
    }

    #[test]
    fn test_config_defaults() {
        let base = STTConfig {
            api_key: "user123|apikey456".to_string(),
            language: String::new(),
            encoding: String::new(),
            sample_rate: 0,
            ..Default::default()
        };

        let config = BhashiniSttConfig::from_base(base).unwrap();
        assert_eq!(config.language, BhashiniLanguage::Hindi); // Default
        assert_eq!(config.audio_format, BhashiniAudioFormat::Wav); // Default
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE); // Default
    }

    #[test]
    fn test_all_languages() {
        let languages = BhashiniLanguage::all();
        assert_eq!(languages.len(), 24);
        assert!(languages.contains(&BhashiniLanguage::Hindi));
        assert!(languages.contains(&BhashiniLanguage::Tamil));
        assert!(languages.contains(&BhashiniLanguage::English));
    }

    #[test]
    fn test_constants() {
        assert_eq!(
            BHASHINI_CONFIG_URL,
            "https://meity-auth.ulcacontrib.org/ulca/apis/v0/model/getModelsPipeline"
        );
        assert_eq!(
            BHASHINI_COMPUTE_URL,
            "https://dhruva-api.bhashini.gov.in/services/inference/pipeline"
        );
        assert_eq!(DEFAULT_SAMPLE_RATE, 16000);
        assert_eq!(MIN_SAMPLE_RATE, 8000);
    }
}
