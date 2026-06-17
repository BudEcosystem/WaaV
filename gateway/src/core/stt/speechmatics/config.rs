//! Speechmatics STT Configuration
//!
//! Configuration types for Speechmatics speech-to-text provider.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::core::stt::base::{STTConfig, STTError};

// =============================================================================
// Region
// =============================================================================

/// Speechmatics API region
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechmaticsRegion {
    /// European Union region (default)
    #[default]
    EU,
    /// United States region
    US,
}

impl SpeechmaticsRegion {
    /// Get the WebSocket URL for this region
    pub fn ws_url(&self) -> &'static str {
        match self {
            SpeechmaticsRegion::EU => super::SPEECHMATICS_WS_URL_EU,
            SpeechmaticsRegion::US => super::SPEECHMATICS_WS_URL_US,
        }
    }
}

impl fmt::Display for SpeechmaticsRegion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpeechmaticsRegion::EU => write!(f, "eu"),
            SpeechmaticsRegion::US => write!(f, "us"),
        }
    }
}

impl FromStr for SpeechmaticsRegion {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "eu" | "europe" | "european" => Ok(SpeechmaticsRegion::EU),
            "us" | "usa" | "united_states" | "united-states" => Ok(SpeechmaticsRegion::US),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid Speechmatics region: '{}'. Valid options: eu, us",
                s
            ))),
        }
    }
}

// =============================================================================
// Operating Point
// =============================================================================

/// Speechmatics operating point (accuracy vs latency tradeoff)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechmaticsOperatingPoint {
    /// Standard accuracy (lower latency)
    #[default]
    Standard,
    /// Enhanced accuracy (higher latency, better quality)
    Enhanced,
}

impl fmt::Display for SpeechmaticsOperatingPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpeechmaticsOperatingPoint::Standard => write!(f, "standard"),
            SpeechmaticsOperatingPoint::Enhanced => write!(f, "enhanced"),
        }
    }
}

impl FromStr for SpeechmaticsOperatingPoint {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "standard" | "std" => Ok(SpeechmaticsOperatingPoint::Standard),
            "enhanced" | "enh" | "high" => Ok(SpeechmaticsOperatingPoint::Enhanced),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid operating point: '{}'. Valid options: standard, enhanced",
                s
            ))),
        }
    }
}

// =============================================================================
// Audio Encoding
// =============================================================================

/// Speechmatics audio encoding format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpeechmaticsEncoding {
    /// 16-bit signed PCM, little-endian (default)
    #[default]
    #[serde(rename = "pcm_s16le")]
    PcmS16le,
    /// 32-bit float PCM, little-endian
    #[serde(rename = "pcm_f32le")]
    PcmF32le,
    /// mu-law encoding (8-bit)
    #[serde(rename = "mulaw")]
    Mulaw,
}

impl SpeechmaticsEncoding {
    /// Get the encoding name for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            SpeechmaticsEncoding::PcmS16le => "pcm_s16le",
            SpeechmaticsEncoding::PcmF32le => "pcm_f32le",
            SpeechmaticsEncoding::Mulaw => "mulaw",
        }
    }
}

impl fmt::Display for SpeechmaticsEncoding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SpeechmaticsEncoding {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pcm_s16le" | "pcms16le" | "linear16" | "s16le" => Ok(SpeechmaticsEncoding::PcmS16le),
            "pcm_f32le" | "pcmf32le" | "float32" | "f32le" => Ok(SpeechmaticsEncoding::PcmF32le),
            "mulaw" | "ulaw" | "mu-law" | "u-law" => Ok(SpeechmaticsEncoding::Mulaw),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid encoding: '{}'. Valid options: pcm_s16le, pcm_f32le, mulaw",
                s
            ))),
        }
    }
}

// =============================================================================
// Language
// =============================================================================

/// Speechmatics supported languages (55+ languages)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SpeechmaticsLanguage {
    /// Automatic language detection
    Auto,
    /// Arabic
    #[serde(rename = "ar")]
    Arabic,
    /// Bashkir
    #[serde(rename = "ba")]
    Bashkir,
    /// Basque
    #[serde(rename = "eu")]
    Basque,
    /// Belarusian
    #[serde(rename = "be")]
    Belarusian,
    /// Bengali
    #[serde(rename = "bn")]
    Bengali,
    /// Bulgarian
    #[serde(rename = "bg")]
    Bulgarian,
    /// Cantonese
    #[serde(rename = "yue")]
    Cantonese,
    /// Catalan
    #[serde(rename = "ca")]
    Catalan,
    /// Croatian
    #[serde(rename = "hr")]
    Croatian,
    /// Czech
    #[serde(rename = "cs")]
    Czech,
    /// Danish
    #[serde(rename = "da")]
    Danish,
    /// Dutch
    #[serde(rename = "nl")]
    Dutch,
    /// English (default)
    #[default]
    #[serde(rename = "en")]
    English,
    /// Esperanto
    #[serde(rename = "eo")]
    Esperanto,
    /// Estonian
    #[serde(rename = "et")]
    Estonian,
    /// Finnish
    #[serde(rename = "fi")]
    Finnish,
    /// French
    #[serde(rename = "fr")]
    French,
    /// Galician
    #[serde(rename = "gl")]
    Galician,
    /// German
    #[serde(rename = "de")]
    German,
    /// Greek
    #[serde(rename = "el")]
    Greek,
    /// Hebrew
    #[serde(rename = "he")]
    Hebrew,
    /// Hindi
    #[serde(rename = "hi")]
    Hindi,
    /// Hungarian
    #[serde(rename = "hu")]
    Hungarian,
    /// Indonesian
    #[serde(rename = "id")]
    Indonesian,
    /// Interlingua
    #[serde(rename = "ia")]
    Interlingua,
    /// Irish
    #[serde(rename = "ga")]
    Irish,
    /// Italian
    #[serde(rename = "it")]
    Italian,
    /// Japanese
    #[serde(rename = "ja")]
    Japanese,
    /// Korean
    #[serde(rename = "ko")]
    Korean,
    /// Latvian
    #[serde(rename = "lv")]
    Latvian,
    /// Lithuanian
    #[serde(rename = "lt")]
    Lithuanian,
    /// Malay
    #[serde(rename = "ms")]
    Malay,
    /// Maltese
    #[serde(rename = "mt")]
    Maltese,
    /// Mandarin Chinese
    #[serde(rename = "cmn")]
    Mandarin,
    /// Marathi
    #[serde(rename = "mr")]
    Marathi,
    /// Mongolian
    #[serde(rename = "mn")]
    Mongolian,
    /// Norwegian
    #[serde(rename = "no")]
    Norwegian,
    /// Persian (Farsi)
    #[serde(rename = "fa")]
    Persian,
    /// Polish
    #[serde(rename = "pl")]
    Polish,
    /// Portuguese
    #[serde(rename = "pt")]
    Portuguese,
    /// Romanian
    #[serde(rename = "ro")]
    Romanian,
    /// Russian
    #[serde(rename = "ru")]
    Russian,
    /// Slovak
    #[serde(rename = "sk")]
    Slovak,
    /// Slovenian
    #[serde(rename = "sl")]
    Slovenian,
    /// Spanish
    #[serde(rename = "es")]
    Spanish,
    /// Swahili
    #[serde(rename = "sw")]
    Swahili,
    /// Swedish
    #[serde(rename = "sv")]
    Swedish,
    /// Tamil
    #[serde(rename = "ta")]
    Tamil,
    /// Thai
    #[serde(rename = "th")]
    Thai,
    /// Turkish
    #[serde(rename = "tr")]
    Turkish,
    /// Ukrainian
    #[serde(rename = "uk")]
    Ukrainian,
    /// Urdu
    #[serde(rename = "ur")]
    Urdu,
    /// Uyghur
    #[serde(rename = "ug")]
    Uyghur,
    /// Vietnamese
    #[serde(rename = "vi")]
    Vietnamese,
    /// Welsh
    #[serde(rename = "cy")]
    Welsh,
}

impl SpeechmaticsLanguage {
    /// Get the ISO language code
    pub fn as_str(&self) -> &'static str {
        match self {
            SpeechmaticsLanguage::Auto => "auto",
            SpeechmaticsLanguage::Arabic => "ar",
            SpeechmaticsLanguage::Bashkir => "ba",
            SpeechmaticsLanguage::Basque => "eu",
            SpeechmaticsLanguage::Belarusian => "be",
            SpeechmaticsLanguage::Bengali => "bn",
            SpeechmaticsLanguage::Bulgarian => "bg",
            SpeechmaticsLanguage::Cantonese => "yue",
            SpeechmaticsLanguage::Catalan => "ca",
            SpeechmaticsLanguage::Croatian => "hr",
            SpeechmaticsLanguage::Czech => "cs",
            SpeechmaticsLanguage::Danish => "da",
            SpeechmaticsLanguage::Dutch => "nl",
            SpeechmaticsLanguage::English => "en",
            SpeechmaticsLanguage::Esperanto => "eo",
            SpeechmaticsLanguage::Estonian => "et",
            SpeechmaticsLanguage::Finnish => "fi",
            SpeechmaticsLanguage::French => "fr",
            SpeechmaticsLanguage::Galician => "gl",
            SpeechmaticsLanguage::German => "de",
            SpeechmaticsLanguage::Greek => "el",
            SpeechmaticsLanguage::Hebrew => "he",
            SpeechmaticsLanguage::Hindi => "hi",
            SpeechmaticsLanguage::Hungarian => "hu",
            SpeechmaticsLanguage::Indonesian => "id",
            SpeechmaticsLanguage::Interlingua => "ia",
            SpeechmaticsLanguage::Irish => "ga",
            SpeechmaticsLanguage::Italian => "it",
            SpeechmaticsLanguage::Japanese => "ja",
            SpeechmaticsLanguage::Korean => "ko",
            SpeechmaticsLanguage::Latvian => "lv",
            SpeechmaticsLanguage::Lithuanian => "lt",
            SpeechmaticsLanguage::Malay => "ms",
            SpeechmaticsLanguage::Maltese => "mt",
            SpeechmaticsLanguage::Mandarin => "cmn",
            SpeechmaticsLanguage::Marathi => "mr",
            SpeechmaticsLanguage::Mongolian => "mn",
            SpeechmaticsLanguage::Norwegian => "no",
            SpeechmaticsLanguage::Persian => "fa",
            SpeechmaticsLanguage::Polish => "pl",
            SpeechmaticsLanguage::Portuguese => "pt",
            SpeechmaticsLanguage::Romanian => "ro",
            SpeechmaticsLanguage::Russian => "ru",
            SpeechmaticsLanguage::Slovak => "sk",
            SpeechmaticsLanguage::Slovenian => "sl",
            SpeechmaticsLanguage::Spanish => "es",
            SpeechmaticsLanguage::Swahili => "sw",
            SpeechmaticsLanguage::Swedish => "sv",
            SpeechmaticsLanguage::Tamil => "ta",
            SpeechmaticsLanguage::Thai => "th",
            SpeechmaticsLanguage::Turkish => "tr",
            SpeechmaticsLanguage::Ukrainian => "uk",
            SpeechmaticsLanguage::Urdu => "ur",
            SpeechmaticsLanguage::Uyghur => "ug",
            SpeechmaticsLanguage::Vietnamese => "vi",
            SpeechmaticsLanguage::Welsh => "cy",
        }
    }
}

impl fmt::Display for SpeechmaticsLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for SpeechmaticsLanguage {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" | "automatic" => Ok(SpeechmaticsLanguage::Auto),
            "ar" | "arabic" => Ok(SpeechmaticsLanguage::Arabic),
            "ba" | "bashkir" => Ok(SpeechmaticsLanguage::Bashkir),
            "eu" | "basque" => Ok(SpeechmaticsLanguage::Basque),
            "be" | "belarusian" => Ok(SpeechmaticsLanguage::Belarusian),
            "bn" | "bengali" => Ok(SpeechmaticsLanguage::Bengali),
            "bg" | "bulgarian" => Ok(SpeechmaticsLanguage::Bulgarian),
            "yue" | "cantonese" => Ok(SpeechmaticsLanguage::Cantonese),
            "ca" | "catalan" => Ok(SpeechmaticsLanguage::Catalan),
            "hr" | "croatian" => Ok(SpeechmaticsLanguage::Croatian),
            "cs" | "czech" => Ok(SpeechmaticsLanguage::Czech),
            "da" | "danish" => Ok(SpeechmaticsLanguage::Danish),
            "nl" | "dutch" => Ok(SpeechmaticsLanguage::Dutch),
            "en" | "english" | "en-us" | "en-gb" => Ok(SpeechmaticsLanguage::English),
            "eo" | "esperanto" => Ok(SpeechmaticsLanguage::Esperanto),
            "et" | "estonian" => Ok(SpeechmaticsLanguage::Estonian),
            "fi" | "finnish" => Ok(SpeechmaticsLanguage::Finnish),
            "fr" | "french" => Ok(SpeechmaticsLanguage::French),
            "gl" | "galician" => Ok(SpeechmaticsLanguage::Galician),
            "de" | "german" => Ok(SpeechmaticsLanguage::German),
            "el" | "greek" => Ok(SpeechmaticsLanguage::Greek),
            "he" | "hebrew" => Ok(SpeechmaticsLanguage::Hebrew),
            "hi" | "hindi" => Ok(SpeechmaticsLanguage::Hindi),
            "hu" | "hungarian" => Ok(SpeechmaticsLanguage::Hungarian),
            "id" | "indonesian" => Ok(SpeechmaticsLanguage::Indonesian),
            "ia" | "interlingua" => Ok(SpeechmaticsLanguage::Interlingua),
            "ga" | "irish" => Ok(SpeechmaticsLanguage::Irish),
            "it" | "italian" => Ok(SpeechmaticsLanguage::Italian),
            "ja" | "japanese" => Ok(SpeechmaticsLanguage::Japanese),
            "ko" | "korean" => Ok(SpeechmaticsLanguage::Korean),
            "lv" | "latvian" => Ok(SpeechmaticsLanguage::Latvian),
            "lt" | "lithuanian" => Ok(SpeechmaticsLanguage::Lithuanian),
            "ms" | "malay" => Ok(SpeechmaticsLanguage::Malay),
            "mt" | "maltese" => Ok(SpeechmaticsLanguage::Maltese),
            "cmn" | "mandarin" | "zh" | "chinese" => Ok(SpeechmaticsLanguage::Mandarin),
            "mr" | "marathi" => Ok(SpeechmaticsLanguage::Marathi),
            "mn" | "mongolian" => Ok(SpeechmaticsLanguage::Mongolian),
            "no" | "norwegian" => Ok(SpeechmaticsLanguage::Norwegian),
            "fa" | "persian" | "farsi" => Ok(SpeechmaticsLanguage::Persian),
            "pl" | "polish" => Ok(SpeechmaticsLanguage::Polish),
            "pt" | "portuguese" => Ok(SpeechmaticsLanguage::Portuguese),
            "ro" | "romanian" => Ok(SpeechmaticsLanguage::Romanian),
            "ru" | "russian" => Ok(SpeechmaticsLanguage::Russian),
            "sk" | "slovak" | "slovakian" => Ok(SpeechmaticsLanguage::Slovak),
            "sl" | "slovenian" => Ok(SpeechmaticsLanguage::Slovenian),
            "es" | "spanish" => Ok(SpeechmaticsLanguage::Spanish),
            "sw" | "swahili" => Ok(SpeechmaticsLanguage::Swahili),
            "sv" | "swedish" => Ok(SpeechmaticsLanguage::Swedish),
            "ta" | "tamil" => Ok(SpeechmaticsLanguage::Tamil),
            "th" | "thai" => Ok(SpeechmaticsLanguage::Thai),
            "tr" | "turkish" => Ok(SpeechmaticsLanguage::Turkish),
            "uk" | "ukrainian" => Ok(SpeechmaticsLanguage::Ukrainian),
            "ur" | "urdu" => Ok(SpeechmaticsLanguage::Urdu),
            "ug" | "uyghur" => Ok(SpeechmaticsLanguage::Uyghur),
            "vi" | "vietnamese" => Ok(SpeechmaticsLanguage::Vietnamese),
            "cy" | "welsh" => Ok(SpeechmaticsLanguage::Welsh),
            _ => Err(STTError::ConfigurationError(format!(
                "Unsupported language: '{}'. Speechmatics supports 55+ languages.",
                s
            ))),
        }
    }
}

// =============================================================================
// STT Configuration
// =============================================================================

/// Configuration for Speechmatics STT provider
#[derive(Debug, Clone)]
pub struct SpeechmaticsSTTConfig {
    /// API key for authentication
    pub api_key: String,
    /// Language for transcription
    pub language: SpeechmaticsLanguage,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Audio encoding format
    pub encoding: SpeechmaticsEncoding,
    /// API region (EU or US)
    pub region: SpeechmaticsRegion,
    /// Operating point (standard or enhanced)
    pub operating_point: SpeechmaticsOperatingPoint,
    /// Enable partial (interim) transcripts
    pub enable_partials: bool,
    /// Maximum delay in seconds (0.0-10.0)
    pub max_delay: f32,
    /// Enable speaker diarization
    pub enable_diarization: bool,
    /// Maximum number of speakers for diarization
    pub max_speakers: Option<u8>,
    /// Enable entity recognition
    pub enable_entities: bool,
    /// Custom vocabulary words
    pub additional_vocab: Vec<String>,
    /// Punctuation sensitivity (0.0-1.0)
    pub punctuation_sensitivity: Option<f32>,
    // -------------------------------------------------------------------------
    // Standardized advanced features emitted into `transcription_config`.
    // `None`/empty => the corresponding `transcription_config` key is omitted (provider default).
    // -------------------------------------------------------------------------
    /// Diarization mode override ("speaker", "channel", "channel_and_speaker"). When set this takes
    /// precedence over the legacy `enable_diarization` (which only ever requests "speaker").
    /// Mapped from the typed `multichannel`/`diarization` features. Speechmatics
    /// `transcription_config.diarization`.
    pub diarization_mode: Option<String>,
    /// End-of-utterance silence trigger in SECONDS (turn detection). Mapped from the typed
    /// `utterance_end_ms` (ms -> seconds). Speechmatics
    /// `transcription_config.conversation_config.end_of_utterance_silence_trigger`.
    pub end_of_utterance_silence_trigger: Option<f32>,
    /// Remove disfluencies. Mapped from the typed `filler_words` (inverted: keeping fillers means
    /// NOT removing disfluencies). Speechmatics
    /// `transcription_config.transcript_filtering_config.remove_disfluencies`.
    pub remove_disfluencies: Option<bool>,
    // --- the following are carried via the open `extras` passthrough ---
    /// Speaker diarization sensitivity (0.0-1.0). `speaker_diarization_config.speaker_sensitivity`.
    pub speaker_sensitivity: Option<f32>,
    /// Prefer attributing ambiguous words to the current speaker.
    /// `speaker_diarization_config.prefer_current_speaker`.
    pub prefer_current_speaker: Option<bool>,
    /// Punctuation permitted-marks override. `punctuation_overrides.permitted_marks`.
    pub permitted_marks: Option<Vec<String>>,
    /// Find-and-replace rules: `(from, to)` pairs.
    /// `transcript_filtering_config.replacements`.
    pub replacements: Option<Vec<(String, String)>>,
    /// Output locale (regional variant). `transcription_config.output_locale`.
    pub output_locale: Option<String>,
    /// Domain language pack. `transcription_config.domain`.
    pub domain: Option<String>,
    /// Max-delay mode ("flexible" | "fixed"). `transcription_config.max_delay_mode`.
    pub max_delay_mode: Option<String>,
    /// Phonetic hints per vocabulary word: `word -> [sounds_like, ...]`.
    /// `transcription_config.additional_vocab[].sounds_like`.
    pub vocab_sounds_like: std::collections::BTreeMap<String, Vec<String>>,
    /// Carried from the standardized `endpoint_override` — points the dial at the in-repo mock/proxy
    /// (a local `ws://` server) for credential-free end-to-end integration tests; `None` uses the
    /// region endpoint from `ws_url()`. Only the dialed scheme://host is swapped; the `/v2` path is
    /// preserved (a path-less URL fails the WS handshake).
    pub endpoint_override: Option<String>,

    /// P5 translation (Class A): target languages (ISO-639-1) for the `translation_config` PEER
    /// object (a sibling of `transcription_config`, NOT nested inside it). Empty = no translation.
    /// Speechmatics accepts at most 5; the canonical mapper truncates + warns. Source language is
    /// the normal `transcription_config.language`. Stream output arrives as `AddTranslation` /
    /// `AddPartialTranslation` messages folded into the uniform `translations[]`.
    pub translation_target_languages: Vec<String>,

    /// P5 translation: emit `AddPartialTranslation` (interim) alongside finals
    /// (`translation_config.enable_partials`). `None` = provider default (finals only).
    pub translation_enable_partials: Option<bool>,

    /// P5 translation OUTPUT mapping: the CANONICAL BCP-47 target strings the
    /// developer asked for (e.g. `"es-ES"`, `"de-DE"`), in the SAME order as
    /// [`translation_target_languages`](Self::translation_target_languages)'s
    /// ISO-639-1 codes. Speechmatics echoes only the ISO-639-1 code on each
    /// `AddTranslation` frame (`"es"`), which is lossy; the client uses this to
    /// upgrade that code back to the canonical BCP-47 the caller requested so
    /// the uniform `translations[].lang` stays canonical. Empty = no upgrade
    /// (pass the provider code through verbatim).
    pub translation_target_canonical: Vec<String>,
}

impl Default for SpeechmaticsSTTConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            language: SpeechmaticsLanguage::English,
            sample_rate: 16000,
            encoding: SpeechmaticsEncoding::PcmS16le,
            region: SpeechmaticsRegion::EU,
            operating_point: SpeechmaticsOperatingPoint::Standard,
            enable_partials: true,
            max_delay: super::DEFAULT_MAX_DELAY,
            enable_diarization: false,
            max_speakers: None,
            enable_entities: false,
            additional_vocab: Vec::new(),
            punctuation_sensitivity: None,
            diarization_mode: None,
            end_of_utterance_silence_trigger: None,
            remove_disfluencies: None,
            speaker_sensitivity: None,
            prefer_current_speaker: None,
            permitted_marks: None,
            replacements: None,
            output_locale: None,
            domain: None,
            max_delay_mode: None,
            vocab_sounds_like: std::collections::BTreeMap::new(),
            endpoint_override: None,
            translation_target_languages: Vec::new(),
            translation_enable_partials: None,
            translation_target_canonical: Vec::new(),
        }
    }
}

impl SpeechmaticsSTTConfig {
    /// Create a new configuration with the given API key
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Create configuration from base STTConfig
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        // API key is required
        let api_key = if !config.api_key.is_empty() {
            config.api_key.clone()
        } else {
            std::env::var("SPEECHMATICS_API_KEY").map_err(|_| {
                STTError::ConfigurationError(
                    "Speechmatics API key required. Set api_key or SPEECHMATICS_API_KEY env var"
                        .to_string(),
                )
            })?
        };

        // Parse language
        let language = config
            .language
            .parse()
            .unwrap_or(SpeechmaticsLanguage::English);

        // Parse encoding
        let encoding = config
            .encoding
            .parse()
            .unwrap_or(SpeechmaticsEncoding::PcmS16le);

        Ok(Self {
            api_key,
            language,
            sample_rate: config.sample_rate,
            encoding,
            ..Default::default()
        })
    }

    /// Build from the standardized config (W1 keystone — 3rd provider). Speechmatics has a rich
    /// feature surface, so this unlocks diarization, entity detection, custom vocabulary and
    /// partials through the standardized API — previously all unreachable via the flat factory.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;
        if let Some(d) = f.diarization {
            cfg.enable_diarization = d;
        }
        if let Some(i) = f.interim_results {
            cfg.enable_partials = i;
        }
        if let Some(e) = f.entity_detection {
            cfg.enable_entities = e;
        }
        if let Some(v) = &f.keyterms {
            cfg.additional_vocab = v.clone();
        }
        // Channel diarization mode (typed): `multichannel` requests per-channel transcription. If
        // speaker diarization is ALSO requested, use the combined "channel_and_speaker" mode;
        // otherwise plain "channel". When only `diarization` (speaker) is requested the existing
        // `enable_diarization` path ("speaker") still applies.
        if f.multichannel == Some(true) {
            cfg.diarization_mode = Some(if f.diarization == Some(true) {
                "channel_and_speaker".to_string()
            } else {
                "channel".to_string()
            });
        }
        // End-of-utterance silence trigger (typed): `utterance_end_ms` is in ms; Speechmatics wants
        // seconds.
        if let Some(ms) = f.utterance_end_ms {
            cfg.end_of_utterance_silence_trigger = Some(ms as f32 / 1000.0);
        }
        // Disfluency removal (typed): `filler_words` has inverted sense — keeping fillers (true)
        // means NOT removing disfluencies.
        if let Some(filler) = f.filler_words {
            cfg.remove_disfluencies = Some(!filler);
        }

        // Provider extras → transcription_config knobs not modeled by the typed vocabulary.
        let e = &std.extras.0;
        if let Some(v) = e.get("speaker_sensitivity").and_then(|v| v.as_f64()) {
            cfg.speaker_sensitivity = Some(v as f32);
        }
        if let Some(v) = e.get("prefer_current_speaker").and_then(|v| v.as_bool()) {
            cfg.prefer_current_speaker = Some(v);
        }
        if let Some(arr) = e.get("permitted_marks").and_then(|v| v.as_array()) {
            cfg.permitted_marks = Some(
                arr.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect(),
            );
        }
        if let Some(v) = e.get("punctuation_sensitivity").and_then(|v| v.as_f64()) {
            cfg.punctuation_sensitivity = Some(v as f32);
        }
        if let Some(arr) = e.get("replacements").and_then(|v| v.as_array()) {
            // Each replacement is `{ "from": "...", "to": "..." }`.
            let reps: Vec<(String, String)> = arr
                .iter()
                .filter_map(|r| {
                    let from = r.get("from").and_then(|v| v.as_str())?;
                    let to = r.get("to").and_then(|v| v.as_str())?;
                    Some((from.to_string(), to.to_string()))
                })
                .collect();
            if !reps.is_empty() {
                cfg.replacements = Some(reps);
            }
        }
        if let Some(v) = e.get("output_locale").and_then(|v| v.as_str()) {
            cfg.output_locale = Some(v.to_string());
        }
        if let Some(v) = e.get("domain").and_then(|v| v.as_str()) {
            cfg.domain = Some(v.to_string());
        }
        if let Some(v) = e.get("max_delay_mode").and_then(|v| v.as_str()) {
            cfg.max_delay_mode = Some(v.to_string());
        }
        // Phonetic hints per vocab word: extras["additional_vocab"] = [{ "content": "...",
        // "sounds_like": ["..."] }, ...]. The `content` words are also folded into the
        // `additional_vocab` list (so a sounds_like-only vocab does not need a separate keyterms).
        if let Some(arr) = e.get("additional_vocab").and_then(|v| v.as_array()) {
            for entry in arr {
                let Some(content) = entry.get("content").and_then(|v| v.as_str()) else {
                    continue;
                };
                if !cfg.additional_vocab.iter().any(|w| w == content) {
                    cfg.additional_vocab.push(content.to_string());
                }
                if let Some(sl) = entry.get("sounds_like").and_then(|v| v.as_array()) {
                    let hints: Vec<String> = sl
                        .iter()
                        .filter_map(|h| h.as_str().map(str::to_string))
                        .collect();
                    if !hints.is_empty() {
                        cfg.vocab_sounds_like.insert(content.to_string(), hints);
                    }
                }
            }
        }
        // Standardized endpoint override (mock/proxy host) for credential-free integration tests.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        // P5 translation (Class A — arbitrary targets): the canonical block maps to the
        // `translation_config` PEER object. `target_iso639_1(Some(5))` caps to Speechmatics' MAX 5
        // (the truncation is surfaced as a `config_warning` by `TranslationConfig::warnings_for`).
        if let Some(t) = &std.translation
            && !t.is_noop()
        {
            cfg.translation_target_languages = t
                .target_iso639_1(Some(
                    crate::core::stt::standard::SPEECHMATICS_MAX_TRANSLATION_TARGETS,
                ))
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            // Index-aligned canonical BCP-47 list, so the OUTPUT path can upgrade
            // the ISO-639-1 code Speechmatics echoes back to the canonical target.
            cfg.translation_target_canonical = t
                .target_canonical(Some(
                    crate::core::stt::standard::SPEECHMATICS_MAX_TRANSLATION_TARGETS,
                ))
                .into_iter()
                .map(|s| s.to_string())
                .collect();
            cfg.translation_enable_partials = t.partials;
        }
        Ok(cfg)
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), STTError> {
        if self.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "API key is required".to_string(),
            ));
        }

        if self.sample_rate < 8000 || self.sample_rate > 48000 {
            return Err(STTError::ConfigurationError(format!(
                "Sample rate must be between 8000 and 48000 Hz, got {}",
                self.sample_rate
            )));
        }

        if self.max_delay < 0.0 || self.max_delay > 10.0 {
            return Err(STTError::ConfigurationError(format!(
                "Max delay must be between 0.0 and 10.0 seconds, got {}",
                self.max_delay
            )));
        }

        if let Some(sensitivity) = self.punctuation_sensitivity
            && !(0.0..=1.0).contains(&sensitivity) {
                return Err(STTError::ConfigurationError(format!(
                    "Punctuation sensitivity must be between 0.0 and 1.0, got {}",
                    sensitivity
                )));
            }

        if let Some(max_speakers) = self.max_speakers
            && (!(1..=20).contains(&max_speakers)) {
                return Err(STTError::ConfigurationError(format!(
                    "Max speakers must be between 1 and 20, got {}",
                    max_speakers
                )));
            }

        Ok(())
    }

    /// Get the WebSocket URL for this configuration
    pub fn ws_url(&self) -> &'static str {
        self.region.ws_url()
    }

    /// Set the region
    pub fn with_region(mut self, region: SpeechmaticsRegion) -> Self {
        self.region = region;
        self
    }

    /// Set the operating point
    pub fn with_operating_point(mut self, operating_point: SpeechmaticsOperatingPoint) -> Self {
        self.operating_point = operating_point;
        self
    }

    /// Enable or disable partial transcripts
    pub fn with_partials(mut self, enable: bool) -> Self {
        self.enable_partials = enable;
        self
    }

    /// Set the maximum delay
    pub fn with_max_delay(mut self, delay: f32) -> Self {
        self.max_delay = delay.clamp(0.0, 10.0);
        self
    }

    /// Enable speaker diarization
    pub fn with_diarization(mut self, enable: bool, max_speakers: Option<u8>) -> Self {
        self.enable_diarization = enable;
        self.max_speakers = max_speakers.map(|s| s.clamp(1, 20));
        self
    }

    /// Add custom vocabulary
    pub fn with_vocab(mut self, words: Vec<String>) -> Self {
        self.additional_vocab = words;
        self
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (3rd provider): the standardized features unlock Speechmatics' rich surface
    // (diarization, entities, vocabulary, partials) — previously unreachable via the flat factory.
    #[test]
    fn from_standard_unlocks_speechmatics_features() {
        use crate::core::stt::standard::{SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "speechmatics".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                entity_detection: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Speechmatics".into()]),
                interim_results: Some(false),
                ..Default::default()
            },
            extras: Default::default(),
            translation: None,
        };
        let cfg = SpeechmaticsSTTConfig::from_standard(&std).unwrap();
        assert!(cfg.enable_diarization);
        assert!(cfg.enable_entities);
        assert_eq!(cfg.additional_vocab, vec!["WaaV", "Speechmatics"]);
        assert!(!cfg.enable_partials);
    }

    #[test]
    fn from_standard_maps_canonical_translation_to_peer_object() {
        // P5: the canonical translation block (Class A) → `translation_config` peer fields,
        // canonical BCP-47 → ISO-639-1, with the Speechmatics MAX-5 cap applied.
        use crate::core::lang::CanonicalLanguage;
        use crate::core::stt::standard::{StandardSTTConfig, TranslationConfig};
        let mut std = StandardSTTConfig::from_base(STTConfig {
            provider: "speechmatics".into(),
            api_key: "k".into(),
            ..Default::default()
        });
        std.translation = Some(TranslationConfig {
            target_languages: vec![
                CanonicalLanguage::EsEs,
                CanonicalLanguage::DeDe,
                CanonicalLanguage::FrFr,
                CanonicalLanguage::ItIt,
                CanonicalLanguage::PtPt,
                CanonicalLanguage::NlNl, // 6th → truncated by the cap
            ],
            translate_to_english: None,
            partials: Some(true),
        });
        let cfg = SpeechmaticsSTTConfig::from_standard(&std).unwrap();
        assert_eq!(
            cfg.translation_target_languages,
            vec!["es", "de", "fr", "it", "pt"]
        );
        assert_eq!(cfg.translation_enable_partials, Some(true));
    }

    #[test]
    fn test_region_default() {
        assert_eq!(SpeechmaticsRegion::default(), SpeechmaticsRegion::EU);
    }

    #[test]
    fn test_region_ws_url() {
        assert_eq!(
            SpeechmaticsRegion::EU.ws_url(),
            "wss://eu.rt.speechmatics.com/v2"
        );
        assert_eq!(
            SpeechmaticsRegion::US.ws_url(),
            "wss://us.rt.speechmatics.com/v2"
        );
    }

    #[test]
    fn test_region_from_str() {
        assert_eq!(
            "eu".parse::<SpeechmaticsRegion>().unwrap(),
            SpeechmaticsRegion::EU
        );
        assert_eq!(
            "us".parse::<SpeechmaticsRegion>().unwrap(),
            SpeechmaticsRegion::US
        );
        assert_eq!(
            "europe".parse::<SpeechmaticsRegion>().unwrap(),
            SpeechmaticsRegion::EU
        );
        assert!("invalid".parse::<SpeechmaticsRegion>().is_err());
    }

    #[test]
    fn test_operating_point_default() {
        assert_eq!(
            SpeechmaticsOperatingPoint::default(),
            SpeechmaticsOperatingPoint::Standard
        );
    }

    #[test]
    fn test_operating_point_from_str() {
        assert_eq!(
            "standard".parse::<SpeechmaticsOperatingPoint>().unwrap(),
            SpeechmaticsOperatingPoint::Standard
        );
        assert_eq!(
            "enhanced".parse::<SpeechmaticsOperatingPoint>().unwrap(),
            SpeechmaticsOperatingPoint::Enhanced
        );
        assert!("invalid".parse::<SpeechmaticsOperatingPoint>().is_err());
    }

    #[test]
    fn test_encoding_default() {
        assert_eq!(
            SpeechmaticsEncoding::default(),
            SpeechmaticsEncoding::PcmS16le
        );
    }

    #[test]
    fn test_encoding_as_str() {
        assert_eq!(SpeechmaticsEncoding::PcmS16le.as_str(), "pcm_s16le");
        assert_eq!(SpeechmaticsEncoding::PcmF32le.as_str(), "pcm_f32le");
        assert_eq!(SpeechmaticsEncoding::Mulaw.as_str(), "mulaw");
    }

    #[test]
    fn test_encoding_from_str() {
        assert_eq!(
            "pcm_s16le".parse::<SpeechmaticsEncoding>().unwrap(),
            SpeechmaticsEncoding::PcmS16le
        );
        assert_eq!(
            "linear16".parse::<SpeechmaticsEncoding>().unwrap(),
            SpeechmaticsEncoding::PcmS16le
        );
        assert_eq!(
            "mulaw".parse::<SpeechmaticsEncoding>().unwrap(),
            SpeechmaticsEncoding::Mulaw
        );
        assert!("invalid".parse::<SpeechmaticsEncoding>().is_err());
    }

    #[test]
    fn test_language_default() {
        assert_eq!(
            SpeechmaticsLanguage::default(),
            SpeechmaticsLanguage::English
        );
    }

    #[test]
    fn test_language_as_str() {
        assert_eq!(SpeechmaticsLanguage::English.as_str(), "en");
        assert_eq!(SpeechmaticsLanguage::French.as_str(), "fr");
        assert_eq!(SpeechmaticsLanguage::Japanese.as_str(), "ja");
        assert_eq!(SpeechmaticsLanguage::Auto.as_str(), "auto");
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(
            "en".parse::<SpeechmaticsLanguage>().unwrap(),
            SpeechmaticsLanguage::English
        );
        assert_eq!(
            "english".parse::<SpeechmaticsLanguage>().unwrap(),
            SpeechmaticsLanguage::English
        );
        assert_eq!(
            "ja".parse::<SpeechmaticsLanguage>().unwrap(),
            SpeechmaticsLanguage::Japanese
        );
        assert_eq!(
            "auto".parse::<SpeechmaticsLanguage>().unwrap(),
            SpeechmaticsLanguage::Auto
        );
        assert!("invalid_lang".parse::<SpeechmaticsLanguage>().is_err());
    }

    #[test]
    fn test_config_default() {
        let config = SpeechmaticsSTTConfig::default();
        assert!(config.api_key.is_empty());
        assert_eq!(config.language, SpeechmaticsLanguage::English);
        assert_eq!(config.sample_rate, 16000);
        assert_eq!(config.encoding, SpeechmaticsEncoding::PcmS16le);
        assert_eq!(config.region, SpeechmaticsRegion::EU);
        assert_eq!(config.operating_point, SpeechmaticsOperatingPoint::Standard);
        assert!(config.enable_partials);
        assert_eq!(config.max_delay, 2.0);
        assert!(!config.enable_diarization);
    }

    #[test]
    fn test_config_new() {
        let config = SpeechmaticsSTTConfig::new("test-api-key");
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.language, SpeechmaticsLanguage::English);
    }

    #[test]
    fn test_config_validate_valid() {
        let config = SpeechmaticsSTTConfig::new("test-api-key");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validate_empty_api_key() {
        let config = SpeechmaticsSTTConfig::default();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_invalid_sample_rate() {
        let mut config = SpeechmaticsSTTConfig::new("test-api-key");
        config.sample_rate = 4000;
        assert!(config.validate().is_err());

        config.sample_rate = 100000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_invalid_max_delay() {
        let mut config = SpeechmaticsSTTConfig::new("test-api-key");
        config.max_delay = -1.0;
        assert!(config.validate().is_err());

        config.max_delay = 15.0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_invalid_punctuation_sensitivity() {
        let mut config = SpeechmaticsSTTConfig::new("test-api-key");
        config.punctuation_sensitivity = Some(-0.5);
        assert!(config.validate().is_err());

        config.punctuation_sensitivity = Some(1.5);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_validate_invalid_max_speakers() {
        let mut config = SpeechmaticsSTTConfig::new("test-api-key");
        config.max_speakers = Some(0);
        assert!(config.validate().is_err());

        config.max_speakers = Some(25);
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_config_builder_methods() {
        let config = SpeechmaticsSTTConfig::new("test-api-key")
            .with_region(SpeechmaticsRegion::US)
            .with_operating_point(SpeechmaticsOperatingPoint::Enhanced)
            .with_partials(false)
            .with_max_delay(5.0)
            .with_diarization(true, Some(4))
            .with_vocab(vec!["custom".to_string(), "words".to_string()]);

        assert_eq!(config.region, SpeechmaticsRegion::US);
        assert_eq!(config.operating_point, SpeechmaticsOperatingPoint::Enhanced);
        assert!(!config.enable_partials);
        assert_eq!(config.max_delay, 5.0);
        assert!(config.enable_diarization);
        assert_eq!(config.max_speakers, Some(4));
        assert_eq!(config.additional_vocab.len(), 2);
    }

    #[test]
    fn test_config_from_base() {
        let base_config = STTConfig {
            api_key: "test-key".to_string(),
            language: "fr".to_string(),
            sample_rate: 44100,
            encoding: "pcm_f32le".to_string(),
            ..Default::default()
        };

        let config = SpeechmaticsSTTConfig::from_base(&base_config).unwrap();
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.language, SpeechmaticsLanguage::French);
        assert_eq!(config.sample_rate, 44100);
        assert_eq!(config.encoding, SpeechmaticsEncoding::PcmF32le);
    }

    #[test]
    fn test_config_ws_url() {
        let mut config = SpeechmaticsSTTConfig::new("test-api-key");
        assert_eq!(config.ws_url(), "wss://eu.rt.speechmatics.com/v2");

        config.region = SpeechmaticsRegion::US;
        assert_eq!(config.ws_url(), "wss://us.rt.speechmatics.com/v2");
    }
}
