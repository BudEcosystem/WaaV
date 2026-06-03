//! Configuration types for NAVER CLOVA Voice (TTS Premium) API.
//!
//! This module provides configuration types for NAVER Cloud Platform's
//! CLOVA Voice service, which converts text to speech.
//!
//! # Overview
//!
//! CLOVA Voice is NAVER's premium TTS service offering:
//! - 100+ high-quality voices with NeuVis technology
//! - Multiple languages (Korean, Japanese, English, Chinese, Spanish)
//! - Adjustable parameters (volume, speed, pitch, emotion)
//! - MP3 and WAV output formats
//!
//! # Authentication
//!
//! The API uses a two-key authentication system:
//! - `X-NCP-APIGW-API-KEY-ID`: Client ID
//! - `X-NCP-APIGW-API-KEY`: Client Secret

use serde::{Deserialize, Serialize};

use crate::core::tts::base::{TTSConfig, TTSError};

// =============================================================================
// Constants
// =============================================================================

/// CLOVA Voice Premium TTS API endpoint.
pub const NAVER_TTS_ENDPOINT: &str = "https://naveropenapi.apigw.ntruss.com/tts-premium/v1/tts";

/// Maximum text length per request (2000 characters).
pub const MAX_TEXT_LENGTH: usize = 2000;

/// Default sample rate for output audio.
pub const DEFAULT_SAMPLE_RATE: u32 = 24000;

// =============================================================================
// Voice Types
// =============================================================================

/// CLOVA Voice speaker IDs.
///
/// NAVER CLOVA Voice offers 100+ speakers across multiple languages.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NaverClovaVoice {
    // Korean Voices
    /// Nara - Female, Standard, Clear, Professional
    #[default]
    Nara,
    /// Nara Call - Female, Call center style
    NaraCall,
    /// Nminsang - Male, Standard, Professional
    Nminsang,
    /// Nhajun - Male, Child voice
    Nhajun,
    /// Ndain - Female, Child voice
    Ndain,
    /// Njiyun - Female, Standard, Natural
    Njiyun,
    /// Nsujin - Female, Standard, Friendly
    Nsujin,
    /// Njinho - Male, Standard, Professional
    Njinho,
    /// Nminseo - Female, Standard, Bright
    Nminseo,
    /// Njooahn - Female, Standard, Calm
    Njooahn,
    /// Nseonghoon - Male, Standard, Deep
    Nseonghoon,
    /// Njihun - Male, Standard, Warm
    Njihun,

    // Japanese Voices
    /// Ntomoko - Female, Japanese, Standard
    Ntomoko,
    /// Nnaomi - Female, Japanese, Standard
    Nnaomi,
    /// Nsayuri - Female, Japanese, Bright
    Nsayuri,

    // English Voices
    /// Clara - Female, English, Standard
    Clara,
    /// Matt - Male, English, Standard
    Matt,
    /// Danna - Female, English, Premium
    Danna,
    /// Djoey - Male, English, Premium
    Djoey,

    // Chinese Voices
    /// Meimei - Female, Chinese, Standard
    Meimei,
    /// Liangliang - Male, Chinese, Standard
    Liangliang,

    // Spanish Voices
    /// Jose - Male, Spanish, Standard
    Jose,
    /// Carmen - Female, Spanish, Standard
    Carmen,

    /// Custom voice ID
    Custom(String),
}

impl NaverClovaVoice {
    /// Get the speaker ID for the API.
    pub fn speaker_id(&self) -> &str {
        match self {
            Self::Nara => "nara",
            Self::NaraCall => "nara_call",
            Self::Nminsang => "nminsang",
            Self::Nhajun => "nhajun",
            Self::Ndain => "ndain",
            Self::Njiyun => "njiyun",
            Self::Nsujin => "nsujin",
            Self::Njinho => "njinho",
            Self::Nminseo => "nminseo",
            Self::Njooahn => "njooahn",
            Self::Nseonghoon => "nseonghoon",
            Self::Njihun => "njihun",
            Self::Ntomoko => "ntomoko",
            Self::Nnaomi => "nnaomi",
            Self::Nsayuri => "nsayuri",
            Self::Clara => "clara",
            Self::Matt => "matt",
            Self::Danna => "danna",
            Self::Djoey => "djoey",
            Self::Meimei => "meimei",
            Self::Liangliang => "liangliang",
            Self::Jose => "jose",
            Self::Carmen => "carmen",
            Self::Custom(id) => id,
        }
    }

    /// Get the display name for the voice.
    pub fn display_name(&self) -> &str {
        match self {
            Self::Nara => "Nara (Korean Female)",
            Self::NaraCall => "Nara Call Center (Korean Female)",
            Self::Nminsang => "Minsang (Korean Male)",
            Self::Nhajun => "Hajun (Korean Male Child)",
            Self::Ndain => "Dain (Korean Female Child)",
            Self::Njiyun => "Jiyun (Korean Female)",
            Self::Nsujin => "Sujin (Korean Female)",
            Self::Njinho => "Jinho (Korean Male)",
            Self::Nminseo => "Minseo (Korean Female)",
            Self::Njooahn => "Jooahn (Korean Female)",
            Self::Nseonghoon => "Seonghoon (Korean Male)",
            Self::Njihun => "Jihun (Korean Male)",
            Self::Ntomoko => "Tomoko (Japanese Female)",
            Self::Nnaomi => "Naomi (Japanese Female)",
            Self::Nsayuri => "Sayuri (Japanese Female)",
            Self::Clara => "Clara (English Female)",
            Self::Matt => "Matt (English Male)",
            Self::Danna => "Danna (English Female Premium)",
            Self::Djoey => "Joey (English Male Premium)",
            Self::Meimei => "Meimei (Chinese Female)",
            Self::Liangliang => "Liangliang (Chinese Male)",
            Self::Jose => "Jose (Spanish Male)",
            Self::Carmen => "Carmen (Spanish Female)",
            Self::Custom(id) => id,
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "nara" => Self::Nara,
            "nara_call" => Self::NaraCall,
            "nminsang" => Self::Nminsang,
            "nhajun" => Self::Nhajun,
            "ndain" => Self::Ndain,
            "njiyun" => Self::Njiyun,
            "nsujin" => Self::Nsujin,
            "njinho" => Self::Njinho,
            "nminseo" => Self::Nminseo,
            "njooahn" => Self::Njooahn,
            "nseonghoon" => Self::Nseonghoon,
            "njihun" => Self::Njihun,
            "ntomoko" => Self::Ntomoko,
            "nnaomi" => Self::Nnaomi,
            "nsayuri" => Self::Nsayuri,
            "clara" => Self::Clara,
            "matt" => Self::Matt,
            "danna" => Self::Danna,
            "djoey" => Self::Djoey,
            "meimei" => Self::Meimei,
            "liangliang" => Self::Liangliang,
            "jose" => Self::Jose,
            "carmen" => Self::Carmen,
            _ => Self::Custom(s.to_string()),
        }
    }

    /// Get all built-in voices.
    pub fn all() -> Vec<Self> {
        vec![
            Self::Nara,
            Self::NaraCall,
            Self::Nminsang,
            Self::Nhajun,
            Self::Ndain,
            Self::Njiyun,
            Self::Nsujin,
            Self::Njinho,
            Self::Nminseo,
            Self::Njooahn,
            Self::Nseonghoon,
            Self::Njihun,
            Self::Ntomoko,
            Self::Nnaomi,
            Self::Nsayuri,
            Self::Clara,
            Self::Matt,
            Self::Danna,
            Self::Djoey,
            Self::Meimei,
            Self::Liangliang,
            Self::Jose,
            Self::Carmen,
        ]
    }
}

impl std::fmt::Display for NaverClovaVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.speaker_id())
    }
}

// =============================================================================
// Audio Format
// =============================================================================

/// Output audio format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum NaverClovaTtsFormat {
    /// MP3 format (default, smaller file size)
    #[default]
    Mp3,
    /// WAV format (higher quality)
    Wav,
}

impl NaverClovaTtsFormat {
    /// Get the API format parameter.
    pub fn api_format(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
        }
    }

    /// Get the content type.
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
        }
    }

    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "mp3" | "mpeg" => Some(Self::Mp3),
            "wav" | "wave" | "pcm" | "linear16" => Some(Self::Wav),
            _ => None,
        }
    }
}

impl std::fmt::Display for NaverClovaTtsFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.api_format())
    }
}

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for NAVER CLOVA Voice TTS.
#[derive(Debug, Clone)]
pub struct NaverClovaTtsConfig {
    /// Client ID from NAVER Cloud Platform.
    pub client_id: String,

    /// Client Secret from NAVER Cloud Platform.
    pub client_secret: String,

    /// Voice to use.
    pub voice: NaverClovaVoice,

    /// Volume adjustment (-5 to 5, default 0).
    pub volume: i8,

    /// Speed adjustment (-5 to 5, default 0).
    pub speed: i8,

    /// Pitch adjustment (-5 to 5, default 0).
    pub pitch: i8,

    /// Emotion intensity (0-2, default 0).
    pub emotion: u8,

    /// Emotion strength (0-2, default 1).
    pub emotion_strength: u8,

    /// Output audio format.
    pub format: NaverClovaTtsFormat,

    /// Output sampling rate in Hz (8000, 16000, 24000, 48000). CLOVA Voice Premium honors the
    /// `sampling-rate` parameter for WAV output only (MP3 is fixed); `None` keeps the API default.
    pub sampling_rate: Option<u32>,

    /// Alpha (timbre) adjustment (-5 to 5, default 0).
    pub alpha: i8,

    /// End pitch adjustment (-5 to 5, default 0).
    pub end_pitch: i8,

    /// Request timeout in seconds.
    pub request_timeout_secs: u64,

    /// Custom endpoint URL (for enterprise deployments).
    pub custom_endpoint: Option<String>,
}

impl Default for NaverClovaTtsConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            voice: NaverClovaVoice::default(),
            volume: 0,
            speed: 0,
            pitch: 0,
            emotion: 0,
            emotion_strength: 1,
            format: NaverClovaTtsFormat::default(),
            sampling_rate: None,
            alpha: 0,
            end_pitch: 0,
            request_timeout_secs: 30,
            custom_endpoint: None,
        }
    }
}

impl NaverClovaTtsConfig {
    /// Create configuration from base TTSConfig.
    ///
    /// API key format: `client_id|client_secret`
    pub fn from_base(config: TTSConfig) -> Result<Self, TTSError> {
        if config.api_key.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "NAVER CLOVA API key is required (format: client_id|client_secret)".to_string(),
            ));
        }

        // Parse client_id|client_secret format
        let parts: Vec<&str> = config.api_key.split('|').collect();
        if parts.len() != 2 {
            return Err(TTSError::InvalidConfiguration(
                "NAVER CLOVA API key must be in format: client_id|client_secret".to_string(),
            ));
        }

        let client_id = parts[0].to_string();
        let client_secret = parts[1].to_string();

        if client_id.is_empty() || client_secret.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Both client_id and client_secret must be non-empty".to_string(),
            ));
        }

        // Parse voice
        let voice = if let Some(ref voice_id) = config.voice_id {
            NaverClovaVoice::from_str(voice_id)
        } else {
            NaverClovaVoice::default()
        };

        // Parse speed from speaking_rate (0.25-4.0 -> -5 to 5)
        let speed = if let Some(rate) = config.speaking_rate {
            // Map 0.25-4.0 to -5 to 5
            // 1.0 = 0, 0.25 = -5, 4.0 = 5
            let clamped = rate.clamp(0.25, 4.0);
            if clamped < 1.0 {
                // 0.25-1.0 maps to -5 to 0
                ((clamped - 0.25) / 0.75 * 5.0 - 5.0) as i8
            } else {
                // 1.0-4.0 maps to 0 to 5
                ((clamped - 1.0) / 3.0 * 5.0) as i8
            }
        } else {
            0
        };

        // Parse format
        let format = if let Some(ref audio_format) = config.audio_format {
            NaverClovaTtsFormat::from_str(audio_format).unwrap_or_default()
        } else {
            NaverClovaTtsFormat::default()
        };

        // Parse timeout
        let request_timeout_secs = config.request_timeout.unwrap_or(30);

        Ok(Self {
            client_id,
            client_secret,
            voice,
            volume: 0,
            speed,
            pitch: 0,
            emotion: 0,
            emotion_strength: 1,
            format,
            sampling_rate: None,
            alpha: 0,
            end_pitch: 0,
            request_timeout_secs,
            custom_endpoint: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone).
    ///
    /// CLOVA Voice exposes prosody as integer deltas in -5..=5, so this maps the matching
    /// standardized features onto those fields:
    /// - [`TtsFeatures::speed`] -> `speed` (a 0.25..4.0 multiplier folded onto -5..=5, same
    ///   curve as [`from_base`])
    /// - [`TtsFeatures::pitch`] -> `pitch` (clamped to -5..=5)
    /// - [`TtsFeatures::volume`] -> `volume` (clamped to -5..=5)
    ///
    /// - [`TtsFeatures::sample_rate`] -> `sampling_rate` (CLOVA's `sampling-rate` form param; the
    ///   API honors it for WAV output only, so it is emitted on the wire only when `format == wav`)
    ///
    /// CLOVA's `custom_endpoint` (not a standard field) is read from the `extras` passthrough.
    /// Features without a CLOVA field — `emotion` (CLOVA's emotion is a numeric intensity, not a
    /// free-text label), `ssml`, `stability`, `similarity_boost`, `style`,
    /// `use_speaker_boost`, `instructions`, `language`, `word_timestamps`, `streaming`, `seed` —
    /// are skipped.
    ///
    /// [`TtsFeatures::speed`]: crate::core::tts::standard::TtsFeatures::speed
    /// [`TtsFeatures::pitch`]: crate::core::tts::standard::TtsFeatures::pitch
    /// [`TtsFeatures::volume`]: crate::core::tts::standard::TtsFeatures::volume
    /// [`from_base`]: Self::from_base
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> Result<Self, TTSError> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(rate) = f.speed {
            // Same 0.25..4.0 multiplier -> -5..=5 curve used by `from_base` (1.0 = 0).
            let clamped = rate.clamp(0.25, 4.0);
            cfg.speed = if clamped < 1.0 {
                ((clamped - 0.25) / 0.75 * 5.0 - 5.0) as i8
            } else {
                ((clamped - 1.0) / 3.0 * 5.0) as i8
            };
        }
        if let Some(pitch) = f.pitch {
            cfg.pitch = (pitch as i32).clamp(-5, 5) as i8;
        }
        if let Some(volume) = f.volume {
            cfg.volume = (volume as i32).clamp(-5, 5) as i8;
        }
        if let Some(sample_rate) = f.sample_rate {
            // Reaches the wire as `sampling-rate` (WAV-only) in `build_request_body`.
            cfg.sampling_rate = Some(sample_rate);
        }

        // Provider-specific passthrough.
        if let Some(endpoint) = std.extras.0.get("custom_endpoint").and_then(|v| v.as_str()) {
            cfg.custom_endpoint = Some(endpoint.to_string());
        }

        Ok(cfg)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), TTSError> {
        if self.client_id.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Client ID is required".to_string(),
            ));
        }

        if self.client_secret.is_empty() {
            return Err(TTSError::InvalidConfiguration(
                "Client Secret is required".to_string(),
            ));
        }

        // Validate ranges
        if self.volume < -5 || self.volume > 5 {
            return Err(TTSError::InvalidConfiguration(
                "Volume must be between -5 and 5".to_string(),
            ));
        }

        if self.speed < -5 || self.speed > 5 {
            return Err(TTSError::InvalidConfiguration(
                "Speed must be between -5 and 5".to_string(),
            ));
        }

        if self.pitch < -5 || self.pitch > 5 {
            return Err(TTSError::InvalidConfiguration(
                "Pitch must be between -5 and 5".to_string(),
            ));
        }

        if self.emotion > 2 {
            return Err(TTSError::InvalidConfiguration(
                "Emotion must be between 0 and 2".to_string(),
            ));
        }

        Ok(())
    }

    /// Get the API endpoint URL.
    #[inline]
    pub fn get_endpoint_url(&self) -> &str {
        if let Some(ref custom) = self.custom_endpoint {
            custom
        } else {
            NAVER_TTS_ENDPOINT
        }
    }

    /// Build the form-encoded request body.
    pub fn build_request_body(&self, text: &str) -> String {
        // URL-encode the text
        let text_to_encode = &text[..text.len().min(MAX_TEXT_LENGTH)];
        let encoded_text: String =
            url::form_urlencoded::byte_serialize(text_to_encode.as_bytes()).collect();

        let mut params = vec![
            format!("speaker={}", self.voice.speaker_id()),
            format!("text={}", encoded_text),
            format!("volume={}", self.volume),
            format!("speed={}", self.speed),
            format!("pitch={}", self.pitch),
            format!("format={}", self.format.api_format()),
        ];

        // Optional parameters
        if self.emotion > 0 {
            params.push(format!("emotion={}", self.emotion));
            params.push(format!("emotion-strength={}", self.emotion_strength));
        }

        // CLOVA Voice Premium honors `sampling-rate` for WAV output only (MP3 is fixed by the
        // codec), so emit it only when the requested format is WAV — otherwise the API rejects it.
        if let Some(sampling_rate) = self.sampling_rate
            && self.format == NaverClovaTtsFormat::Wav
        {
            params.push(format!("sampling-rate={}", sampling_rate));
        }

        if self.alpha != 0 {
            params.push(format!("alpha={}", self.alpha));
        }

        if self.end_pitch != 0 {
            params.push(format!("end-pitch={}", self.end_pitch));
        }

        params.join("&")
    }

    /// Get list of supported voices.
    pub fn supported_voices() -> Vec<(String, String)> {
        NaverClovaVoice::all()
            .into_iter()
            .map(|v| (v.speaker_id().to_string(), v.display_name().to_string()))
            .collect()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features reach CLOVA's integer prosody deltas
    // (speed/pitch/volume in -5..=5), plus custom_endpoint via the open extras passthrough.
    #[test]
    fn from_standard_maps_prosody_and_extras() {
        use crate::core::tts::standard::{ProviderExtras, StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert(
            "custom_endpoint".into(),
            serde_json::json!("https://enterprise.example.com/tts"),
        );
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "naver-clova".into(),
                api_key: "client_id|client_secret".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                speed: Some(4.0),  // max multiplier -> +5
                pitch: Some(3.0),
                volume: Some(-2.0),
                ssml: Some(true), // capability gap: CLOVA has no SSML, must be ignored
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = NaverClovaTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.speed, 5); // 4.0x -> +5
        assert_eq!(cfg.pitch, 3);
        assert_eq!(cfg.volume, -2);
        assert_eq!(
            cfg.custom_endpoint,
            Some("https://enterprise.example.com/tts".to_string())
        ); // extras passthrough
    }

    #[test]
    fn test_voice_speaker_id() {
        assert_eq!(NaverClovaVoice::Nara.speaker_id(), "nara");
        assert_eq!(NaverClovaVoice::Clara.speaker_id(), "clara");
        assert_eq!(NaverClovaVoice::Ntomoko.speaker_id(), "ntomoko");
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(NaverClovaVoice::from_str("nara"), NaverClovaVoice::Nara);
        assert_eq!(NaverClovaVoice::from_str("CLARA"), NaverClovaVoice::Clara);
        assert_eq!(
            NaverClovaVoice::from_str("custom_voice"),
            NaverClovaVoice::Custom("custom_voice".to_string())
        );
    }

    #[test]
    fn test_format_api_format() {
        assert_eq!(NaverClovaTtsFormat::Mp3.api_format(), "mp3");
        assert_eq!(NaverClovaTtsFormat::Wav.api_format(), "wav");
    }

    #[test]
    fn test_format_from_str() {
        assert_eq!(
            NaverClovaTtsFormat::from_str("mp3"),
            Some(NaverClovaTtsFormat::Mp3)
        );
        assert_eq!(
            NaverClovaTtsFormat::from_str("wav"),
            Some(NaverClovaTtsFormat::Wav)
        );
        assert_eq!(
            NaverClovaTtsFormat::from_str("linear16"),
            Some(NaverClovaTtsFormat::Wav)
        );
        assert_eq!(NaverClovaTtsFormat::from_str("invalid"), None);
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "client_id|client_secret".to_string(),
            voice_id: Some("clara".to_string()),
            speaking_rate: Some(1.0),
            audio_format: Some("mp3".to_string()),
            ..Default::default()
        };

        let config = NaverClovaTtsConfig::from_base(base).unwrap();
        assert_eq!(config.client_id, "client_id");
        assert_eq!(config.client_secret, "client_secret");
        assert_eq!(config.voice, NaverClovaVoice::Clara);
        assert_eq!(config.speed, 0);
        assert_eq!(config.format, NaverClovaTtsFormat::Mp3);
    }

    #[test]
    fn test_config_from_base_invalid() {
        let base = TTSConfig {
            api_key: "invalid_format".to_string(),
            ..Default::default()
        };

        let result = NaverClovaTtsConfig::from_base(base);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_validation_valid() {
        let config = NaverClovaTtsConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            ..Default::default()
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_volume() {
        let config = NaverClovaTtsConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            volume: 10,
            ..Default::default()
        };

        assert!(config.validate().is_err());
    }

    #[test]
    fn test_build_request_body() {
        let config = NaverClovaTtsConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            voice: NaverClovaVoice::Nara,
            volume: 1,
            speed: 2,
            pitch: -1,
            format: NaverClovaTtsFormat::Mp3,
            ..Default::default()
        };

        let body = config.build_request_body("Hello world");
        assert!(body.contains("speaker=nara"));
        // form_urlencoded uses + for spaces (application/x-www-form-urlencoded standard)
        assert!(body.contains("text=Hello+world") || body.contains("text=Hello%20world"));
        assert!(body.contains("volume=1"));
        assert!(body.contains("speed=2"));
        assert!(body.contains("pitch=-1"));
        assert!(body.contains("format=mp3"));
    }

    #[test]
    fn test_build_request_body_with_emotion() {
        let config = NaverClovaTtsConfig {
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
            emotion: 1,
            emotion_strength: 2,
            ..Default::default()
        };

        let body = config.build_request_body("Test");
        assert!(body.contains("emotion=1"));
        assert!(body.contains("emotion-strength=2"));
    }

    #[test]
    fn test_supported_voices() {
        let voices = NaverClovaTtsConfig::supported_voices();
        assert!(voices.len() >= 20);
        assert!(voices.iter().any(|(id, _)| id == "nara"));
        assert!(voices.iter().any(|(id, _)| id == "clara"));
    }

    #[test]
    fn test_default_config() {
        let config = NaverClovaTtsConfig::default();
        assert!(config.client_id.is_empty());
        assert!(config.client_secret.is_empty());
        assert_eq!(config.voice, NaverClovaVoice::Nara);
        assert_eq!(config.volume, 0);
        assert_eq!(config.speed, 0);
        assert_eq!(config.pitch, 0);
        assert_eq!(config.format, NaverClovaTtsFormat::Mp3);
    }

    #[test]
    fn test_constants() {
        assert_eq!(
            NAVER_TTS_ENDPOINT,
            "https://naveropenapi.apigw.ntruss.com/tts-premium/v1/tts"
        );
        assert_eq!(MAX_TEXT_LENGTH, 2000);
    }
}
