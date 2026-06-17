//! SberDevices SaluteSpeech STT Configuration
//!
//! Configuration types for SberDevices SaluteSpeech Speech-to-Text API.
//! Supports OAuth 2.0 authentication with automatic token refresh.

use crate::core::stt::base::{STTConfig, STTError};
use std::str::FromStr;

// =============================================================================
// Constants
// =============================================================================

/// OAuth 2.0 endpoint for token retrieval
pub const OAUTH_ENDPOINT: &str = "https://ngw.devices.sberbank.ru:9443/api/v2/oauth";

/// STT synchronous recognition endpoint
pub const STT_RECOGNIZE_ENDPOINT: &str = "https://smartspeech.sber.ru/rest/v1/speech:recognize";

/// Token validity duration in seconds (30 minutes)
pub const TOKEN_VALIDITY_SECS: u64 = 1800;

/// Token refresh threshold in seconds (refresh when < 60 seconds remaining)
pub const TOKEN_REFRESH_THRESHOLD_SECS: u64 = 60;

/// Maximum audio size for synchronous recognition (2 MB)
pub const MAX_SYNC_AUDIO_SIZE: usize = 2 * 1024 * 1024;

/// Maximum audio duration for synchronous recognition (60 seconds)
pub const MAX_SYNC_AUDIO_DURATION_SECS: u32 = 60;

/// Default sample rate in Hz
pub const DEFAULT_SAMPLE_RATE: u32 = 16000;

// =============================================================================
// Audio Format
// =============================================================================

/// Supported audio formats for SberDevices STT
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SberSTTAudioFormat {
    /// Linear PCM 16-bit signed little-endian (default)
    #[default]
    Pcm16,
    /// OPUS codec
    Opus,
    /// MP3 format
    Mp3,
    /// FLAC format
    Flac,
    /// A-law (G.711)
    Alaw,
    /// Mu-law (G.711)
    Mulaw,
}

impl SberSTTAudioFormat {
    /// Get the content-type header value for this format
    pub fn content_type(&self, sample_rate: u32) -> String {
        match self {
            Self::Pcm16 => format!("audio/x-pcm;bit=16;rate={}", sample_rate),
            Self::Opus => "audio/opus".to_string(),
            Self::Mp3 => "audio/mpeg".to_string(),
            Self::Flac => "audio/flac".to_string(),
            Self::Alaw => format!("audio/x-alaw-basic;rate={}", sample_rate),
            Self::Mulaw => format!("audio/x-mulaw;rate={}", sample_rate),
        }
    }

    /// Get the API format string
    pub fn as_api_str(&self) -> &'static str {
        match self {
            Self::Pcm16 => "PCM_S16LE",
            Self::Opus => "OPUS",
            Self::Mp3 => "MP3",
            Self::Flac => "FLAC",
            Self::Alaw => "ALAW",
            Self::Mulaw => "MULAW",
        }
    }
}

impl FromStr for SberSTTAudioFormat {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "pcm" | "pcm16" | "pcm_s16le" | "linear16" | "lpcm" | "raw" => Ok(Self::Pcm16),
            "opus" | "ogg_opus" | "oggopus" => Ok(Self::Opus),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "flac" => Ok(Self::Flac),
            "alaw" | "g711a" => Ok(Self::Alaw),
            "mulaw" | "ulaw" | "g711u" => Ok(Self::Mulaw),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid SberDevices STT audio format: '{}'. Valid: pcm, opus, mp3, flac, alaw, mulaw",
                s
            ))),
        }
    }
}

// =============================================================================
// Language Configuration
// =============================================================================

/// Supported languages for SberDevices STT
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SberSTTLanguage {
    /// Russian (default)
    #[default]
    Russian,
    /// English (US)
    English,
    /// Kazakh
    Kazakh,
    /// Kyrgyz
    Kyrgyz,
    /// Uzbek
    Uzbek,
}

impl SberSTTLanguage {
    /// Get the language code for the API
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::Russian => "ru-RU",
            Self::English => "en-US",
            Self::Kazakh => "kk-KZ",
            Self::Kyrgyz => "ky-KG",
            Self::Uzbek => "uz-UZ",
        }
    }
}

impl FromStr for SberSTTLanguage {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().replace('_', "-").trim() {
            "ru" | "ru-ru" | "russian" => Ok(Self::Russian),
            "en" | "en-us" | "english" => Ok(Self::English),
            "kk" | "kk-kz" | "kazakh" => Ok(Self::Kazakh),
            "ky" | "ky-kg" | "kyrgyz" => Ok(Self::Kyrgyz),
            "uz" | "uz-uz" | "uzbek" => Ok(Self::Uzbek),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid SberDevices STT language: '{}'. Valid: ru-RU, en-US, kk-KZ, ky-KG, uz-UZ",
                s
            ))),
        }
    }
}

// =============================================================================
// OAuth Scope
// =============================================================================

/// OAuth scope for SaluteSpeech API access
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SberScope {
    /// Personal (individuals) - 5 concurrent streams
    #[default]
    Personal,
    /// Corporate (organizations postpaid) - 10 concurrent streams
    Corporate,
    /// B2B (organizations prepaid)
    B2B,
    /// Legacy enterprise
    Legacy,
}

impl SberScope {
    /// Get the scope string for OAuth request
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Personal => "SALUTE_SPEECH_PERS",
            Self::Corporate => "SALUTE_SPEECH_CORP",
            Self::B2B => "SALUTE_SPEECH_B2B",
            Self::Legacy => "SBER_SPEECH",
        }
    }
}

impl FromStr for SberScope {
    type Err = STTError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "SALUTE_SPEECH_PERS" | "PERS" | "PERSONAL" => Ok(Self::Personal),
            "SALUTE_SPEECH_CORP" | "CORP" | "CORPORATE" => Ok(Self::Corporate),
            "SALUTE_SPEECH_B2B" | "B2B" => Ok(Self::B2B),
            "SBER_SPEECH" | "LEGACY" => Ok(Self::Legacy),
            _ => Err(STTError::ConfigurationError(format!(
                "Invalid SberDevices scope: '{}'. Valid: SALUTE_SPEECH_PERS, SALUTE_SPEECH_CORP, SALUTE_SPEECH_B2B, SBER_SPEECH",
                s
            ))),
        }
    }
}

// =============================================================================
// SberDevices STT Configuration
// =============================================================================

/// SberDevices SaluteSpeech STT Configuration
#[derive(Debug, Clone)]
pub struct SberSTTConfig {
    /// Client credentials (Base64 encoded client_id:client_secret)
    pub client_credentials: String,
    /// OAuth scope
    pub scope: SberScope,
    /// Language for recognition
    pub language: SberSTTLanguage,
    /// Audio format
    pub audio_format: SberSTTAudioFormat,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Enable automatic punctuation
    pub enable_punctuation: bool,
    /// Connection timeout in seconds
    pub connection_timeout_secs: u64,
    /// Request timeout in seconds
    pub request_timeout_secs: u64,
    // -------------------------------------------------------------------------
    // Standardized recognition options emitted as `speech:recognize` query params.
    // -------------------------------------------------------------------------
    /// Enable profanity filtering/masking. SaluteSpeech `speech:recognize` `enable_profanity_filter`
    /// query param. Mapped from the typed `SttFeatures::profanity_filter`.
    pub enable_profanity_filter: bool,
    /// Number of audio channels to transcribe independently (per-channel transcription).
    /// SaluteSpeech `speech:recognize` `channels_count` query param. `None` => omitted (provider
    /// default, mono). Driven by the typed `SttFeatures::multichannel` flag (count from the base
    /// `channels`).
    pub channels_count: Option<u16>,
    /// Acoustic recognition model / domain (e.g. "general", "callcenter", "media"). SaluteSpeech
    /// `speech:recognize` `model` query param. `None` => omitted (provider default). Carried via the
    /// standardized `extras` passthrough (`extras["model"]`) — distinct from the OAuth `scope`
    /// (which the flat `STTConfig::model` field already reuses).
    pub recognition_model: Option<String>,
    /// Base endpoint override (scheme://host) from the standardized `endpoint_override` — rewrites
    /// BOTH the OAuth token host and the `speech:recognize` host to an in-repo mock/proxy for
    /// credential-free e2e (paths preserved). `None` uses the production Sber hosts.
    pub endpoint_override: Option<String>,
}

impl SberSTTConfig {
    /// Create configuration from base STTConfig
    ///
    /// The api_key should be in format "client_id:client_secret" or
    /// already Base64 encoded credentials.
    pub fn from_base(config: &STTConfig) -> Result<Self, STTError> {
        // Validate API key
        if config.api_key.is_empty() {
            return Err(STTError::ConfigurationError(
                "SberDevices requires client credentials (client_id:client_secret)".to_string(),
            ));
        }

        // Check if already Base64 encoded or needs encoding
        let client_credentials = if config.api_key.contains(':') {
            // Raw format - encode to Base64
            use base64::{Engine, engine::general_purpose::STANDARD};
            STANDARD.encode(config.api_key.as_bytes())
        } else {
            // Assume already encoded
            config.api_key.clone()
        };

        // Parse language
        let language = if config.language.is_empty() {
            SberSTTLanguage::default()
        } else {
            config.language.parse().unwrap_or_default()
        };

        // Parse audio format
        let audio_format = if config.encoding.is_empty() {
            SberSTTAudioFormat::default()
        } else {
            config.encoding.parse().unwrap_or_default()
        };

        // Validate sample rate
        let sample_rate = if config.sample_rate == 0 {
            DEFAULT_SAMPLE_RATE
        } else {
            config.sample_rate
        };

        // Parse scope from model field
        let scope = if config.model.is_empty() {
            SberScope::default()
        } else {
            config.model.parse().unwrap_or_default()
        };

        Ok(Self {
            client_credentials,
            scope,
            language,
            audio_format,
            sample_rate,
            enable_punctuation: config.punctuation,
            connection_timeout_secs: 30,
            request_timeout_secs: 60,
            // Standardized recognition options are absent on the flat path (no features/extras).
            enable_profanity_filter: false,
            channels_count: None,
            recognition_model: None,
            endpoint_override: None,
        })
    }

    /// Build from the standardized config (W1 keystone). SberDevices SaluteSpeech is a synchronous
    /// REST recognizer (`speech:recognize`); the options it accepts are emitted as query params on
    /// the recognize URL. This maps the standardized features it can express:
    /// - `profanity_filter` (typed) -> `enable_profanity_filter` query param,
    /// - `multichannel` (typed) -> `channels_count` query param (per-channel transcription; the
    ///   count is taken from the base `channels`),
    /// - `extras["model"]` -> `model` query param (acoustic recognition domain — distinct from the
    ///   OAuth `scope`, which the flat `STTConfig::model` field reuses).
    ///
    /// Streaming-only knobs (interim_results, diarization, vad/endpointing, etc.) are capability
    /// gaps for this synchronous endpoint and stay at provider defaults.
    pub fn from_standard(
        std: &crate::core::stt::standard::StandardSTTConfig,
    ) -> Result<Self, STTError> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;
        if let Some(p) = f.profanity_filter {
            cfg.enable_profanity_filter = p;
        }
        // Per-channel transcription: when `multichannel` is requested, emit the channel count from
        // the base config (default to 1 if the base left it at 0). `None`/false -> mono default.
        if f.multichannel == Some(true) {
            let channels = if std.base.channels == 0 {
                1
            } else {
                std.base.channels
            };
            cfg.channels_count = Some(channels);
        }
        // Acoustic recognition model/domain via the open passthrough (`extras["model"]`).
        if let Some(m) = std.extras.0.get("model").and_then(|v| v.as_str()) {
            cfg.recognition_model = Some(m.to_string());
        }
        // Standardized endpoint override → both Sber hosts (OAuth + recognize) for mock e2e.
        cfg.endpoint_override = std.endpoint_override().map(|s| s.to_string());
        Ok(cfg)
    }

    /// Build the `speech:recognize` request URL with the recognition options as query params.
    ///
    /// SaluteSpeech accepts recognition options as query parameters on the POST URL (the audio is
    /// the request body). `language` and `sample_rate` are always emitted; the standardized
    /// options (`enable_profanity_filter`, `channels_count`, `model`) are emitted only when set so
    /// unset knobs fall back to the provider default. This is the URL that actually reaches Sber.
    pub fn recognize_url(&self) -> String {
        // When overridden, keep the production path (`/rest/v1/speech:recognize`) but swap the host.
        let base = match self.endpoint_override {
            Some(ref ov) => format!("{}/rest/v1/speech:recognize", ov.trim_end_matches('/')),
            None => STT_RECOGNIZE_ENDPOINT.to_string(),
        };
        let mut url = format!(
            "{}?language={}&sample_rate={}",
            base,
            self.language.as_code(),
            self.sample_rate
        );
        if self.enable_profanity_filter {
            url.push_str("&enable_profanity_filter=true");
        }
        if let Some(channels) = self.channels_count {
            url.push_str(&format!("&channels_count={}", channels));
        }
        if let Some(ref model) = self.recognition_model {
            url.push_str(&format!("&model={}", model));
        }
        url
    }

    /// Get the Authorization header for OAuth token request
    pub fn oauth_auth_header(&self) -> String {
        format!("Basic {}", self.client_credentials)
    }

    /// Build the OAuth token URL. When `endpoint_override` is set, the production OAuth path
    /// (`/api/v2/oauth`) is preserved but the host is swapped to the override (mock e2e); otherwise
    /// the production `OAUTH_ENDPOINT` is used.
    pub fn oauth_url(&self) -> String {
        match self.endpoint_override {
            Some(ref ov) => format!("{}/api/v2/oauth", ov.trim_end_matches('/')),
            None => OAUTH_ENDPOINT.to_string(),
        }
    }

    /// Get the content-type for audio data
    pub fn audio_content_type(&self) -> String {
        self.audio_format.content_type(self.sample_rate)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: SberDevices maps the standardized features its synchronous `speech:recognize`
    // endpoint can express (profanity filter, per-channel transcription, acoustic model). Streaming
    // knobs (diarization, interim_results) remain capability gaps. The base still carries through.
    #[test]
    fn from_standard_passthrough_carries_base() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "sberdevices".into(),
                api_key: "test_client_id:test_client_secret".into(),
                language: "ru-RU".into(),
                ..Default::default()
            },
            // Streaming-only features are set but SberDevices cannot express them; they are ignored.
            features: SttFeatures {
                diarization: Some(true),
                interim_results: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras::default(),
            translation: None,
        };
        let cfg = SberSTTConfig::from_standard(&std).unwrap();
        // Base carried through: credentials encoded and language parsed, exactly like from_base.
        use base64::{Engine, engine::general_purpose::STANDARD};
        let expected = STANDARD.encode("test_client_id:test_client_secret".as_bytes());
        assert_eq!(cfg.client_credentials, expected);
        assert_eq!(cfg.language, SberSTTLanguage::Russian);
        // Capability gaps: streaming knobs do not touch the recognize options.
        assert!(!cfg.enable_profanity_filter);
        assert_eq!(cfg.channels_count, None);
        assert_eq!(cfg.recognition_model, None);
    }

    // WIRE-LEVEL: profanity filter (typed `profanity_filter`), per-channel transcription (typed
    // `multichannel` -> `channels_count` from the base `channels`) and the acoustic recognition
    // model (`extras["model"]`) must travel from the standardized config onto the actual
    // `speech:recognize` request URL — the bytes that reach Sber — not merely the config struct.
    // Guards the recurring "set on the struct but never emitted to the wire" gap class.
    #[test]
    fn standardized_options_reach_recognize_url() {
        use crate::core::stt::standard::{ProviderExtras, SttFeatures, StandardSTTConfig};
        let mut extras = serde_json::Map::new();
        extras.insert("model".into(), serde_json::json!("callcenter"));
        let std = StandardSTTConfig {
            base: STTConfig {
                provider: "sberdevices".into(),
                api_key: "test_client_id:test_client_secret".into(),
                language: "ru-RU".into(),
                sample_rate: 16000,
                channels: 2,
                ..Default::default()
            },
            features: SttFeatures {
                profanity_filter: Some(true),
                multichannel: Some(true),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
            translation: None,
        };
        let cfg = SberSTTConfig::from_standard(&std).unwrap();
        // Config carried the mapping...
        assert!(cfg.enable_profanity_filter);
        assert_eq!(cfg.channels_count, Some(2));
        assert_eq!(cfg.recognition_model, Some("callcenter".to_string()));
        // ...and crucially the params reach the wire (recognize URL).
        let url = cfg.recognize_url();
        assert!(
            url.starts_with("https://smartspeech.sber.ru/rest/v1/speech:recognize?"),
            "url: {url}"
        );
        assert!(
            url.contains("enable_profanity_filter=true"),
            "profanity filter missing from URL: {url}"
        );
        assert!(
            url.contains("channels_count=2"),
            "channels_count missing from URL: {url}"
        );
        assert!(url.contains("model=callcenter"), "model missing from URL: {url}");
        // Sanity: base language + sample_rate are always emitted.
        assert!(url.contains("language=ru-RU"), "language missing: {url}");
        assert!(url.contains("sample_rate=16000"), "sample_rate missing: {url}");

        // And when the features are NOT requested, the optional params are omitted (defaults).
        let bare = SberSTTConfig::from_standard(&StandardSTTConfig::from_base(STTConfig {
            api_key: "c:s".into(),
            language: "ru-RU".into(),
            sample_rate: 16000,
            ..Default::default()
        }))
        .unwrap()
        .recognize_url();
        assert!(!bare.contains("enable_profanity_filter"), "url: {bare}");
        assert!(!bare.contains("channels_count"), "url: {bare}");
        assert!(!bare.contains("model="), "url: {bare}");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            "pcm".parse::<SberSTTAudioFormat>().unwrap(),
            SberSTTAudioFormat::Pcm16
        );
        assert_eq!(
            "linear16".parse::<SberSTTAudioFormat>().unwrap(),
            SberSTTAudioFormat::Pcm16
        );
        assert_eq!(
            "opus".parse::<SberSTTAudioFormat>().unwrap(),
            SberSTTAudioFormat::Opus
        );
        assert_eq!(
            "mp3".parse::<SberSTTAudioFormat>().unwrap(),
            SberSTTAudioFormat::Mp3
        );
        assert_eq!(
            "flac".parse::<SberSTTAudioFormat>().unwrap(),
            SberSTTAudioFormat::Flac
        );
        assert_eq!(
            "alaw".parse::<SberSTTAudioFormat>().unwrap(),
            SberSTTAudioFormat::Alaw
        );
        assert_eq!(
            "mulaw".parse::<SberSTTAudioFormat>().unwrap(),
            SberSTTAudioFormat::Mulaw
        );
    }

    #[test]
    fn test_audio_format_content_type() {
        assert_eq!(
            SberSTTAudioFormat::Pcm16.content_type(16000),
            "audio/x-pcm;bit=16;rate=16000"
        );
        assert_eq!(SberSTTAudioFormat::Opus.content_type(16000), "audio/opus");
        assert_eq!(SberSTTAudioFormat::Mp3.content_type(16000), "audio/mpeg");
        assert_eq!(SberSTTAudioFormat::Flac.content_type(16000), "audio/flac");
        assert_eq!(
            SberSTTAudioFormat::Alaw.content_type(8000),
            "audio/x-alaw-basic;rate=8000"
        );
        assert_eq!(
            SberSTTAudioFormat::Mulaw.content_type(8000),
            "audio/x-mulaw;rate=8000"
        );
    }

    #[test]
    fn test_audio_format_invalid() {
        assert!("wav".parse::<SberSTTAudioFormat>().is_err());
        assert!("invalid".parse::<SberSTTAudioFormat>().is_err());
    }

    #[test]
    fn test_language_from_str() {
        assert_eq!(
            "ru-RU".parse::<SberSTTLanguage>().unwrap(),
            SberSTTLanguage::Russian
        );
        assert_eq!(
            "ru".parse::<SberSTTLanguage>().unwrap(),
            SberSTTLanguage::Russian
        );
        assert_eq!(
            "en-US".parse::<SberSTTLanguage>().unwrap(),
            SberSTTLanguage::English
        );
        assert_eq!(
            "kk-KZ".parse::<SberSTTLanguage>().unwrap(),
            SberSTTLanguage::Kazakh
        );
        assert_eq!(
            "ky-KG".parse::<SberSTTLanguage>().unwrap(),
            SberSTTLanguage::Kyrgyz
        );
        assert_eq!(
            "uz-UZ".parse::<SberSTTLanguage>().unwrap(),
            SberSTTLanguage::Uzbek
        );
    }

    #[test]
    fn test_language_code() {
        assert_eq!(SberSTTLanguage::Russian.as_code(), "ru-RU");
        assert_eq!(SberSTTLanguage::English.as_code(), "en-US");
        assert_eq!(SberSTTLanguage::Kazakh.as_code(), "kk-KZ");
        assert_eq!(SberSTTLanguage::Kyrgyz.as_code(), "ky-KG");
        assert_eq!(SberSTTLanguage::Uzbek.as_code(), "uz-UZ");
    }

    #[test]
    fn test_language_invalid() {
        assert!("de-DE".parse::<SberSTTLanguage>().is_err());
        assert!("fr-FR".parse::<SberSTTLanguage>().is_err());
    }

    #[test]
    fn test_scope_from_str() {
        assert_eq!(
            "SALUTE_SPEECH_PERS".parse::<SberScope>().unwrap(),
            SberScope::Personal
        );
        assert_eq!("PERS".parse::<SberScope>().unwrap(), SberScope::Personal);
        assert_eq!(
            "SALUTE_SPEECH_CORP".parse::<SberScope>().unwrap(),
            SberScope::Corporate
        );
        assert_eq!(
            "SALUTE_SPEECH_B2B".parse::<SberScope>().unwrap(),
            SberScope::B2B
        );
        assert_eq!(
            "SBER_SPEECH".parse::<SberScope>().unwrap(),
            SberScope::Legacy
        );
    }

    #[test]
    fn test_scope_as_str() {
        assert_eq!(SberScope::Personal.as_str(), "SALUTE_SPEECH_PERS");
        assert_eq!(SberScope::Corporate.as_str(), "SALUTE_SPEECH_CORP");
        assert_eq!(SberScope::B2B.as_str(), "SALUTE_SPEECH_B2B");
        assert_eq!(SberScope::Legacy.as_str(), "SBER_SPEECH");
    }

    #[test]
    fn test_config_from_base_raw_credentials() {
        let base = STTConfig {
            api_key: "test_client_id:test_client_secret".to_string(),
            language: "ru-RU".to_string(),
            sample_rate: 16000,
            encoding: "pcm".to_string(),
            punctuation: true,
            ..Default::default()
        };

        let config = SberSTTConfig::from_base(&base).unwrap();

        // Should be Base64 encoded
        use base64::{Engine, engine::general_purpose::STANDARD};
        let expected = STANDARD.encode("test_client_id:test_client_secret".as_bytes());
        assert_eq!(config.client_credentials, expected);
        assert_eq!(config.language, SberSTTLanguage::Russian);
        assert_eq!(config.audio_format, SberSTTAudioFormat::Pcm16);
        assert_eq!(config.sample_rate, 16000);
        assert!(config.enable_punctuation);
    }

    #[test]
    fn test_config_from_base_encoded_credentials() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let encoded = STANDARD.encode("client:secret".as_bytes());

        let base = STTConfig {
            api_key: encoded.clone(),
            language: "en-US".to_string(),
            sample_rate: 8000,
            encoding: "opus".to_string(),
            ..Default::default()
        };

        let config = SberSTTConfig::from_base(&base).unwrap();

        // Should use as-is (no colon means already encoded)
        assert_eq!(config.client_credentials, encoded);
        assert_eq!(config.language, SberSTTLanguage::English);
        assert_eq!(config.audio_format, SberSTTAudioFormat::Opus);
    }

    #[test]
    fn test_config_requires_credentials() {
        let base = STTConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = SberSTTConfig::from_base(&base);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_defaults() {
        // Test SberDevices defaults when fields are empty
        let base = STTConfig {
            api_key: "test:secret".to_string(),
            language: String::new(), // Empty to test SberDevices default (Russian)
            encoding: String::new(), // Empty to test SberDevices default (Pcm16)
            model: String::new(),    // Empty to test SberDevices default (Personal scope)
            sample_rate: 0,          // Zero to test SberDevices default (16000)
            ..Default::default()
        };

        let config = SberSTTConfig::from_base(&base).unwrap();
        assert_eq!(config.language, SberSTTLanguage::Russian);
        assert_eq!(config.audio_format, SberSTTAudioFormat::Pcm16);
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.scope, SberScope::Personal);
    }

    #[test]
    fn test_oauth_auth_header() {
        let base = STTConfig {
            api_key: "test:secret".to_string(),
            ..Default::default()
        };

        let config = SberSTTConfig::from_base(&base).unwrap();
        let header = config.oauth_auth_header();

        assert!(header.starts_with("Basic "));
        assert!(header.len() > 10);
    }

    #[test]
    fn test_config_with_scope() {
        let base = STTConfig {
            api_key: "test:secret".to_string(),
            model: "SALUTE_SPEECH_CORP".to_string(),
            ..Default::default()
        };

        let config = SberSTTConfig::from_base(&base).unwrap();
        assert_eq!(config.scope, SberScope::Corporate);
    }
}
