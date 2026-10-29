//! Gnani.ai TTS Configuration
//!
//! Configuration types for Gnani's Text-to-Speech API with support for
//! 7 Indian and English languages, multi-speaker voices, and SSML gender.

use crate::core::tts::base::TTSConfig;
use serde::{Deserialize, Serialize};

fn validate_gnani_tts_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

/// Gnani TTS provider-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GnaniTTSConfig {
    /// Base TTS configuration
    #[serde(flatten)]
    pub base: TTSConfig,

    /// Gnani API key, sent as `X-API-Key-ID`.
    #[serde(default, skip_serializing)]
    pub api_key: String,

    /// The Timbre model; its voices are its own (a voice of the other model is a 400).
    #[serde(default)]
    pub model: GnaniModel,

    /// Speed multiplier (Gnani: 0.85 to 1.15).
    #[serde(default = "default_speed")]
    pub speed: f32,

    /// Language code for TTS
    #[serde(default)]
    pub language_code: GnaniTTSLanguage,

    /// Voice name from the model's catalog; the model's default for the gender when unset
    #[serde(default)]
    pub voice_name: Option<String>,

    /// Gender of the default voice when no voice is named
    #[serde(default)]
    pub ssml_gender: GnaniGender,

    /// Output sample rate in Hz (default: 8000; Gnani: 8000, 16000, 22050, 24000, 44100, 48000)
    #[serde(default = "default_sample_rate")]
    pub output_sample_rate: u32,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    /// Test-only override base (scheme+host) for the synth endpoint; passthrough when None.
    #[serde(default, skip_serializing)]
    pub endpoint_override: Option<String>,
}

fn default_sample_rate() -> u32 {
    8000
}

fn default_request_timeout() -> u64 {
    30
}

fn default_speed() -> f32 {
    1.0
}

/// Gnani's text-to-speech endpoint (docs.gnani.ai, "Text-to-Speech (REST)"). The former
/// `asr.gnani.ai/synthesize` is not in Gnani's docs, and the host returns no HTTP response.
pub const GNANI_TTS_ENDPOINT: &str = "https://api.vachana.ai/api/v1/tts/inference";

/// The sample rates Gnani's `audio_config.sample_rate` accepts.
pub const GNANI_SAMPLE_RATES: &[u32] = &[8000, 16000, 22050, 24000, 44100, 48000];

/// Timbre v3.0's voices (docs.gnani.ai, "Available Voices"): every one speaks all ten languages.
pub const TIMBRE_V3_VOICES: &[(&str, GnaniGender)] = &[
    ("Shivani", GnaniGender::Female),
    ("Lakshmi", GnaniGender::Female),
    ("Kalpana", GnaniGender::Female),
    ("Chetana", GnaniGender::Female),
    ("Vasudha", GnaniGender::Female),
    ("Riyaa", GnaniGender::Female),
    ("Tarika", GnaniGender::Female),
    ("Avinash", GnaniGender::Male),
    ("Shubham", GnaniGender::Male),
    ("Prakash", GnaniGender::Male),
    ("Devansh", GnaniGender::Male),
    ("Yashraj", GnaniGender::Male),
    ("Harshad", GnaniGender::Male),
];

/// Timbre v2.5's voices (the same catalog); each has a preferred language.
pub const TIMBRE_V25_VOICES: &[&str] = &[
    "Ambuja", "Asmita", "Bhavna", "Brinda", "Chitra", "Deepak", "Devika", "Dhruva", "Falak",
    "Girish", "Hemraj", "Ishaan", "Jalaj", "Jwala", "Kaveri", "Kavin", "Kirra", "Lavanya",
    "Lehara", "Mehuli", "Nalini", "Noopur", "Omkar", "Poorvi", "Pranav", "Reshma", "Riyaan",
    "Roopesh", "Saanvi", "Shlok", "Suhana", "Trisha", "Trupti", "Urmila", "Varuni", "Vedika",
    "Veera", "Vikrant", "Yashvi", "Yukti", "Zahira", "Zayan",
];

/// Gnani's text-to-speech models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GnaniModel {
    /// Timbre v3.0 (13 voices, each speaks all ten languages).
    #[default]
    #[serde(rename = "timbre-v3.0")]
    TimbreV30,
    /// Timbre v2.5 (42 voices, one preferred language each).
    #[serde(rename = "timbre-v2.5")]
    TimbreV25,
}

impl GnaniModel {
    /// The `model` value sent to the API.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TimbreV30 => "timbre-v3.0",
            Self::TimbreV25 => "timbre-v2.5",
        }
    }

    /// Parse a model name.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "timbre-v3.0" | "timbre-v3" | "v3.0" | "v3" => Some(Self::TimbreV30),
            "timbre-v2.5" | "v2.5" => Some(Self::TimbreV25),
            _ => None,
        }
    }

    /// The model whose catalog has `voice` (case-insensitive), with the voice as Gnani spells it.
    pub fn of_voice(voice: &str) -> Option<(Self, &'static str)> {
        let voice = voice.trim();
        TIMBRE_V3_VOICES
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(voice))
            .map(|(name, _)| (Self::TimbreV30, *name))
            .or_else(|| {
                TIMBRE_V25_VOICES
                    .iter()
                    .find(|name| name.eq_ignore_ascii_case(voice))
                    .map(|name| (Self::TimbreV25, *name))
            })
    }

    /// The voice a session that names none speaks with: for v3.0 by gender; for v2.5 the
    /// language's preferred voice of that gender (else of the other; the catalog's preferred
    /// Tamil, Telugu and Hinglish voices are all female).
    pub fn default_voice(&self, language: GnaniTTSLanguage, gender: GnaniGender) -> &'static str {
        match self {
            Self::TimbreV30 => match gender {
                GnaniGender::Female => "Shivani",
                GnaniGender::Male => "Avinash",
            },
            Self::TimbreV25 => {
                let (female, male) = match language {
                    GnaniTTSLanguage::EnglishIndia => ("Kaveri", Some("Pranav")),
                    GnaniTTSLanguage::Hindi | GnaniTTSLanguage::HindiAlt => {
                        ("Nalini", Some("Deepak"))
                    }
                    GnaniTTSLanguage::Tamil => ("Asmita", None),
                    GnaniTTSLanguage::Telugu => ("Suhana", None),
                    GnaniTTSLanguage::Kannada => ("Saanvi", Some("Kavin")),
                    GnaniTTSLanguage::Malayalam => ("Reshma", Some("Riyaan")),
                    GnaniTTSLanguage::Marathi => ("Zahira", Some("Ishaan")),
                    GnaniTTSLanguage::Bengali => ("Kirra", Some("Dhruva")),
                    GnaniTTSLanguage::Gujarati => ("Falak", Some("Veera")),
                    GnaniTTSLanguage::Punjabi => ("Mehuli", Some("Zayan")),
                    GnaniTTSLanguage::Nepali => ("Nalini", Some("Deepak")),
                };
                match gender {
                    GnaniGender::Male => male.unwrap_or(female),
                    GnaniGender::Female => female,
                }
            }
        }
    }
}

impl Default for GnaniTTSConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig {
                provider: "gnani".to_string(),
                api_key: String::new(),
                voice_id: None,
                model: "default".to_string(),
                speaking_rate: Some(1.0),
                audio_format: Some("pcm16".to_string()),
                sample_rate: Some(8000),
                connection_timeout: Some(10),
                request_timeout: Some(30),
                pronunciations: Vec::new(),
                request_pool_size: None,
                emotion_config: None,
                api_base: None,
            },
            api_key: String::new(),
            model: GnaniModel::default(),
            speed: default_speed(),
            language_code: GnaniTTSLanguage::default(),
            voice_name: None,
            ssml_gender: GnaniGender::default(),
            output_sample_rate: default_sample_rate(),
            request_timeout_secs: default_request_timeout(),
            endpoint_override: None,
        }
    }
}

impl GnaniTTSConfig {
    /// Create GnaniTTSConfig from base TTSConfig
    pub fn from_base(base: TTSConfig) -> Result<Self, String> {
        // One key, sent as `X-API-Key-ID`: the standardized `api_key` (the server's `GNANI_TOKEN`),
        // else `GNANI_TOKEN`. A `token|access_key` pair from the former API gives its first half.
        let api_key = crate::core::credentials::explicit_api_key(&base.api_key)
            .map(|key| match key.split_once('|') {
                Some((token, _)) => token.trim().to_string(),
                None => key,
            })
            .filter(|key| !key.is_empty())
            .or_else(|| crate::core::credentials::env_api_key("GNANI_TOKEN"))
            .unwrap_or_default();

        // `model` names the Timbre model, or (the former API's convention) a gender.
        let named_model = GnaniModel::from_str(&base.model);
        let ssml_gender = if named_model.is_none()
            && base.model.to_uppercase().contains("MALE")
            && !base.model.to_uppercase().contains("FEMALE")
        {
            GnaniGender::Male
        } else {
            GnaniGender::Female
        };

        // `voice_id` is a language code (the former API's convention) or a voice name; a voice
        // name selects its own model, since a voice of the other model is refused.
        let mut language_code = GnaniTTSLanguage::default();
        let mut voice_name = None;
        let mut model = named_model.unwrap_or_default();
        if let Some(voice) = base
            .voice_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if let Ok(language) = GnaniTTSLanguage::from_str(voice) {
                language_code = language;
            } else if let Some((voice_model, name)) = GnaniModel::of_voice(voice) {
                model = voice_model;
                voice_name = Some(name.to_string());
            } else {
                voice_name = Some(voice.to_string());
            }
        }

        let output_sample_rate = base.sample_rate.unwrap_or(8000);
        let speed = base.speaking_rate.unwrap_or(1.0);

        Ok(Self {
            base,
            api_key,
            model,
            speed,
            language_code,
            voice_name,
            ssml_gender,
            output_sample_rate,
            request_timeout_secs: default_request_timeout(),
            endpoint_override: None,
        })
    }

    /// The voice sent: the one named, else the model's default for the language and gender.
    pub fn resolved_voice(&self) -> String {
        self.voice_name.clone().unwrap_or_else(|| {
            self.model
                .default_voice(self.language_code, self.ssml_gender)
                .to_string()
        })
    }

    /// Build from the standardized TTS config (W1 keystone). Gnani's request surface is limited to
    /// language selection, SSML gender and the output sample rate, so this maps the standardized
    /// features that have a real field: `language` -> `language_code` (parsed, falling back to the
    /// `from_base` value on an unrecognized code) and `sample_rate` -> `output_sample_rate`.
    /// Gnani has no speed/pitch/volume/stability/emotion/instructions/SSML-input knobs
    /// (`ssml_gender` is voice gender, not the `ssml` text-mode flag), so those features are
    /// skipped. The non-standard `voice_name` is read from the `extras` passthrough.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, String> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(language) = f.language.as_deref()
            && let Ok(lang) = GnaniTTSLanguage::from_str(language)
        {
            cfg.language_code = lang;
        }
        if let Some(rate) = f.sample_rate {
            cfg.output_sample_rate = rate;
        }

        // Provider-specific passthrough.
        if let Some(voice_name) = std.extras.0.get("voice_name").and_then(|v| v.as_str()) {
            match GnaniModel::of_voice(voice_name) {
                Some((model, name)) => {
                    cfg.model = model;
                    cfg.voice_name = Some(name.to_string());
                }
                None => cfg.voice_name = Some(voice_name.to_string()),
            }
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);

        if let Some(endpoint) = &cfg.endpoint_override {
            validate_gnani_tts_endpoint("endpoint_override", endpoint)?;
        }

        Ok(cfg)
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err(
                "Gnani API key is required. Set api_key, or the GNANI_TOKEN environment variable."
                    .to_string(),
            );
        }

        if self.language_code.api_code().is_none() {
            return Err(format!(
                "Gnani text-to-speech has no {} voice; its languages are Hindi, English, Tamil, \
                 Telugu, Kannada, Malayalam, Marathi, Punjabi, Bengali and Gujarati",
                self.language_code.display_name()
            ));
        }

        if !GNANI_SAMPLE_RATES.contains(&self.output_sample_rate) {
            return Err(format!(
                "Gnani sample rate {} is not supported; use one of {GNANI_SAMPLE_RATES:?}",
                self.output_sample_rate
            ));
        }

        if let Some(endpoint) = &self.endpoint_override {
            validate_gnani_tts_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Get the TTS API endpoint
    pub fn endpoint(&self) -> &'static str {
        GNANI_TTS_ENDPOINT
    }
}

/// SSML Gender for voice selection
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GnaniGender {
    /// Female voice
    #[default]
    #[serde(rename = "FEMALE")]
    Female,
    /// Male voice
    #[serde(rename = "MALE")]
    Male,
}

impl GnaniGender {
    /// Get the gender string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Female => "FEMALE",
            Self::Male => "MALE",
        }
    }

    /// Parse from string
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_uppercase().as_str() {
            "FEMALE" | "F" => Ok(Self::Female),
            "MALE" | "M" => Ok(Self::Male),
            _ => Err(format!("Invalid gender: {}. Use MALE or FEMALE", s)),
        }
    }
}

/// Supported languages for Gnani TTS (7 languages with single/multi-speaker)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GnaniTTSLanguage {
    /// English - India (En-IN) - supports single and multi-speaker
    #[default]
    #[serde(rename = "En-IN")]
    EnglishIndia,
    /// Hindi (Hi-IN) - supports single and multi-speaker
    #[serde(rename = "Hi-IN")]
    Hindi,
    /// Hindi Alternate (Hi-IN-al)
    #[serde(rename = "Hi-IN-al")]
    HindiAlt,
    /// Kannada (Kn-IN) - supports single and multi-speaker
    #[serde(rename = "Kn-IN")]
    Kannada,
    /// Tamil (Ta-IN) - supports single and multi-speaker
    #[serde(rename = "Ta-IN")]
    Tamil,
    /// Telugu (Te-IN) - supports single and multi-speaker
    #[serde(rename = "Te-IN")]
    Telugu,
    /// Marathi (Mr-IN) - Female only for single-speaker
    #[serde(rename = "Mr-IN")]
    Marathi,
    /// Malayalam (Ml-IN) - multi-speaker only
    #[serde(rename = "Ml-IN")]
    Malayalam,
    /// Gujarati (Gu-IN) - multi-speaker only
    #[serde(rename = "Gu-IN")]
    Gujarati,
    /// Bengali (Bn-IN) - multi-speaker only
    #[serde(rename = "Bn-IN")]
    Bengali,
    /// Punjabi (Pa-IN) - multi-speaker only
    #[serde(rename = "Pa-IN")]
    Punjabi,
    /// Nepali (Ne-NP) - multi-speaker only
    #[serde(rename = "Ne-NP")]
    Nepali,
}

impl GnaniTTSLanguage {
    /// Get the language code string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EnglishIndia => "En-IN",
            Self::Hindi => "Hi-IN",
            Self::HindiAlt => "Hi-IN-al",
            Self::Kannada => "Kn-IN",
            Self::Tamil => "Ta-IN",
            Self::Telugu => "Te-IN",
            Self::Marathi => "Mr-IN",
            Self::Malayalam => "Ml-IN",
            Self::Gujarati => "Gu-IN",
            Self::Bengali => "Bn-IN",
            Self::Punjabi => "Pa-IN",
            Self::Nepali => "Ne-NP",
        }
    }

    /// The `language` value the API takes (`hi-IN`, ...): Gnani speaks ten languages, and the
    /// former alternate Hindi is Hindi; Nepali has no voice.
    pub fn api_code(&self) -> Option<&'static str> {
        Some(match self {
            Self::EnglishIndia => "en-IN",
            Self::Hindi | Self::HindiAlt => "hi-IN",
            Self::Kannada => "kn-IN",
            Self::Tamil => "ta-IN",
            Self::Telugu => "te-IN",
            Self::Marathi => "mr-IN",
            Self::Malayalam => "ml-IN",
            Self::Gujarati => "gu-IN",
            Self::Bengali => "bn-IN",
            Self::Punjabi => "pa-IN",
            Self::Nepali => return None,
        })
    }

    /// Parse language code string to enum
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "en-in" | "english" | "english-india" => Ok(Self::EnglishIndia),
            "hi-in" | "hindi" => Ok(Self::Hindi),
            "hi-in-al" | "hindi-alt" => Ok(Self::HindiAlt),
            "kn-in" | "kannada" => Ok(Self::Kannada),
            "ta-in" | "tamil" => Ok(Self::Tamil),
            "te-in" | "telugu" => Ok(Self::Telugu),
            "mr-in" | "marathi" => Ok(Self::Marathi),
            "ml-in" | "malayalam" => Ok(Self::Malayalam),
            "gu-in" | "gujarati" => Ok(Self::Gujarati),
            "bn-in" | "bengali" => Ok(Self::Bengali),
            "pa-in" | "punjabi" => Ok(Self::Punjabi),
            "ne-np" | "nepali" => Ok(Self::Nepali),
            _ => Err(format!(
                "Unsupported Gnani TTS language: {}. Supported: En-IN, Hi-IN, Kn-IN, Ta-IN, Te-IN, Mr-IN, Ml-IN, Gu-IN, Bn-IN, Pa-IN, Ne-NP",
                s
            )),
        }
    }

    /// Get all supported language codes
    pub fn all_codes() -> &'static [&'static str] {
        &[
            "En-IN", "Hi-IN", "Hi-IN-al", "Kn-IN", "Ta-IN", "Te-IN", "Mr-IN", "Ml-IN", "Gu-IN",
            "Bn-IN", "Pa-IN", "Ne-NP",
        ]
    }

    /// Check if this language supports single-speaker voices
    pub fn supports_single_speaker(&self) -> bool {
        matches!(
            self,
            Self::EnglishIndia
                | Self::Hindi
                | Self::HindiAlt
                | Self::Kannada
                | Self::Tamil
                | Self::Telugu
                | Self::Marathi
        )
    }

    /// Check if this language supports multi-speaker voices
    pub fn supports_multi_speaker(&self) -> bool {
        // All languages support multi-speaker
        true
    }

    /// Get supported genders for single-speaker mode
    pub fn supported_genders(&self) -> &'static [GnaniGender] {
        match self {
            Self::Marathi => &[GnaniGender::Female],
            Self::EnglishIndia
            | Self::Hindi
            | Self::HindiAlt
            | Self::Kannada
            | Self::Tamil
            | Self::Telugu => &[GnaniGender::Male, GnaniGender::Female],
            // Multi-speaker only languages
            _ => &[],
        }
    }

    /// Get display name for the language
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::EnglishIndia => "English (India)",
            Self::Hindi => "Hindi",
            Self::HindiAlt => "Hindi (Alternate)",
            Self::Kannada => "Kannada",
            Self::Tamil => "Tamil",
            Self::Telugu => "Telugu",
            Self::Marathi => "Marathi",
            Self::Malayalam => "Malayalam",
            Self::Gujarati => "Gujarati",
            Self::Bengali => "Bengali",
            Self::Punjabi => "Punjabi",
            Self::Nepali => "Nepali",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (TTS): the standardized features Gnani can express (language override, output
    // sample rate) reach their real fields, and the non-standard voice_name flows through the
    // extras passthrough. Features Gnani has no field for (e.g. ssml text-mode) are ignored.
    #[test]
    fn from_standard_maps_language_and_sample_rate() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("voice_name".into(), serde_json::json!("speaker-3"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "gnani".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                language: Some("Ta-IN".into()),
                sample_rate: Some(16000),
                ssml: Some(true), // capability gap: Gnani has no SSML text mode, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = GnaniTTSConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.language_code, GnaniTTSLanguage::Tamil);
        assert_eq!(cfg.output_sample_rate, 16000);
        assert_eq!(cfg.voice_name, Some("speaker-3".to_string())); // extras passthrough
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = GnaniTTSConfig {
            api_key: "test-api-key".to_string(),
            ..Default::default()
        };

        config.endpoint_override = Some("https://gnani-proxy.example.com".to_string());
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
    fn test_gnani_tts_language_from_str() {
        assert_eq!(
            GnaniTTSLanguage::from_str("Hi-IN").unwrap(),
            GnaniTTSLanguage::Hindi
        );
        assert_eq!(
            GnaniTTSLanguage::from_str("hindi").unwrap(),
            GnaniTTSLanguage::Hindi
        );
        assert_eq!(
            GnaniTTSLanguage::from_str("En-IN").unwrap(),
            GnaniTTSLanguage::EnglishIndia
        );
        assert!(GnaniTTSLanguage::from_str("invalid").is_err());
    }

    #[test]
    fn test_gnani_tts_language_as_str() {
        assert_eq!(GnaniTTSLanguage::Hindi.as_str(), "Hi-IN");
        assert_eq!(GnaniTTSLanguage::EnglishIndia.as_str(), "En-IN");
        assert_eq!(GnaniTTSLanguage::Kannada.as_str(), "Kn-IN");
    }

    #[test]
    fn test_gnani_gender() {
        assert_eq!(GnaniGender::Female.as_str(), "FEMALE");
        assert_eq!(GnaniGender::Male.as_str(), "MALE");
        assert_eq!(GnaniGender::default(), GnaniGender::Female);
    }

    #[test]
    fn test_gnani_gender_from_str() {
        assert_eq!(GnaniGender::from_str("MALE").unwrap(), GnaniGender::Male);
        assert_eq!(
            GnaniGender::from_str("female").unwrap(),
            GnaniGender::Female
        );
        assert_eq!(GnaniGender::from_str("M").unwrap(), GnaniGender::Male);
        assert_eq!(GnaniGender::from_str("F").unwrap(), GnaniGender::Female);
    }

    #[test]
    fn test_gnani_tts_language_single_speaker_support() {
        assert!(GnaniTTSLanguage::Hindi.supports_single_speaker());
        assert!(GnaniTTSLanguage::EnglishIndia.supports_single_speaker());
        assert!(!GnaniTTSLanguage::Malayalam.supports_single_speaker());
        assert!(!GnaniTTSLanguage::Bengali.supports_single_speaker());
    }

    #[test]
    fn test_gnani_tts_marathi_female_only() {
        let genders = GnaniTTSLanguage::Marathi.supported_genders();
        assert_eq!(genders.len(), 1);
        assert_eq!(genders[0], GnaniGender::Female);
    }

    /// One key, from `api_key` (a former `token|access_key` pair gives its token); the voice
    /// names its model; the former languages map onto the API's ten, and Nepali is refused.
    #[test]
    fn the_config_speaks_gnanis_current_api() {
        let base = |voice: &str, model: &str| TTSConfig {
            provider: "gnani".to_string(),
            api_key: "test-token|test-access".to_string(),
            voice_id: Some(voice.to_string()),
            model: model.to_string(),
            ..Default::default()
        };
        let cfg = GnaniTTSConfig::from_base(base("Hi-IN", "MALE")).unwrap();
        assert_eq!(cfg.api_key, "test-token");
        assert_eq!(cfg.model, GnaniModel::TimbreV30);
        assert_eq!(cfg.resolved_voice(), "Avinash");
        assert_eq!(cfg.language_code.api_code(), Some("hi-IN"));
        assert!(cfg.validate().is_ok());

        let cfg = GnaniTTSConfig::from_base(base("Ta-IN", "timbre-v2.5")).unwrap();
        assert_eq!(cfg.resolved_voice(), "Asmita");
        let cfg = GnaniTTSConfig::from_base(base("Kn-IN", "timbre-v2.5 MALE")).unwrap();
        assert_eq!(
            cfg.model,
            GnaniModel::TimbreV30,
            "an unrecognised model name is the default"
        );

        let cfg = GnaniTTSConfig::from_base(base("lakshmi", "")).unwrap();
        assert_eq!(
            (cfg.model, cfg.resolved_voice().as_str()),
            (GnaniModel::TimbreV30, "Lakshmi")
        );
        let cfg = GnaniTTSConfig::from_base(base("Deepak", "")).unwrap();
        assert_eq!(
            (cfg.model, cfg.resolved_voice().as_str()),
            (GnaniModel::TimbreV25, "Deepak")
        );

        assert_eq!(GnaniTTSLanguage::HindiAlt.api_code(), Some("hi-IN"));
        let nepali = GnaniTTSConfig::from_base(base("Ne-NP", "")).unwrap();
        assert!(nepali.validate().unwrap_err().contains("no Nepali voice"));

        let mut odd_rate = GnaniTTSConfig::from_base(base("Hi-IN", "")).unwrap();
        odd_rate.output_sample_rate = 11025;
        assert!(odd_rate.validate().unwrap_err().contains("11025"));
        assert_eq!(GnaniTTSConfig::default().endpoint(), GNANI_TTS_ENDPOINT);
    }
}
