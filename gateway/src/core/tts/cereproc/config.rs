//! CereProc CereVoice Cloud Configuration Types
//!
//! This module defines configuration structures for the CereProc TTS provider,
//! including credentials, audio format options, and voice settings.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::core::tts::base::{TTSConfig, TTSError, TTSResult};

fn validate_cereproc_tts_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
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

/// Supported audio output formats for CereVoice Cloud
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CereprocAudioFormat {
    /// WAV audio format
    #[default]
    Wav,
    /// MP3 audio format
    Mp3,
    /// OGG Vorbis format
    Ogg,
    /// Raw PCM audio
    Raw,
}

impl CereprocAudioFormat {
    /// Get the format string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Ogg => "ogg",
            Self::Raw => "raw",
        }
    }

    /// Get the file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => ".wav",
            Self::Mp3 => ".mp3",
            Self::Ogg => ".ogg",
            Self::Raw => ".raw",
        }
    }

    /// Get the MIME type for this format
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
            Self::Ogg => "audio/ogg",
            Self::Raw => "audio/raw",
        }
    }

    /// Check if format supports streaming
    pub fn supports_streaming(&self) -> bool {
        matches!(self, Self::Mp3 | Self::Ogg)
    }
}

impl fmt::Display for CereprocAudioFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for CereprocAudioFormat {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav" | "wave" => Ok(Self::Wav),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "ogg" | "vorbis" => Ok(Self::Ogg),
            "raw" | "pcm" => Ok(Self::Raw),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown audio format: {}. Supported: wav, mp3, ogg, raw",
                s
            ))),
        }
    }
}

// =============================================================================
// Emotion Types
// =============================================================================

/// Supported emotion types for CereVoice emotional voices
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CereprocEmotion {
    /// Happy/excited emotion
    Happy,
    /// Sad/melancholy emotion
    Sad,
    /// Calm/peaceful emotion
    Calm,
    /// Cross/angry emotion
    Cross,
}

impl CereprocEmotion {
    /// Get the emotion name for SSML tags
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Happy => "happy",
            Self::Sad => "sad",
            Self::Calm => "calm",
            Self::Cross => "cross",
        }
    }
}

impl fmt::Display for CereprocEmotion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Credentials
// =============================================================================

/// CereVoice Cloud API credentials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CereprocCredentials {
    /// Email address (same as CereProc website login)
    pub email: String,
    /// Password (same as CereProc website login)
    pub password: String,
}

impl CereprocCredentials {
    /// Create credentials from an "email:password" formatted API key
    ///
    /// # Arguments
    /// * `api_key` - String in format "email:password"
    ///
    /// # Returns
    /// * `TTSResult<Self>` - Credentials or error if format is invalid
    pub fn from_api_key(api_key: &str) -> TTSResult<Self> {
        let parts: Vec<&str> = api_key.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(TTSError::InvalidConfiguration(
                "CereProc API key must be in format 'email:password'".to_string(),
            ));
        }

        let email = parts[0].trim();
        let password = parts[1].trim();

        if email.is_empty() || password.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "CereProc credentials cannot be empty".to_string(),
            ));
        }

        // Basic email validation
        if !email.contains('@') || !email.contains('.') {
            return Err(TTSError::InvalidConfiguration(
                "Invalid email format in CereProc credentials".to_string(),
            ));
        }

        Ok(Self {
            email: email.to_string(),
            password: password.to_string(),
        })
    }

    /// Get the Basic auth header value (base64 encoded)
    pub fn basic_auth_value(&self) -> String {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let credentials = format!("{}:{}", self.email, self.password);
        format!("Basic {}", STANDARD.encode(credentials.as_bytes()))
    }
}

// =============================================================================
// Voice Info
// =============================================================================

/// Information about a CereVoice voice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CereprocVoice {
    /// Voice name (e.g., "Stuart")
    pub name: String,
    /// Language code (e.g., "en-SC")
    pub language: String,
    /// Gender (male/female)
    #[serde(default)]
    pub gender: Option<String>,
    /// Whether the voice supports emotions
    #[serde(default)]
    pub has_emotions: bool,
    /// Available emotions for this voice
    #[serde(default)]
    pub emotions: Vec<String>,
}

// =============================================================================
// Credit Info
// =============================================================================

/// Credit balance information
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CereprocCredit {
    /// Free credits remaining
    #[serde(default)]
    pub free_credit: String,
    /// Paid credits remaining
    #[serde(default)]
    pub paid_credit: String,
    /// Total characters available
    #[serde(default)]
    pub chars_available: String,
}

impl CereprocCredit {
    /// Get total available credits as a number
    pub fn total_available(&self) -> u64 {
        self.chars_available.parse().unwrap_or(0)
    }

    /// Check if credits are low (below 1000)
    pub fn is_low(&self) -> bool {
        self.total_available() < 1000
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// CereProc TTS configuration
#[derive(Debug, Clone)]
pub struct CereprocTtsConfig {
    /// API credentials
    pub credentials: CereprocCredentials,
    /// Voice name
    pub voice_id: String,
    /// Audio output format
    pub audio_format: CereprocAudioFormat,
    /// Sample rate in Hz
    pub sample_rate: u32,
    /// Enable 3D audio processing
    pub audio_3d: bool,
    /// Include metadata in response
    pub include_metadata: bool,
    /// Default emotion for the voice (if supported)
    pub emotion: Option<CereprocEmotion>,
    /// Optional REST endpoint base (scheme+host) override for auth/synth POSTs (test redirection).
    pub endpoint_override: Option<String>,
}

impl CereprocTtsConfig {
    /// Create configuration from a base TTSConfig
    ///
    /// # Arguments
    /// * `config` - Base TTS configuration with api_key in "email:password" format
    ///
    /// # Returns
    /// * `TTSResult<Self>` - Cereproc-specific configuration or error
    pub fn from_base(config: &TTSConfig) -> TTSResult<Self> {
        // Parse credentials from api_key
        let credentials = CereprocCredentials::from_api_key(&config.api_key)?;

        // Parse voice ID
        let voice_id = config
            .voice_id
            .clone()
            .unwrap_or_else(|| super::DEFAULT_VOICE_ID.to_string());

        // Parse audio format
        let audio_format = if let Some(ref fmt) = config.audio_format {
            if !fmt.is_empty() {
                fmt.parse().unwrap_or_default()
            } else {
                CereprocAudioFormat::default()
            }
        } else {
            CereprocAudioFormat::default()
        };

        // Parse sample rate
        let sample_rate = match config.sample_rate {
            Some(sr) if sr > 0 => sr.clamp(super::MIN_SAMPLE_RATE, super::MAX_SAMPLE_RATE),
            _ => super::DEFAULT_SAMPLE_RATE,
        };

        // Extract emotion from emotion_config if present
        // CereProc supports: Happy, Sad, Calm, Cross
        let emotion = config.emotion_config.as_ref().and_then(|ec| {
            ec.emotion.as_ref().and_then(|e| {
                // Map standard emotion to CereprocEmotion
                match e.to_string().to_lowercase().as_str() {
                    "happy" | "excited" | "joyful" => Some(CereprocEmotion::Happy),
                    "sad" | "melancholy" | "sorrowful" => Some(CereprocEmotion::Sad),
                    "calm" | "peaceful" | "relaxed" | "neutral" => Some(CereprocEmotion::Calm),
                    "angry" | "cross" | "frustrated" | "annoyed" => Some(CereprocEmotion::Cross),
                    _ => None, // Unsupported emotion maps to None (default voice)
                }
            })
        });

        // These options could be configured via environment variables in future versions
        let audio_3d = false;
        let include_metadata = false;

        Ok(Self {
            credentials,
            voice_id,
            audio_format,
            sample_rate,
            audio_3d,
            include_metadata,
            emotion,
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone). CereProc expresses emotion through
    /// custom SSML `<emotion>` tags, so [`TtsFeatures::emotion`] maps to the `emotion` field (using
    /// the same Happy/Sad/Calm/Cross vocabulary as `from_base`), and [`TtsFeatures::sample_rate`]
    /// overrides the output `sample_rate` (clamped to the supported range). The non-standard
    /// `audio_3d` / `include_metadata` knobs are read from the `extras` passthrough.
    ///
    /// # Capability gaps on the v2 `/speak` synthesis endpoint
    ///
    /// The CereVoice Cloud **v2 REST** `/speak` endpoint this provider targets
    /// (`https://api.cerevoice.com/v2/speak`) is controlled solely by the query parameters
    /// `voice`, `audioFormat`, `sampleRate`, `audio3D`, `metadata` (see
    /// [`build_query_params`]) plus the XML/SSML body. It has **no** `lang`/`language`,
    /// `accent`, or `streaming` synthesis parameter:
    ///
    /// - **`language`** — not a `/speak` parameter. On the v2 REST API the spoken language is
    ///   determined entirely by the chosen `voice`; `language` is a field only on the *voice
    ///   metadata* (`VoiceInfo.language`) and on *lexicon/abbreviation uploads*
    ///   (`LexiconInfo.language`), never on synthesis. Confirmed against the published CereVoice
    ///   Cloud client surface (`SpeakExtendedInput` exposes only voice/text/audioFormat/sampleRate/
    ///   audio3D/metadata). So [`TtsFeatures::language`] is a **capability gap** here and is left
    ///   unwired rather than fabricated onto the wire.
    /// - **`accent`** — not a `/speak` parameter either; the accent is implicit in the `voice`
    ///   (e.g. "Stuart" is Scottish English). `accent` appears only on lexicon uploads
    ///   (`LexiconInfo.accent`). The `extras["accent"]` value is therefore intentionally **not**
    ///   forwarded to `/speak` (capability gap).
    /// - **`streaming`** — the v2 `/speak` endpoint returns a `fileUrl` to a fully-rendered audio
    ///   file (download model), not a chunked/streamed body, so there is no streaming flag to set
    ///   ([`TtsFeatures::streaming`] is a capability gap).
    ///
    /// Other features CereProc has no field for (speed, pitch, volume, stability,
    /// similarity_boost, style, use_speaker_boost, instructions, ssml, word_timestamps, seed) are
    /// likewise skipped.
    ///
    /// [`TtsFeatures::emotion`]: crate::core::tts::standard::TtsFeatures::emotion
    /// [`TtsFeatures::sample_rate`]: crate::core::tts::standard::TtsFeatures::sample_rate
    /// [`TtsFeatures::language`]: crate::core::tts::standard::TtsFeatures::language
    /// [`TtsFeatures::streaming`]: crate::core::tts::standard::TtsFeatures::streaming
    /// [`build_query_params`]: CereprocTtsConfig::build_query_params
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> TTSResult<Self> {
        let f = &std.features;
        let mut cfg = Self::from_base(&std.base)?;

        // Emotion via CereProc's custom <emotion> SSML tags (Happy/Sad/Calm/Cross).
        if let Some(emotion) = f.emotion.as_deref() {
            cfg.emotion = match emotion.to_lowercase().as_str() {
                "happy" | "excited" | "joyful" => Some(CereprocEmotion::Happy),
                "sad" | "melancholy" | "sorrowful" => Some(CereprocEmotion::Sad),
                "calm" | "peaceful" | "relaxed" | "neutral" => Some(CereprocEmotion::Calm),
                "angry" | "cross" | "frustrated" | "annoyed" => Some(CereprocEmotion::Cross),
                _ => cfg.emotion, // Unsupported emotion: keep base mapping (default voice).
            };
        }
        if let Some(rate) = f.sample_rate {
            cfg.sample_rate = rate.clamp(super::MIN_SAMPLE_RATE, super::MAX_SAMPLE_RATE);
        }

        // NOTE (capability gaps): `features.language`, `features.streaming`, and
        // `extras["accent"]` are deliberately NOT mapped — the v2 `/speak` endpoint has no
        // corresponding synthesis parameter (language/accent come from the voice; the endpoint
        // returns a fileUrl, not a stream). See the doc comment above for the full citation. They
        // are left unwired rather than fabricated onto the request.

        // Provider-specific passthrough.
        if let Some(audio_3d) = std.extras.0.get("audio_3d").and_then(|v| v.as_bool()) {
            cfg.audio_3d = audio_3d;
        }
        if let Some(include_metadata) = std
            .extras
            .0
            .get("include_metadata")
            .and_then(|v| v.as_bool())
        {
            cfg.include_metadata = include_metadata;
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);
        cfg.validate_endpoint_override()?;

        Ok(cfg)
    }

    /// Validate the provider-level endpoint override, when present.
    pub fn validate_endpoint_override(&self) -> TTSResult<()> {
        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_cereproc_tts_endpoint("endpoint_override", endpoint)
                .map_err(TTSError::InvalidConfiguration)?;
        }
        Ok(())
    }

    /// Build the query parameters for the speak endpoint
    pub fn build_query_params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![("voice", self.voice_id.clone())];

        // Add format if not default
        if self.audio_format != CereprocAudioFormat::Wav {
            params.push(("audioFormat", self.audio_format.to_string()));
        }

        // Add sample rate if not default
        if self.sample_rate != super::DEFAULT_SAMPLE_RATE {
            params.push(("sampleRate", self.sample_rate.to_string()));
        }

        // Add 3D audio if enabled
        if self.audio_3d {
            params.push(("audio3D", "true".to_string()));
        }

        // Add metadata if enabled
        if self.include_metadata {
            params.push(("metadata", "true".to_string()));
        }

        params
    }

    /// Validate text length
    ///
    /// # Arguments
    /// * `text` - Text to validate
    ///
    /// # Returns
    /// * `TTSResult<()>` - Ok if valid, error if too long
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

    /// Wrap text with emotion tags if emotion is set
    ///
    /// # Arguments
    /// * `text` - Original text
    ///
    /// # Returns
    /// * String with emotion tags if emotion is configured
    pub fn wrap_with_emotion(&self, text: &str) -> String {
        if let Some(emotion) = &self.emotion {
            format!(
                "<doc><emotion name=\"{}\">{}</emotion></doc>",
                emotion, text
            )
        } else {
            format!("<doc>{}</doc>", text)
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone (TTS): the standardized features CereProc can express (emotion via its custom
    // <emotion> SSML tags, output sample rate) reach their real fields, and the non-standard
    // audio_3d / include_metadata knobs flow through the extras passthrough.
    #[test]
    fn from_standard_maps_emotion_sample_rate_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("audio_3d".into(), serde_json::json!(true));
        extras.insert("include_metadata".into(), serde_json::json!(true));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "cereproc".into(),
                api_key: "user@example.com:password123".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                emotion: Some("cheerful".into()), // unsupported -> stays None; see "happy" below
                sample_rate: Some(16000),
                ssml: Some(true), // capability gap: CereProc has no ssml flag field, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = CereprocTtsConfig::from_standard(&std).unwrap();
        assert!(cfg.emotion.is_none()); // "cheerful" is not in CereProc's vocabulary
        assert_eq!(cfg.sample_rate, 16000);
        assert!(cfg.audio_3d); // extras passthrough
        assert!(cfg.include_metadata); // extras passthrough

        // A supported emotion maps to the matching CereprocEmotion variant.
        let std = StandardTTSConfig {
            base: TTSConfig {
                api_key: "user@example.com:password123".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                emotion: Some("happy".into()),
                ..Default::default()
            },
            extras: Default::default(),
        };
        let cfg = CereprocTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.emotion, Some(CereprocEmotion::Happy));
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = CereprocTtsConfig::from_base(&TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            ..Default::default()
        })
        .unwrap();

        config.endpoint_override = Some("https://cereproc-proxy.example.com".to_string());
        assert!(config.validate_endpoint_override().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(format!("{err:?}").contains("SSRF protection"));

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("file endpoint_override must be rejected");
        assert!(format!("{err:?}").contains("URL scheme"));

        config.endpoint_override = Some("ws://cereproc-proxy.example.com".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("WebSocket endpoint_override must be rejected");
        assert!(format!("{err:?}").contains("URL scheme"));

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "cereproc".to_string(),
            api_key: "user@example.com:password123".to_string(),
            ..Default::default()
        })
        .with_endpoint_override("file:///tmp/socket");
        assert!(CereprocTtsConfig::from_standard(&std).is_err());
    }

    #[test]
    fn test_audio_format_conversion() {
        assert_eq!(CereprocAudioFormat::Wav.as_str(), "wav");
        assert_eq!(CereprocAudioFormat::Mp3.as_str(), "mp3");
        assert_eq!(CereprocAudioFormat::Ogg.as_str(), "ogg");
        assert_eq!(CereprocAudioFormat::Raw.as_str(), "raw");
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            "wav".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Wav
        );
        assert_eq!(
            "mp3".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Mp3
        );
        assert_eq!(
            "ogg".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Ogg
        );
        assert_eq!(
            "raw".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Raw
        );
    }

    #[test]
    fn test_audio_format_aliases() {
        assert_eq!(
            "wave".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Wav
        );
        assert_eq!(
            "mpeg".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Mp3
        );
        assert_eq!(
            "vorbis".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Ogg
        );
        assert_eq!(
            "pcm".parse::<CereprocAudioFormat>().unwrap(),
            CereprocAudioFormat::Raw
        );
    }

    #[test]
    fn test_audio_format_invalid() {
        assert!("flac".parse::<CereprocAudioFormat>().is_err());
    }

    #[test]
    fn test_audio_format_mime_types() {
        assert_eq!(CereprocAudioFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(CereprocAudioFormat::Mp3.mime_type(), "audio/mpeg");
        assert_eq!(CereprocAudioFormat::Ogg.mime_type(), "audio/ogg");
        assert_eq!(CereprocAudioFormat::Raw.mime_type(), "audio/raw");
    }

    #[test]
    fn test_credentials_from_api_key() {
        let creds = CereprocCredentials::from_api_key("user@example.com:password123").unwrap();
        assert_eq!(creds.email, "user@example.com");
        assert_eq!(creds.password, "password123");
    }

    #[test]
    fn test_credentials_invalid_format() {
        assert!(CereprocCredentials::from_api_key("invalid").is_err());
        assert!(CereprocCredentials::from_api_key("").is_err());
        assert!(CereprocCredentials::from_api_key(":password").is_err());
        assert!(CereprocCredentials::from_api_key("user@example.com:").is_err());
    }

    #[test]
    fn test_credentials_invalid_email() {
        assert!(CereprocCredentials::from_api_key("notanemail:password").is_err());
    }

    #[test]
    fn test_credentials_basic_auth() {
        let creds = CereprocCredentials::from_api_key("user@test.com:pass").unwrap();
        let auth = creds.basic_auth_value();
        assert!(auth.starts_with("Basic "));
        // Base64 of "user@test.com:pass"
        assert!(auth.contains("dXNlckB0ZXN0LmNvbTpwYXNz"));
    }

    #[test]
    fn test_emotion_as_str() {
        assert_eq!(CereprocEmotion::Happy.as_str(), "happy");
        assert_eq!(CereprocEmotion::Sad.as_str(), "sad");
        assert_eq!(CereprocEmotion::Calm.as_str(), "calm");
        assert_eq!(CereprocEmotion::Cross.as_str(), "cross");
    }

    #[test]
    fn test_credit_total() {
        let credit = CereprocCredit {
            free_credit: "5000".to_string(),
            paid_credit: "10000".to_string(),
            chars_available: "15000".to_string(),
        };
        assert_eq!(credit.total_available(), 15000);
        assert!(!credit.is_low());
    }

    #[test]
    fn test_credit_low() {
        let credit = CereprocCredit {
            free_credit: "500".to_string(),
            paid_credit: "0".to_string(),
            chars_available: "500".to_string(),
        };
        assert!(credit.is_low());
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            voice_id: Some("Katherine".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        };

        let config = CereprocTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.credentials.email, "user@example.com");
        assert_eq!(config.credentials.password, "password123");
        assert_eq!(config.voice_id, "Katherine");
        assert_eq!(config.audio_format, CereprocAudioFormat::Mp3);
        assert_eq!(config.sample_rate, 16000);
    }

    #[test]
    fn test_config_defaults() {
        let base = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            voice_id: None,     // Explicitly None to test Cereproc default
            audio_format: None, // Explicitly None to test Cereproc default
            sample_rate: None,  // Explicitly None to test Cereproc default
            ..Default::default()
        };

        let config = CereprocTtsConfig::from_base(&base).unwrap();
        assert_eq!(config.voice_id, "Stuart");
        assert_eq!(config.audio_format, CereprocAudioFormat::Wav);
        assert_eq!(config.sample_rate, 22050);
        assert!(!config.audio_3d);
        assert!(!config.include_metadata);
        assert!(config.emotion.is_none());
    }

    #[test]
    fn test_build_query_params() {
        let base = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            voice_id: Some("Jess".to_string()),
            audio_format: Some("ogg".to_string()),
            sample_rate: Some(16000),
            ..Default::default()
        };

        let config = CereprocTtsConfig::from_base(&base).unwrap();
        let params = config.build_query_params();

        assert!(params.iter().any(|(k, v)| *k == "voice" && v == "Jess"));
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "audioFormat" && v == "ogg")
        );
        assert!(
            params
                .iter()
                .any(|(k, v)| *k == "sampleRate" && v == "16000")
        );
    }

    #[test]
    fn test_validate_text() {
        let base = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            ..Default::default()
        };
        let config = CereprocTtsConfig::from_base(&base).unwrap();

        // Valid text
        assert!(config.validate_text("Hello world").is_ok());

        // Too long text (MAX_TEXT_LENGTH is 5000 characters)
        let long_text = "x".repeat(crate::core::tts::cereproc::MAX_TEXT_LENGTH + 1);
        assert!(config.validate_text(&long_text).is_err());
    }

    #[test]
    fn test_wrap_with_emotion() {
        let base = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            ..Default::default()
        };

        // Without emotion
        let config = CereprocTtsConfig::from_base(&base).unwrap();
        let wrapped = config.wrap_with_emotion("Hello");
        assert_eq!(wrapped, "<doc>Hello</doc>");
    }

    #[test]
    fn test_wrap_with_emotion_happy() {
        let base = TTSConfig {
            api_key: "user@example.com:password123".to_string(),
            ..Default::default()
        };

        let mut config = CereprocTtsConfig::from_base(&base).unwrap();
        // Set emotion directly for testing
        config.emotion = Some(CereprocEmotion::Happy);

        let wrapped = config.wrap_with_emotion("Hello");
        assert_eq!(
            wrapped,
            "<doc><emotion name=\"happy\">Hello</emotion></doc>"
        );
    }

    #[test]
    fn test_voice_deserialization() {
        let json = r#"{
            "name": "Stuart",
            "language": "en-SC",
            "gender": "male",
            "has_emotions": true,
            "emotions": ["happy", "sad", "calm", "cross"]
        }"#;

        let voice: CereprocVoice = serde_json::from_str(json).unwrap();
        assert_eq!(voice.name, "Stuart");
        assert_eq!(voice.language, "en-SC");
        assert_eq!(voice.gender, Some("male".to_string()));
        assert!(voice.has_emotions);
        assert_eq!(voice.emotions.len(), 4);
    }
}
