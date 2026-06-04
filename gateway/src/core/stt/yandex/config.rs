//! Yandex SpeechKit STT Configuration
//!
//! Configuration types and parsing for Yandex SpeechKit Speech-to-Text API.

use crate::core::stt::base::{STTConfig, STTError};
use std::str::FromStr;

// =============================================================================
// Audio Format
// =============================================================================

/// Supported audio formats for Yandex STT
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YandexSTTAudioFormat {
    /// OGG container with Opus codec
    OggOpus,
    /// Linear PCM (16-bit signed, little-endian)
    #[default]
    Lpcm,
    /// MP3 format
    Mp3,
}

impl YandexSTTAudioFormat {
    /// Get the API format string
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::OggOpus => "oggopus",
            Self::Lpcm => "lpcm",
            Self::Mp3 => "mp3",
        }
    }

    /// Get the content type for multipart upload
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::OggOpus => "audio/ogg",
            Self::Lpcm => "audio/x-pcm",
            Self::Mp3 => "audio/mpeg",
        }
    }
}

impl FromStr for YandexSTTAudioFormat {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "lpcm" | "linear16" | "pcm" | "l16" | "raw" => Ok(Self::Lpcm),
            "oggopus" | "ogg" | "opus" | "ogg_opus" | "ogg-opus" => Ok(Self::OggOpus),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid Yandex STT audio format: '{}'. Valid formats: lpcm, oggopus, mp3",
                s
            ))),
        }
    }
}

// =============================================================================
// Language Configuration
// =============================================================================

/// Supported languages for Yandex STT
#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(Default)]
pub enum YandexSTTLanguage {
    /// Russian (default)
    #[default]
    Russian,
    /// English
    English,
    /// German
    German,
    /// French
    French,
    /// Finnish
    Finnish,
    /// Swedish
    Swedish,
    /// Dutch
    Dutch,
    /// Polish
    Polish,
    /// Portuguese
    Portuguese,
    /// Turkish
    Turkish,
    /// Ukrainian
    Ukrainian,
    /// Kazakh
    Kazakh,
    /// Uzbek
    Uzbek,
    /// Hebrew
    Hebrew,
    /// Auto-detect language
    Auto,
}


impl YandexSTTLanguage {
    /// Get the language code for the API
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::Russian => "ru-RU",
            Self::English => "en-US",
            Self::German => "de-DE",
            Self::French => "fr-FR",
            Self::Finnish => "fi-FI",
            Self::Swedish => "sv-SE",
            Self::Dutch => "nl-NL",
            Self::Polish => "pl-PL",
            Self::Portuguese => "pt-PT",
            Self::Turkish => "tr-TR",
            Self::Ukrainian => "uk-UA",
            Self::Kazakh => "kk-KZ",
            Self::Uzbek => "uz-UZ",
            Self::Hebrew => "he-IL",
            Self::Auto => "auto",
        }
    }
}

impl FromStr for YandexSTTLanguage {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "ru" | "ru-ru" | "russian" => Ok(Self::Russian),
            "en" | "en-us" | "en-gb" | "english" => Ok(Self::English),
            "de" | "de-de" | "german" => Ok(Self::German),
            "fr" | "fr-fr" | "french" => Ok(Self::French),
            "fi" | "fi-fi" | "finnish" => Ok(Self::Finnish),
            "sv" | "sv-se" | "swedish" => Ok(Self::Swedish),
            "nl" | "nl-nl" | "dutch" => Ok(Self::Dutch),
            "pl" | "pl-pl" | "polish" => Ok(Self::Polish),
            "pt" | "pt-pt" | "portuguese" => Ok(Self::Portuguese),
            "tr" | "tr-tr" | "turkish" => Ok(Self::Turkish),
            "uk" | "uk-ua" | "ukrainian" => Ok(Self::Ukrainian),
            "kk" | "kk-kz" | "kazakh" => Ok(Self::Kazakh),
            "uz" | "uz-uz" | "uzbek" => Ok(Self::Uzbek),
            "he" | "he-il" | "hebrew" => Ok(Self::Hebrew),
            "auto" | "detect" => Ok(Self::Auto),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid Yandex STT language: '{}'. Valid: ru, en, de, fr, fi, sv, nl, pl, pt, tr, uk, kk, uz, he, auto",
                s
            ))),
        }
    }
}

// =============================================================================
// Recognition Model
// =============================================================================

/// Recognition model for Yandex STT
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum YandexSTTModel {
    /// General model (default)
    #[default]
    General,
    /// General with improved real-time streaming
    GeneralRc,
    /// Conversational model optimized for dialog
    Deferred,
}

impl YandexSTTModel {
    /// Get the model topic for the API
    pub fn as_topic(&self) -> &'static str {
        match self {
            Self::General => "general",
            Self::GeneralRc => "general:rc",
            Self::Deferred => "deferred",
        }
    }
}

impl FromStr for YandexSTTModel {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "general" | "" | "default" => Ok(Self::General),
            "general:rc" | "rc" | "realtime" => Ok(Self::GeneralRc),
            "deferred" | "batch" => Ok(Self::Deferred),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid Yandex STT model: '{}'. Valid: general, general:rc, deferred",
                s
            ))),
        }
    }
}

// =============================================================================
// Yandex STT Configuration
// =============================================================================

/// Yandex SpeechKit STT Configuration
#[derive(Debug, Clone)]
pub struct YandexSTTConfig {
    /// API key (or IAM token)
    pub api_key: String,
    /// Folder ID (required for user accounts)
    pub folder_id: Option<String>,
    /// Whether the api_key is an IAM token
    pub is_iam_token: bool,
    /// Language for recognition
    pub language: YandexSTTLanguage,
    /// Audio format
    pub audio_format: YandexSTTAudioFormat,
    /// Sample rate in Hz (required for LPCM)
    pub sample_rate: u32,
    /// Recognition model
    pub model: YandexSTTModel,
    /// Enable profanity filter
    pub profanity_filter: bool,
    /// Enable automatic punctuation
    pub punctuation: bool,
    /// Enable partial (interim) results
    pub partial_results: bool,
    /// Enable speaker identification
    pub speaker_identification: bool,
    /// Maximum number of alternatives to return
    pub max_alternatives: u32,
    /// Custom vocabulary words for improved recognition
    pub hints: Vec<String>,
    /// Disable number normalization — write numbers as words instead of figures
    /// (`rawResults` query param). `true` → words ("twenty-five"); `false`/default → figures
    /// ("25"). Maps from the canonical `numerals` feature (which requests digits) as
    /// `raw_results = !numerals`.
    pub raw_results: bool,

    /// Base endpoint override (scheme://host) from the standardized `endpoint_override` — points the
    /// recognize POST at a mock/proxy host while preserving the `/speech/v1/stt:recognize` path
    /// (credential-free e2e); `None` uses the production Yandex host.
    pub endpoint_override: Option<String>,
}

impl YandexSTTConfig {
    /// Create configuration from base STTConfig
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "Yandex API key is required".to_string(),
            ));
        }

        // Detect IAM token vs API key
        let is_iam_token = config.api_key.len() > 100 || config.api_key.contains('.');

        // Extract folder_id from api_key if format is "folder_id:api_key"
        let (folder_id, api_key) = if config.api_key.contains(':') && !is_iam_token {
            let parts: Vec<&str> = config.api_key.splitn(2, ':').collect();
            (Some(parts[0].to_string()), parts[1].to_string())
        } else {
            (None, config.api_key.clone())
        };

        // Parse language
        let language = if config.language.is_empty() {
            YandexSTTLanguage::default()
        } else {
            config.language.parse().unwrap_or_default()
        };

        // Parse audio format
        let audio_format = if config.encoding.is_empty() {
            YandexSTTAudioFormat::default()
        } else {
            config.encoding.parse().unwrap_or_default()
        };

        // Validate sample rate for LPCM
        let sample_rate = if config.sample_rate == 0 {
            16000
        } else {
            config.sample_rate
        };

        // Parse model
        let model = if config.model.is_empty() {
            YandexSTTModel::default()
        } else {
            config.model.parse().unwrap_or_default()
        };

        Ok(Self {
            api_key,
            folder_id,
            is_iam_token,
            language,
            audio_format,
            sample_rate,
            model,
            profanity_filter: false,
            punctuation: config.punctuation,
            partial_results: true,
            speaker_identification: false,
            max_alternatives: 1,
            hints: Vec::new(),
            raw_results: false,
            endpoint_override: None,
        })
    }

    /// Resolve the recognize endpoint URL, honoring `endpoint_override` (mock host swap; the
    /// `/speech/v1/stt:recognize` path + query are preserved).
    pub fn recognize_url(&self) -> String {
        match &self.endpoint_override {
            Some(ov) => format!("{}/speech/v1/stt:recognize", ov.trim_end_matches('/')),
            None => crate::core::stt::yandex::YANDEX_STT_RECOGNIZE_URL.to_string(),
        }
    }

    /// Build from the standardized config (W1 keystone). Yandex exposes a modest feature
    /// surface, so this unlocks diarization, partials, profanity filtering and custom
    /// vocabulary (hints) through the standardized API. Features Yandex cannot express
    /// (word_timestamps, smart_format, redaction, entity_detection, …) are capability gaps
    /// and left at their defaults.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        if let Some(d) = f.diarization {
            cfg.speaker_identification = d;
        }
        if let Some(i) = f.interim_results {
            cfg.partial_results = i;
        }
        if let Some(p) = f.profanity_filter {
            cfg.profanity_filter = p;
        }
        if let Some(v) = &f.keyterms {
            cfg.hints = v.clone();
        }
        // Number normalization (words vs figures). Canonical `numerals` requests digits/figures,
        // which is Yandex's DEFAULT (rawResults=false). `rawResults=true` disables normalization,
        // emitting numbers as words — so map `raw_results = !numerals`.
        if let Some(n) = f.numerals {
            cfg.raw_results = !n;
        }
        Ok(cfg)
    }

    /// Get the Authorization header value
    pub fn auth_header_value(&self) -> String {
        if self.is_iam_token {
            format!("Bearer {}", self.api_key)
        } else {
            format!("Api-Key {}", self.api_key)
        }
    }

    /// Build query parameters for synchronous recognition
    pub fn build_query_params(&self) -> Vec<(&'static str, String)> {
        let mut params = Vec::new();

        // Language code
        params.push(("lang", self.language.as_code().to_string()));

        // Model topic
        params.push(("topic", self.model.as_topic().to_string()));

        // Audio format
        params.push(("format", self.audio_format.as_api_str().to_string()));

        // Sample rate (required for LPCM)
        if self.audio_format == YandexSTTAudioFormat::Lpcm {
            params.push(("sampleRateHertz", self.sample_rate.to_string()));
        }

        // Folder ID if specified
        if let Some(ref folder_id) = self.folder_id {
            params.push(("folderId", folder_id.clone()));
        }

        // Profanity filter
        if self.profanity_filter {
            params.push(("profanityFilter", "true".to_string()));
        }

        // Maximum alternatives
        if self.max_alternatives > 1 {
            params.push(("maxAlternatives", self.max_alternatives.to_string()));
        }

        // Number normalization: only emit when disabling normalization (words). The API default
        // (rawResults absent / false) already yields figures, so we keep the URL minimal.
        if self.raw_results {
            params.push(("rawResults", "true".to_string()));
        }

        params
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: maps the standardized features Yandex can express (diarization +
    // custom vocabulary hints) onto its own config fields.
    #[test]
    fn from_standard_maps_features() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "yandex".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                diarization: Some(true),
                keyterms: Some(vec!["WaaV".into(), "Yandex".into()]),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = YandexSTTConfig::from_standard(&std).unwrap();
        assert!(cfg.speaker_identification); // diarization
        assert_eq!(cfg.hints, vec!["WaaV", "Yandex"]); // keyterms
    }

    // WIRE-LEVEL: the canonical `numerals` feature must reach the request URL as the Yandex
    // `rawResults` query param (number normalization: figures vs words) — proving it lands on the
    // wire, not merely on the config struct. We build the actual reqwest request and inspect its
    // serialized URL (this is the same `.query(&build_query_params())` path the STT client uses).
    #[test]
    fn numerals_reaches_request_url_as_raw_results() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};

        // `numerals: Some(false)` = numbers as WORDS = rawResults=true.
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "yandex".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                numerals: Some(false),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg = YandexSTTConfig::from_standard(&std).unwrap();
        assert!(cfg.raw_results);

        let params = cfg.build_query_params();
        // Build the real request and assert the param is in the serialized URL.
        let client = reqwest::Client::new();
        let req = client
            .post(crate::core::stt::yandex::YANDEX_STT_RECOGNIZE_URL)
            .query(&params)
            .build()
            .unwrap();
        let url = req.url().as_str();
        assert!(
            url.contains("rawResults=true"),
            "rawResults not in request URL: {url}"
        );

        // `numerals: Some(true)` = figures = Yandex default = NO rawResults param (minimal URL).
        let std_figures = StandardSTTConfig {
            base: STTConfig {
                provider: "yandex".into(),
                api_key: "test-key".into(),
                ..Default::default()
            },
            features: SttFeatures {
                numerals: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
        };
        let cfg2 = YandexSTTConfig::from_standard(&std_figures).unwrap();
        assert!(!cfg2.raw_results);
        let req2 = reqwest::Client::new()
            .post(crate::core::stt::yandex::YANDEX_STT_RECOGNIZE_URL)
            .query(&cfg2.build_query_params())
            .build()
            .unwrap();
        assert!(
            !req2.url().as_str().contains("rawResults"),
            "rawResults must be absent for figures (default)"
        );
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            "lpcm".parse::<YandexSTTAudioFormat>().unwrap(),
            YandexSTTAudioFormat::Lpcm
        );
        assert_eq!(
            "linear16".parse::<YandexSTTAudioFormat>().unwrap(),
            YandexSTTAudioFormat::Lpcm
        );
        assert_eq!(
            "oggopus".parse::<YandexSTTAudioFormat>().unwrap(),
            YandexSTTAudioFormat::OggOpus
        );
        assert_eq!(
            "mp3".parse::<YandexSTTAudioFormat>().unwrap(),
            YandexSTTAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_api_str() {
        assert_eq!(YandexSTTAudioFormat::Lpcm.as_api_str(), "lpcm");
        assert_eq!(YandexSTTAudioFormat::OggOpus.as_api_str(), "oggopus");
        assert_eq!(YandexSTTAudioFormat::Mp3.as_api_str(), "mp3");
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(
            "ru-RU".parse::<YandexSTTLanguage>().unwrap(),
            YandexSTTLanguage::Russian
        );
        assert_eq!(
            "en".parse::<YandexSTTLanguage>().unwrap(),
            YandexSTTLanguage::English
        );
        assert_eq!(
            "de-de".parse::<YandexSTTLanguage>().unwrap(),
            YandexSTTLanguage::German
        );
        assert_eq!(
            "auto".parse::<YandexSTTLanguage>().unwrap(),
            YandexSTTLanguage::Auto
        );
    }

    #[test]
    fn test_language_code() {
        assert_eq!(YandexSTTLanguage::Russian.as_code(), "ru-RU");
        assert_eq!(YandexSTTLanguage::English.as_code(), "en-US");
        assert_eq!(YandexSTTLanguage::Auto.as_code(), "auto");
    }

    #[test]
    fn test_model_from_str() {
        assert_eq!(
            "general".parse::<YandexSTTModel>().unwrap(),
            YandexSTTModel::General
        );
        assert_eq!(
            "general:rc".parse::<YandexSTTModel>().unwrap(),
            YandexSTTModel::GeneralRc
        );
        assert_eq!(
            "deferred".parse::<YandexSTTModel>().unwrap(),
            YandexSTTModel::Deferred
        );
    }

    #[test]
    fn test_config_from_base() {
        let base = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            encoding: "lpcm".to_string(),
            punctuation: true,
            ..Default::default()
        };

        let config = YandexSTTConfig::from_base(&base).unwrap();
        assert_eq!(config.api_key, "test-api-key");
        assert_eq!(config.language, YandexSTTLanguage::Russian);
        assert_eq!(config.audio_format, YandexSTTAudioFormat::Lpcm);
        assert_eq!(config.sample_rate, 16000);
        assert!(config.punctuation);
    }

    #[test]
    fn test_config_with_folder_id() {
        let base = STTConfig {
            api_key: "b1g12345:AQVN1234567890".to_string(),
            language: "en".to_string(),
            ..Default::default()
        };

        let config = YandexSTTConfig::from_base(&base).unwrap();
        assert_eq!(config.folder_id, Some("b1g12345".to_string()));
        assert_eq!(config.api_key, "AQVN1234567890");
        assert!(!config.is_iam_token);
    }

    #[test]
    fn test_config_requires_api_key() {
        let base = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = YandexSTTConfig::from_base(&base);
        assert!(result.is_err());
    }

    #[test]
    fn test_auth_header_api_key() {
        let config = YandexSTTConfig {
            api_key: "test-key".to_string(),
            folder_id: None,
            is_iam_token: false,
            language: YandexSTTLanguage::default(),
            audio_format: YandexSTTAudioFormat::default(),
            sample_rate: 16000,
            model: YandexSTTModel::default(),
            profanity_filter: false,
            punctuation: true,
            partial_results: true,
            speaker_identification: false,
            max_alternatives: 1,
            hints: Vec::new(),
            raw_results: false,
            endpoint_override: None,
        };

        assert_eq!(config.auth_header_value(), "Api-Key test-key");
    }

    #[test]
    fn test_build_query_params() {
        let config = YandexSTTConfig {
            api_key: "test-key".to_string(),
            folder_id: Some("folder123".to_string()),
            is_iam_token: false,
            language: YandexSTTLanguage::Russian,
            audio_format: YandexSTTAudioFormat::Lpcm,
            sample_rate: 16000,
            model: YandexSTTModel::General,
            profanity_filter: true,
            punctuation: true,
            partial_results: true,
            speaker_identification: false,
            max_alternatives: 3,
            hints: Vec::new(),
            raw_results: false,
            endpoint_override: None,
        };

        let params = config.build_query_params();
        assert!(params.iter().any(|(k, v)| *k == "lang" && v == "ru-RU"));
        assert!(params.iter().any(|(k, v)| *k == "topic" && v == "general"));
        assert!(params.iter().any(|(k, v)| *k == "format" && v == "lpcm"));
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "sampleRateHertz" && v == "16000")
        );
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "folderId" && v == "folder123")
        );
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "profanityFilter" && v == "true")
        );
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "maxAlternatives" && v == "3")
        );
    }
}
