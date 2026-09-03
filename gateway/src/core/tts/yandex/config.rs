//! Yandex SpeechKit TTS Configuration Types
//!
//! This module defines configuration structures for the Yandex TTS provider,
//! including voices, emotions, audio formats, and authentication settings.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::tts::base::{TTSConfig, TTSError, TTSResult};

fn validate_yandex_tts_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }
    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// Audio Format Enum
// =============================================================================

/// Supported audio output formats for Yandex SpeechKit TTS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum YandexAudioFormat {
    /// Linear PCM (raw audio without header)
    Lpcm,
    /// OGG container with Opus codec (default)
    #[default]
    OggOpus,
    /// MP3 format
    Mp3,
}

impl YandexAudioFormat {
    /// Get the format string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lpcm => "lpcm",
            Self::OggOpus => "oggopus",
            Self::Mp3 => "mp3",
        }
    }

    /// Get the file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Lpcm => ".raw",
            Self::OggOpus => ".ogg",
            Self::Mp3 => ".mp3",
        }
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Lpcm => "audio/x-pcm",
            Self::OggOpus => "audio/ogg",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Check if sample rate parameter is supported (only for LPCM)
    pub fn supports_sample_rate(&self) -> bool {
        matches!(self, Self::Lpcm)
    }
}

impl fmt::Display for YandexAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for YandexAudioFormat {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "lpcm" | "pcm" | "linear16" | "raw" => Ok(Self::Lpcm),
            "oggopus" | "ogg" | "opus" => Ok(Self::OggOpus),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Yandex audio format: {}. Supported: lpcm, oggopus, mp3",
                s
            ))),
        }
    }
}

// =============================================================================
// Voice Enum
// =============================================================================

/// Supported voices for Yandex SpeechKit TTS
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum YandexVoice {
    // Russian voices
    #[default]
    Alena,
    Filipp,
    Ermil,
    Jane,
    Omazh,
    Zahar,
    Marina,
    Dasha,
    Julia,
    Lera,
    Masha,
    Alexander,
    Kirill,
    Anton,
    MadiRu,
    SauleRu,
    ZamiraRu,
    ZhanarRu,
    YulduzRu,
    // English voice
    John,
    // German voice
    Lea,
    // Hebrew voice
    Naomi,
    // Kazakh voices
    Amira,
    Madi,
    Saule,
    Zhanar,
    // Uzbek voices
    Nigora,
    Zamira,
    Yulduz,
    /// Custom voice ID (for brand voices)
    Custom(String),
}

impl YandexVoice {
    /// Get the voice ID string for API requests
    pub fn as_str(&self) -> &str {
        match self {
            Self::Alena => "alena",
            Self::Filipp => "filipp",
            Self::Ermil => "ermil",
            Self::Jane => "jane",
            Self::Omazh => "omazh",
            Self::Zahar => "zahar",
            Self::Marina => "marina",
            Self::Dasha => "dasha",
            Self::Julia => "julia",
            Self::Lera => "lera",
            Self::Masha => "masha",
            Self::Alexander => "alexander",
            Self::Kirill => "kirill",
            Self::Anton => "anton",
            Self::MadiRu => "madi_ru",
            Self::SauleRu => "saule_ru",
            Self::ZamiraRu => "zamira_ru",
            Self::ZhanarRu => "zhanar_ru",
            Self::YulduzRu => "yulduz_ru",
            Self::John => "john",
            Self::Lea => "lea",
            Self::Naomi => "naomi",
            Self::Amira => "amira",
            Self::Madi => "madi",
            Self::Saule => "saule",
            Self::Zhanar => "zhanar",
            Self::Nigora => "nigora",
            Self::Zamira => "zamira",
            Self::Yulduz => "yulduz",
            Self::Custom(s) => s,
        }
    }

    /// Get the default language for this voice
    pub fn default_language(&self) -> &'static str {
        match self {
            Self::John => "en-US",
            Self::Lea => "de-DE",
            Self::Naomi => "he-IL",
            Self::Amira | Self::Madi | Self::Saule | Self::Zhanar => "kk-KZ",
            Self::Nigora | Self::Zamira | Self::Yulduz => "uz-UZ",
            _ => "ru-RU",
        }
    }

    /// Check if the voice supports emotions
    pub fn supports_emotions(&self) -> bool {
        matches!(
            self,
            Self::Alena
                | Self::Ermil
                | Self::Jane
                | Self::Omazh
                | Self::Zahar
                | Self::Marina
                | Self::Dasha
                | Self::Julia
                | Self::Lera
                | Self::Masha
                | Self::Alexander
                | Self::Kirill
                | Self::Anton
                | Self::SauleRu
                | Self::ZamiraRu
                | Self::ZhanarRu
                | Self::YulduzRu
        )
    }
}

impl fmt::Display for YandexVoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for YandexVoice {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "alena" => Ok(Self::Alena),
            "filipp" => Ok(Self::Filipp),
            "ermil" => Ok(Self::Ermil),
            "jane" => Ok(Self::Jane),
            "omazh" => Ok(Self::Omazh),
            "zahar" => Ok(Self::Zahar),
            "marina" => Ok(Self::Marina),
            "dasha" => Ok(Self::Dasha),
            "julia" => Ok(Self::Julia),
            "lera" => Ok(Self::Lera),
            "masha" => Ok(Self::Masha),
            "alexander" => Ok(Self::Alexander),
            "kirill" => Ok(Self::Kirill),
            "anton" => Ok(Self::Anton),
            "madi_ru" => Ok(Self::MadiRu),
            "saule_ru" => Ok(Self::SauleRu),
            "zamira_ru" => Ok(Self::ZamiraRu),
            "zhanar_ru" => Ok(Self::ZhanarRu),
            "yulduz_ru" => Ok(Self::YulduzRu),
            "john" => Ok(Self::John),
            "lea" => Ok(Self::Lea),
            "naomi" => Ok(Self::Naomi),
            "amira" => Ok(Self::Amira),
            "madi" => Ok(Self::Madi),
            "saule" => Ok(Self::Saule),
            "zhanar" => Ok(Self::Zhanar),
            "nigora" => Ok(Self::Nigora),
            "zamira" => Ok(Self::Zamira),
            "yulduz" => Ok(Self::Yulduz),
            other => Ok(Self::Custom(other.to_string())),
        }
    }
}

// =============================================================================
// Emotion/Role Enum
// =============================================================================

/// Supported emotions/roles for Yandex SpeechKit TTS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum YandexEmotion {
    /// Neutral tone (default)
    #[default]
    Neutral,
    /// Friendly/cheerful tone
    Good,
    /// Irritated/angry tone
    Evil,
    /// Formal/professional tone
    Strict,
    /// Warm/approachable tone
    Friendly,
    /// Whispered speech
    Whisper,
}

impl YandexEmotion {
    /// Get the emotion string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Good => "good",
            Self::Evil => "evil",
            Self::Strict => "strict",
            Self::Friendly => "friendly",
            Self::Whisper => "whisper",
        }
    }
}

impl fmt::Display for YandexEmotion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for YandexEmotion {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "neutral" | "default" => Ok(Self::Neutral),
            "good" | "cheerful" | "happy" => Ok(Self::Good),
            "evil" | "angry" | "irritated" => Ok(Self::Evil),
            "strict" | "formal" | "professional" => Ok(Self::Strict),
            "friendly" | "warm" => Ok(Self::Friendly),
            "whisper" | "whispered" => Ok(Self::Whisper),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Yandex emotion: {}. Supported: neutral, good, evil, strict, friendly, whisper",
                s
            ))),
        }
    }
}

// =============================================================================
// Language Enum
// =============================================================================

/// Supported languages for Yandex SpeechKit TTS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum YandexLanguage {
    /// Russian (default)
    #[default]
    #[serde(rename = "ru-RU")]
    Russian,
    /// English (US)
    #[serde(rename = "en-US")]
    English,
    /// German
    #[serde(rename = "de-DE")]
    German,
    /// Hebrew
    #[serde(rename = "he-IL")]
    Hebrew,
    /// Kazakh
    #[serde(rename = "kk-KZ")]
    Kazakh,
    /// Uzbek
    #[serde(rename = "uz-UZ")]
    Uzbek,
}

impl YandexLanguage {
    /// Get the language code for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Russian => "ru-RU",
            Self::English => "en-US",
            Self::German => "de-DE",
            Self::Hebrew => "he-IL",
            Self::Kazakh => "kk-KZ",
            Self::Uzbek => "uz-UZ",
        }
    }
}

impl fmt::Display for YandexLanguage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for YandexLanguage {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").as_str() {
            "ru-ru" | "ru" | "russian" => Ok(Self::Russian),
            "en-us" | "en" | "english" => Ok(Self::English),
            "de-de" | "de" | "german" => Ok(Self::German),
            "he-il" | "he" | "hebrew" => Ok(Self::Hebrew),
            "kk-kz" | "kk" | "kazakh" => Ok(Self::Kazakh),
            "uz-uz" | "uz" | "uzbek" => Ok(Self::Uzbek),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown Yandex language: {}. Supported: ru-RU, en-US, de-DE, he-IL, kk-KZ, uz-UZ",
                s
            ))),
        }
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// Yandex SpeechKit TTS configuration
#[derive(Debug, Clone)]
pub struct YandexTtsConfig {
    /// API key or IAM token
    pub api_key: String,
    /// Whether the api_key is an IAM token (true) or API key (false)
    pub is_iam_token: bool,
    /// Folder ID (required for user accounts)
    pub folder_id: Option<String>,
    /// Voice to use
    pub voice: YandexVoice,
    /// Language code
    pub language: YandexLanguage,
    /// Emotion/role
    pub emotion: YandexEmotion,
    /// Speech speed (0.1 to 3.0)
    pub speed: f32,
    /// Audio output format
    pub audio_format: YandexAudioFormat,
    /// Sample rate in Hz (only for LPCM format)
    pub sample_rate: u32,
    /// Treat the synthesis input as SSML markup. When `true`, the text is sent in the v1
    /// `:synthesize` request's `ssml` form field instead of `text` (the two are mutually exclusive
    /// per the Yandex v1 REST API — see docs/providers/yandex.md "SSML Support").
    pub ssml: bool,
    /// Override base (scheme+host) for the synth HTTP POST; used by the mock e2e harness.
    pub endpoint_override: Option<String>,
}

impl YandexTtsConfig {
    /// Create configuration from a base TTSConfig
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration
    ///
    /// # Returns
    /// * `TTSResult<Self>` - Yandex-specific configuration or error
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Yandex SpeechKit requires an API key or IAM token".to_string(),
            ));
        }

        // Determine if it's an IAM token or API key
        // IAM tokens are typically longer and follow a specific format
        let is_iam_token = config.api_key.len() > 100 || config.api_key.contains('.');

        // Extract folder_id from api_key if format is "folder_id:api_key"
        let (folder_id, api_key) = if config.api_key.contains(':') && !is_iam_token {
            let parts: Vec<&str> = config.api_key.splitn(2, ':').collect();
            (Some(parts[0].to_string()), parts[1].to_string())
        } else {
            (None, config.api_key.clone())
        };

        // Parse voice
        let voice = if let Some(ref v) = config.voice_id {
            if !v.is_empty() {
                v.parse().unwrap_or_default()
            } else {
                YandexVoice::default()
            }
        } else {
            YandexVoice::default()
        };

        // Parse audio format
        let audio_format = if let Some(ref fmt) = config.audio_format {
            if !fmt.is_empty() {
                fmt.parse().unwrap_or_default()
            } else {
                YandexAudioFormat::default()
            }
        } else {
            YandexAudioFormat::default()
        };

        // Parse sample rate
        let sample_rate = match config.sample_rate {
            Some(sr) if sr > 0 => {
                // Yandex only supports 8000, 16000, 48000
                if sr <= 12000 {
                    super::MIN_SAMPLE_RATE
                } else if sr <= 32000 {
                    16000
                } else {
                    super::MAX_SAMPLE_RATE
                }
            }
            _ => super::DEFAULT_SAMPLE_RATE,
        };

        // Parse speed from speaking_rate
        let speed = config
            .speaking_rate
            .map(|s| s.clamp(super::MIN_SPEED, super::MAX_SPEED))
            .unwrap_or(super::DEFAULT_SPEED);

        // Extract emotion from emotion_config
        let emotion = config
            .emotion_config
            .as_ref()
            .and_then(|ec| {
                ec.emotion
                    .as_ref()
                    .and_then(|e| match e.to_string().to_lowercase().as_str() {
                        "happy" | "cheerful" | "good" | "joyful" => Some(YandexEmotion::Good),
                        "angry" | "evil" | "irritated" | "frustrated" => Some(YandexEmotion::Evil),
                        "formal" | "strict" | "professional" => Some(YandexEmotion::Strict),
                        "friendly" | "warm" | "kind" => Some(YandexEmotion::Friendly),
                        "whisper" | "quiet" => Some(YandexEmotion::Whisper),
                        _ => None,
                    })
            })
            .unwrap_or_default();

        // Default language based on voice
        let language = voice.default_language().parse().unwrap_or_default();

        Ok(Self {
            api_key,
            is_iam_token,
            folder_id,
            voice,
            language,
            emotion,
            speed,
            audio_format,
            sample_rate,
            ssml: false,
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone). Maps the features Yandex SpeechKit can
    /// express to real fields: [`TtsFeatures::speed`] is a `1.0`-normal multiplier applied directly
    /// to `speed` (reusing `from_base`'s `MIN_SPEED..=MAX_SPEED` clamp), [`TtsFeatures::emotion`] is
    /// matched to a [`YandexEmotion`] using the same string mapping as `from_base`,
    /// [`TtsFeatures::language`] overrides the [`YandexLanguage`] when it parses, and
    /// [`TtsFeatures::sample_rate`] is normalized to Yandex's supported rates (8000/16000/48000)
    /// using `from_base`'s bucketing, and [`TtsFeatures::ssml`] flips the input to the v1
    /// `:synthesize` `ssml` form field (instead of `text`). Yandex's non-standard `folder_id` and
    /// `is_iam_token` knobs are read from the `extras` passthrough. Features without a Yandex field
    /// (pitch, volume, stability, similarity_boost, style, use_speaker_boost, instructions,
    /// word_timestamps, streaming, seed) are skipped.
    ///
    /// [`TtsFeatures::speed`]: crate::core::tts::standard::TtsFeatures::speed
    /// [`TtsFeatures::emotion`]: crate::core::tts::standard::TtsFeatures::emotion
    /// [`TtsFeatures::language`]: crate::core::tts::standard::TtsFeatures::language
    /// [`TtsFeatures::sample_rate`]: crate::core::tts::standard::TtsFeatures::sample_rate
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;

        if let Some(speed) = f.speed {
            // Standardized speed is a 1.0-is-normal multiplier; Yandex uses the same scale.
            cfg.speed = speed.clamp(super::MIN_SPEED, super::MAX_SPEED);
        }
        if let Some(ref emotion) = f.emotion {
            // Reuse from_base's string -> YandexEmotion mapping; unknown values leave the base value.
            match emotion.to_lowercase().as_str() {
                "happy" | "cheerful" | "good" | "joyful" => cfg.emotion = YandexEmotion::Good,
                "angry" | "evil" | "irritated" | "frustrated" => cfg.emotion = YandexEmotion::Evil,
                "formal" | "strict" | "professional" => cfg.emotion = YandexEmotion::Strict,
                "friendly" | "warm" | "kind" => cfg.emotion = YandexEmotion::Friendly,
                "whisper" | "quiet" => cfg.emotion = YandexEmotion::Whisper,
                _ => {}
            }
        }
        if let Some(ref language) = f.language
            && let Ok(lang) = language.parse::<YandexLanguage>()
        {
            cfg.language = lang;
        }
        if let Some(sr) = f.sample_rate {
            // Reuse from_base's normalization to Yandex's supported rates.
            cfg.sample_rate = if sr == 0 {
                super::DEFAULT_SAMPLE_RATE
            } else if sr <= 12000 {
                super::MIN_SAMPLE_RATE
            } else if sr <= 32000 {
                16000
            } else {
                super::MAX_SAMPLE_RATE
            };
        }
        if let Some(ssml) = f.ssml {
            // SSML input is sent in the `ssml` form field instead of `text` on the v1 endpoint.
            cfg.ssml = ssml;
        }

        // Provider-specific passthrough.
        if let Some(folder_id) = std.extras.0.get("folder_id").and_then(|v| v.as_str()) {
            cfg.folder_id = Some(folder_id.to_string());
        }
        if let Some(is_iam_token) = std.extras.0.get("is_iam_token").and_then(|v| v.as_bool()) {
            cfg.is_iam_token = is_iam_token;
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);

        cfg.validate()?;
        Ok(cfg)
    }

    /// Build the form parameters for the synthesis request
    pub fn build_form_params(&self, text: &str) -> Vec<(&'static str, String)> {
        // The v1 `:synthesize` endpoint takes the input under EITHER `text` (plain) OR `ssml`
        // (SSML markup) — they are mutually exclusive (docs/providers/yandex.md "SSML Support").
        let input_field = if self.ssml { "ssml" } else { "text" };
        let mut params = vec![
            (input_field, text.to_string()),
            ("voice", self.voice.to_string()),
            ("lang", self.language.to_string()),
            ("format", self.audio_format.to_string()),
        ];

        // Add emotion if not neutral
        if self.emotion != YandexEmotion::Neutral {
            params.push(("emotion", self.emotion.to_string()));
        }

        // Add speed if not default
        if (self.speed - super::DEFAULT_SPEED).abs() > 0.01 {
            params.push(("speed", format!("{:.1}", self.speed)));
        }

        // Add sample rate only for LPCM format
        if self.audio_format == YandexAudioFormat::Lpcm {
            params.push(("sampleRateHertz", self.sample_rate.to_string()));
        }

        // Add folder ID if available
        if let Some(ref folder_id) = self.folder_id {
            params.push(("folderId", folder_id.clone()));
        }

        params
    }

    /// Get the Authorization header value
    pub fn auth_header_value(&self) -> String {
        if self.is_iam_token {
            format!("Bearer {}", self.api_key)
        } else {
            format!("Api-Key {}", self.api_key)
        }
    }

    /// Validate text length
    pub fn validate_text(&self, text: &str) -> TTSResult<()> {
        if text.len() > super::MAX_TEXT_LENGTH {
            return Err(TTSError::InvalidConfiguration(format!(
                "Text length {} exceeds maximum {} characters",
                text.len(),
                super::MAX_TEXT_LENGTH
            )));
        }
        Ok(())
    }

    /// Validate provider-specific URL surfaces.
    pub fn validate(&self) -> TTSResult<()> {
        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_yandex_tts_endpoint("endpoint_override", endpoint)
                .map_err(TTSError::InvalidConfiguration)?;
        }
        Ok(())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features Yandex can express (speed, emotion, language, output
    // sample rate) reach their real config fields, and the non-standard folder_id / is_iam_token
    // knobs flow through the extras passthrough.
    #[test]
    fn from_standard_maps_speed_emotion_language_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("folder_id".into(), serde_json::json!("b1gfolder123"));
        extras.insert("is_iam_token".into(), serde_json::json!(true));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "yandex".into(),
                api_key: "AQVN1234567890".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(1.5),
                emotion: Some("cheerful".into()),
                language: Some("en-US".into()),
                sample_rate: Some(16000),
                ssml: Some(true), // now a real field: routes input to the `ssml` form param
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = YandexTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 1.5); // 1.5 multiplier clamped to 0.1..=3.0
        assert_eq!(cfg.emotion, YandexEmotion::Good); // "cheerful" -> Good
        assert_eq!(cfg.language, YandexLanguage::English); // "en-US" -> English
        assert_eq!(cfg.sample_rate, 16000); // 16000 -> 16000 bucket
        assert!(cfg.ssml); // ssml feature mapped onto the config
        assert_eq!(cfg.folder_id, Some("b1gfolder123".to_string())); // extras passthrough
        assert!(cfg.is_iam_token);
    }

    // WIRE-LEVEL guard (the recurring bug class is asserting only the config struct). This asserts
    // the `ssml` feature actually reaches the v1 `:synthesize` form BODY: the synthesis input is
    // emitted under the `ssml` form key and NOT under `text` (the two are mutually exclusive per the
    // Yandex v1 REST API). Without the wiring in `build_form_params` the input would stay under
    // `text` and this test fails.
    #[test]
    fn ssml_feature_reaches_synthesize_form_body_under_ssml_key() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let ssml_text = "<speak>Привет<break time=\"500ms\"/>мир</speak>";

        // Plain (ssml = false): input is `text`, no `ssml` key.
        let plain = YandexTtsConfig::from_standard(&StandardTTSConfig {
            base: TTSConfig {
                provider: "yandex".into(),
                api_key: "AQVN1234567890".into(),
                voice_id: Some("alena".into()),
                ..Default::default()
            },
            features: TtsFeatures::default(),
            extras: Default::default(),
        })
        .unwrap();
        let plain_params = plain.build_form_params(ssml_text);
        assert!(
            plain_params
                .iter()
                .any(|(k, v)| *k == "text" && v == ssml_text),
            "plain input must be under `text`"
        );
        assert!(
            !plain_params.iter().any(|(k, _)| *k == "ssml"),
            "no `ssml` key when the feature is off"
        );

        // ssml = true: input moves to the `ssml` form key and `text` is absent.
        let ssml_cfg = YandexTtsConfig::from_standard(&StandardTTSConfig {
            base: TTSConfig {
                provider: "yandex".into(),
                api_key: "AQVN1234567890".into(),
                voice_id: Some("alena".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(true),
                ..Default::default()
            },
            extras: Default::default(),
        })
        .unwrap();
        let ssml_params = ssml_cfg.build_form_params(ssml_text);
        assert!(
            ssml_params
                .iter()
                .any(|(k, v)| *k == "ssml" && v == ssml_text),
            "ssml input must reach the `ssml` form key"
        );
        assert!(
            !ssml_params.iter().any(|(k, _)| *k == "text"),
            "`text` must be omitted when sending SSML (mutually exclusive)"
        );
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};

        let mk = |endpoint: &str| {
            StandardTTSConfig {
                base: TTSConfig {
                    provider: "yandex".into(),
                    api_key: "test-key".into(),
                    ..Default::default()
                },
                features: TtsFeatures::default(),
                extras: ProviderExtras::default(),
            }
            .with_endpoint_override(endpoint)
        };

        assert!(YandexTtsConfig::from_standard(&mk("https://tts-proxy.example.com")).is_ok());
        assert!(YandexTtsConfig::from_standard(&mk("http://127.0.0.1:9000")).is_err());
        assert!(YandexTtsConfig::from_standard(&mk("file:///tmp/socket")).is_err());
        assert!(YandexTtsConfig::from_standard(&mk("wss://tts-proxy.example.com")).is_err());
    }

    #[test]
    fn test_audio_format_conversion() {
        assert_eq!(YandexAudioFormat::Lpcm.as_str(), "lpcm");
        assert_eq!(YandexAudioFormat::OggOpus.as_str(), "oggopus");
        assert_eq!(YandexAudioFormat::Mp3.as_str(), "mp3");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            "lpcm".parse::<YandexAudioFormat>().unwrap(),
            YandexAudioFormat::Lpcm
        );
        assert_eq!(
            "oggopus".parse::<YandexAudioFormat>().unwrap(),
            YandexAudioFormat::OggOpus
        );
        assert_eq!(
            "mp3".parse::<YandexAudioFormat>().unwrap(),
            YandexAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_aliases() {
        assert_eq!(
            "pcm".parse::<YandexAudioFormat>().unwrap(),
            YandexAudioFormat::Lpcm
        );
        assert_eq!(
            "ogg".parse::<YandexAudioFormat>().unwrap(),
            YandexAudioFormat::OggOpus
        );
        assert_eq!(
            "mpeg".parse::<YandexAudioFormat>().unwrap(),
            YandexAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_invalid() {
        assert!("flac".parse::<YandexAudioFormat>().is_err());
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!("alena".parse::<YandexVoice>().unwrap(), YandexVoice::Alena);
        assert_eq!("john".parse::<YandexVoice>().unwrap(), YandexVoice::John);
        assert_eq!("lea".parse::<YandexVoice>().unwrap(), YandexVoice::Lea);
    }

    #[test]
    fn test_voice_custom() {
        let voice: YandexVoice = "custom_voice".parse().unwrap();
        assert_eq!(voice.as_str(), "custom_voice");
    }

    #[test]
    fn test_voice_default_language() {
        assert_eq!(YandexVoice::Alena.default_language(), "ru-RU");
        assert_eq!(YandexVoice::John.default_language(), "en-US");
        assert_eq!(YandexVoice::Lea.default_language(), "de-DE");
        assert_eq!(YandexVoice::Naomi.default_language(), "he-IL");
        assert_eq!(YandexVoice::Amira.default_language(), "kk-KZ");
        assert_eq!(YandexVoice::Nigora.default_language(), "uz-UZ");
    }

    #[test]
    fn test_emotion_from_str() {
        assert_eq!(
            "neutral".parse::<YandexEmotion>().unwrap(),
            YandexEmotion::Neutral
        );
        assert_eq!(
            "good".parse::<YandexEmotion>().unwrap(),
            YandexEmotion::Good
        );
        assert_eq!(
            "evil".parse::<YandexEmotion>().unwrap(),
            YandexEmotion::Evil
        );
        assert_eq!(
            "whisper".parse::<YandexEmotion>().unwrap(),
            YandexEmotion::Whisper
        );
    }

    #[test]
    fn test_emotion_aliases() {
        assert_eq!(
            "cheerful".parse::<YandexEmotion>().unwrap(),
            YandexEmotion::Good
        );
        assert_eq!(
            "angry".parse::<YandexEmotion>().unwrap(),
            YandexEmotion::Evil
        );
        assert_eq!(
            "formal".parse::<YandexEmotion>().unwrap(),
            YandexEmotion::Strict
        );
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(
            "ru-RU".parse::<YandexLanguage>().unwrap(),
            YandexLanguage::Russian
        );
        assert_eq!(
            "en-US".parse::<YandexLanguage>().unwrap(),
            YandexLanguage::English
        );
        assert_eq!(
            "de".parse::<YandexLanguage>().unwrap(),
            YandexLanguage::German
        );
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("alena".to_string()),
            audio_format: Some("mp3".to_string()),
            speaking_rate: Some(1.5),
            ..Default::default()
        };

        let config = YandexTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.api_key, "test-api-key");
        assert!(!config.is_iam_token);
        assert_eq!(config.voice, YandexVoice::Alena);
        assert_eq!(config.audio_format, YandexAudioFormat::Mp3);
        assert_eq!(config.speed, 1.5);
    }

    #[test]
    fn test_config_with_folder_id() {
        let base = TTSConfig {
            api_key: "b1g12345678:AQVN1234567890".to_string(),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };

        let config = YandexTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.folder_id, Some("b1g12345678".to_string()));
        assert_eq!(config.api_key, "AQVN1234567890");
    }

    #[test]
    fn test_config_defaults() {
        let base = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };

        let config = YandexTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.voice, YandexVoice::Alena);
        assert_eq!(config.audio_format, YandexAudioFormat::OggOpus);
        assert_eq!(config.sample_rate, 48000);
        assert_eq!(config.speed, 1.0);
        assert_eq!(config.emotion, YandexEmotion::Neutral);
    }

    #[test]
    fn test_config_requires_api_key() {
        let base = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = YandexTtsConfig::from_base(&base);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_form_params() {
        let base = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: Some("alena".to_string()),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let config = YandexTtsConfig::from_base(&base).unwrap();
        let params = config.build_form_params("Hello");

        assert!(params.iter().any(|(k, v)| *k == "text" && v == "Hello"));
        assert!(params.iter().any(|(k, v)| *k == "voice" && v == "alena"));
        assert!(params.iter().any(|(k, v)| *k == "format" && v == "mp3"));
    }

    #[test]
    fn test_build_form_params_with_emotion() {
        let base = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };

        let mut config = YandexTtsConfig::from_base(&base).unwrap();
        config.emotion = YandexEmotion::Good;

        let params = config.build_form_params("Hello");
        assert!(params.iter().any(|(k, v)| *k == "emotion" && v == "good"));
    }

    #[test]
    fn test_auth_header_api_key() {
        let base = TTSConfig {
            api_key: "AQVN1234567890".to_string(),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };

        let config = YandexTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.auth_header_value(), "Api-Key AQVN1234567890");
    }

    #[test]
    fn test_validate_text() {
        let base = TTSConfig {
            api_key: "test-api-key".to_string(),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };
        let config = YandexTtsConfig::from_base(&base).unwrap();

        // Valid text
        assert!(config.validate_text("Hello world").is_ok());

        // Too long text
        let long_text = "x".repeat(crate::core::tts::yandex::MAX_TEXT_LENGTH + 1);
        assert!(config.validate_text(&long_text).is_err());
    }

    #[test]
    fn test_sample_rate_normalization() {
        // Low sample rate -> 8000
        let base = TTSConfig {
            api_key: "test".to_string(),
            sample_rate: Some(8000),
            voice_id: None,
            audio_format: None,
            ..Default::default()
        };
        let config = YandexTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.sample_rate, 8000);

        // Medium sample rate -> 16000
        let base = TTSConfig {
            api_key: "test".to_string(),
            sample_rate: Some(16000),
            voice_id: None,
            audio_format: None,
            ..Default::default()
        };
        let config = YandexTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.sample_rate, 16000);

        // High sample rate -> 48000
        let base = TTSConfig {
            api_key: "test".to_string(),
            sample_rate: Some(44100),
            voice_id: None,
            audio_format: None,
            ..Default::default()
        };
        let config = YandexTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.sample_rate, 48000);
    }
}
