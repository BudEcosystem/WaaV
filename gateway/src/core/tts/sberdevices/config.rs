//! SberDevices SaluteSpeech TTS Configuration
//!
//! Configuration types for SberDevices SaluteSpeech Text-to-Speech API.
//! Supports OAuth 2.0 authentication with automatic token refresh.

use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

// =============================================================================
// Constants
// =============================================================================

/// TTS synthesis endpoint
pub const SBER_TTS_SYNTHESIZE_URL: &str = "https://smartspeech.sber.ru/rest/v1/text:synthesize";

/// OAuth 2.0 endpoint (shared with STT)
pub const OAUTH_ENDPOINT: &str = "https://ngw.devices.sberbank.ru:9443/api/v2/oauth";

/// Token validity duration in seconds (30 minutes)
pub const TOKEN_VALIDITY_SECS: u64 = 1800;

/// Token refresh threshold in seconds
pub const TOKEN_REFRESH_THRESHOLD_SECS: u64 = 60;

/// Maximum text length (including SSML markup)
pub const MAX_TEXT_LENGTH: usize = 4000;

/// Default sample rate in Hz
pub const DEFAULT_SAMPLE_RATE: u32 = 24000;

// =============================================================================
// Voice Configuration
// =============================================================================

/// Available voices for SberDevices TTS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SberTtsVoice {
    /// Natalia - Russian female voice (default)
    #[default]
    Nec,
    /// Boris - Russian male voice
    Bys,
    /// Martha - Russian female voice
    May,
    /// Taras - Russian male voice
    Tur,
    /// Alexandra - Russian female voice
    Ost,
    /// Sergey - Russian male voice
    Pon,
    /// Kira - English female voice
    Kin,
}

impl SberTtsVoice {
    /// Get the voice ID for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nec => "Nec",
            Self::Bys => "Bys",
            Self::May => "May",
            Self::Tur => "Tur",
            Self::Ost => "Ost",
            Self::Pon => "Pon",
            Self::Kin => "Kin",
        }
    }

    /// Get the full voice ID with sample rate for API
    pub fn voice_id(&self, sample_rate: u32) -> String {
        format!("{}_{}", self.as_str(), sample_rate)
    }

    /// Get display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Nec => "Natalia",
            Self::Bys => "Boris",
            Self::May => "Martha",
            Self::Tur => "Taras",
            Self::Ost => "Alexandra",
            Self::Pon => "Sergey",
            Self::Kin => "Kira",
        }
    }

    /// Get gender
    pub fn gender(&self) -> &'static str {
        match self {
            Self::Nec | Self::May | Self::Ost | Self::Kin => "female",
            Self::Bys | Self::Tur | Self::Pon => "male",
        }
    }

    /// Get language code
    pub fn language(&self) -> &'static str {
        match self {
            Self::Kin => "en-US",
            _ => "ru-RU",
        }
    }

    /// Get all available voices
    pub fn all() -> &'static [SberTtsVoice] {
        &[
            Self::Nec,
            Self::Bys,
            Self::May,
            Self::Tur,
            Self::Ost,
            Self::Pon,
            Self::Kin,
        ]
    }

    /// Get voices by language
    pub fn voices_for_language(language: &str) -> Vec<SberTtsVoice> {
        Self::all()
            .iter()
            .filter(|v| v.language() == language)
            .copied()
            .collect()
    }
}

impl FromStr for SberTtsVoice {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Remove sample rate suffix if present (e.g., "Nec_24000" -> "Nec")
        let voice_id = s.split('_').next().unwrap_or(s);

        match voice_id.to_lowercase().as_str() {
            "nec" | "natalia" => Ok(Self::Nec),
            "bys" | "boris" => Ok(Self::Bys),
            "may" | "martha" => Ok(Self::May),
            "tur" | "taras" => Ok(Self::Tur),
            "ost" | "alexandra" => Ok(Self::Ost),
            "pon" | "sergey" => Ok(Self::Pon),
            "kin" | "kira" => Ok(Self::Kin),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown SberDevices voice: '{}'. Available: Nec, Bys, May, Tur, Ost, Pon, Kin",
                s
            ))),
        }
    }
}

// =============================================================================
// Audio Format Configuration
// =============================================================================

/// Audio output formats for SberDevices TTS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SberTtsAudioFormat {
    /// WAV format (PCM 16-bit, default)
    #[default]
    Wav,
    /// Opus codec
    Opus,
    /// MP3 format
    Mp3,
}

impl SberTtsAudioFormat {
    /// Get the format string for API requests
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Opus => "opus",
            Self::Mp3 => "mp3",
        }
    }

    /// Get MIME type
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Opus => "audio/opus",
            Self::Mp3 => "audio/mpeg",
        }
    }

    /// Get supported sample rates for this format
    pub fn supported_sample_rates(&self) -> &'static [u32] {
        match self {
            Self::Wav | Self::Opus | Self::Mp3 => &[8000, 24000],
        }
    }
}

impl FromStr for SberTtsAudioFormat {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "wav" | "pcm" | "linear16" | "pcm16" => Ok(Self::Wav),
            "opus" | "ogg_opus" => Ok(Self::Opus),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Unknown SberDevices audio format: '{}'. Available: wav, opus, mp3",
                s
            ))),
        }
    }
}

// =============================================================================
// OAuth Scope
// =============================================================================

/// OAuth scope for SaluteSpeech API access
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SberTtsScope {
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

impl SberTtsScope {
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

impl FromStr for SberTtsScope {
    type Err = TTSError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "SALUTE_SPEECH_PERS" | "PERS" | "PERSONAL" => Ok(Self::Personal),
            "SALUTE_SPEECH_CORP" | "CORP" | "CORPORATE" => Ok(Self::Corporate),
            "SALUTE_SPEECH_B2B" | "B2B" => Ok(Self::B2B),
            "SBER_SPEECH" | "LEGACY" => Ok(Self::Legacy),
            _ => Err(TTSError::InvalidConfiguration(format!(
                "Invalid SberDevices scope: '{}'. Valid: SALUTE_SPEECH_PERS, SALUTE_SPEECH_CORP, SALUTE_SPEECH_B2B, SBER_SPEECH",
                s
            ))),
        }
    }
}

// =============================================================================
// TTS Configuration
// =============================================================================

/// SberDevices SaluteSpeech TTS Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SberTtsConfig {
    /// Base TTS configuration
    #[serde(flatten)]
    pub base: TTSConfig,

    /// Client credentials (Base64 encoded client_id:client_secret)
    #[serde(skip)]
    pub client_credentials: String,

    /// OAuth scope
    #[serde(default)]
    pub scope: SberTtsScope,

    /// Voice selection
    #[serde(default)]
    pub voice: SberTtsVoice,

    /// Audio output format
    #[serde(default)]
    pub audio_format: SberTtsAudioFormat,

    /// Sample rate in Hz (8000 or 24000)
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Connection timeout in seconds
    #[serde(default = "default_connection_timeout")]
    pub connection_timeout_secs: u64,

    /// Request timeout in seconds
    #[serde(default = "default_request_timeout")]
    pub request_timeout_secs: u64,

    /// Treat synthesis input as SSML markup. When set, the synth request is sent with
    /// `Content-Type: application/ssml` instead of `application/text`, so SaluteSpeech parses the
    /// input as SSML (the documented SSML input mode for `/rest/v1/text:synthesize`).
    #[serde(default)]
    pub ssml_input: bool,

    /// Optional base URL override (scheme+host) for the OAuth token and synth REST calls; used by the mock e2e harness to redirect to localhost.
    #[serde(skip)]
    pub endpoint_override: Option<String>,
}

fn default_sample_rate() -> u32 {
    DEFAULT_SAMPLE_RATE
}

fn default_connection_timeout() -> u64 {
    30
}

fn default_request_timeout() -> u64 {
    60
}

impl Default for SberTtsConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig::default(),
            client_credentials: String::new(),
            scope: SberTtsScope::default(),
            voice: SberTtsVoice::default(),
            audio_format: SberTtsAudioFormat::default(),
            sample_rate: default_sample_rate(),
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
            ssml_input: false,
            endpoint_override: None,
        }
    }
}

impl SberTtsConfig {
    /// Create configuration from base TTSConfig
    ///
    /// The api_key should be in format "client_id:client_secret" or
    /// already Base64 encoded credentials.
    pub fn from_base(base: TTSConfig) -> Result<Self, String> {
        // Get credentials from environment or config
        let api_key = std::env::var("SBERDEVICES_API_KEY")
            .or_else(|_| std::env::var("SALUTE_SPEECH_API_KEY"))
            .or_else(|_| std::env::var("SMARTSPEECH_API_KEY"))
            .unwrap_or_else(|_| base.api_key.clone());

        if api_key.is_empty() {
            return Err(
                "SberDevices requires client credentials (client_id:client_secret)".to_string(),
            );
        }

        // Check if already Base64 encoded or needs encoding
        let client_credentials = if api_key.contains(':') {
            // Raw format - encode to Base64
            use base64::{Engine, engine::general_purpose::STANDARD};
            STANDARD.encode(api_key.as_bytes())
        } else {
            // Assume already encoded
            api_key
        };

        // Parse voice
        let voice = base
            .voice_id
            .as_ref()
            .map(|v| SberTtsVoice::from_str(v))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();

        // Parse audio format
        let audio_format = base
            .audio_format
            .as_ref()
            .map(|f| SberTtsAudioFormat::from_str(f))
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();

        // Parse sample rate
        let sample_rate = base.sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);
        if sample_rate != 8000 && sample_rate != 24000 {
            return Err(format!(
                "Invalid sample rate: {}. SberDevices TTS only supports 8000 or 24000 Hz",
                sample_rate
            ));
        }

        // Parse scope from model field
        let scope = if base.model.is_empty() {
            SberTtsScope::default()
        } else {
            base.model.parse().unwrap_or_default()
        };

        Ok(Self {
            base,
            client_credentials,
            scope,
            voice,
            audio_format,
            sample_rate,
            connection_timeout_secs: default_connection_timeout(),
            request_timeout_secs: default_request_timeout(),
            ssml_input: false,
            endpoint_override: None,
        })
    }

    /// Build from the standardized TTS config (W1 keystone). SberDevices SaluteSpeech exposes no
    /// prosody knobs (the voice/format/scope come from base fields). The standardized features with
    /// a real wire effect are `sample_rate` -> `sample_rate` (re-validated to the supported
    /// 8000/24000 Hz set, exactly as `from_base` does) and `ssml` -> `ssml_input` (which flips the
    /// synth request `Content-Type` to `application/ssml`). The non-standard `connection_timeout_secs`
    /// and `request_timeout_secs` knobs are read from the `extras` passthrough.
    ///
    /// `language` is intentionally NOT mapped: the SaluteSpeech REST `text:synthesize` endpoint has
    /// no standalone language parameter (its query is only `voice`/`format`/`sample_rate`, and the
    /// language is fixed by the chosen voice). On the gRPC streaming path the proto carries
    /// `SynthesisRequest.language` (field 3), and for SSML input the language lives in the
    /// `<speak xml:lang="...">` attribute the caller supplies — neither is reachable from this REST
    /// synth without rewriting the URL contract or mutating user-supplied SSML, so it is left as a
    /// capability gap rather than fabricated. Other unsupported features (speed, pitch, volume,
    /// stability, similarity_boost, style, use_speaker_boost, emotion, instructions, word_timestamps,
    /// streaming, seed) are likewise skipped.
    pub fn from_standard(std: &crate::core::tts::standard::StandardTTSConfig) -> Result<Self, String> {
        let f = &std.features;
        let mut cfg = Self::from_base(std.base.clone())?;

        if let Some(rate) = f.sample_rate {
            if rate != 8000 && rate != 24000 {
                return Err(format!(
                    "Invalid sample rate: {}. SberDevices TTS only supports 8000 or 24000 Hz",
                    rate
                ));
            }
            cfg.sample_rate = rate;
        }

        // SSML input mode: flips the synth request Content-Type to `application/ssml`.
        if let Some(true) = f.ssml {
            cfg.ssml_input = true;
        }

        // Provider-specific passthrough.
        if let Some(secs) = std.extras.0.get("connection_timeout_secs").and_then(|v| v.as_u64()) {
            cfg.connection_timeout_secs = secs;
        }
        if let Some(secs) = std.extras.0.get("request_timeout_secs").and_then(|v| v.as_u64()) {
            cfg.request_timeout_secs = secs;
        }

        cfg.endpoint_override = std.endpoint_override().map(String::from);

        Ok(cfg)
    }

    /// The `Content-Type` header value for the synth request: `application/ssml` when the input is
    /// SSML markup, else `application/text`. This is the wire surface that puts SaluteSpeech into
    /// SSML input mode for `/rest/v1/text:synthesize`.
    pub fn synthesis_content_type(&self) -> &'static str {
        if self.ssml_input {
            "application/ssml"
        } else {
            "application/text"
        }
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<(), String> {
        if self.client_credentials.is_empty() {
            return Err(
                "SberDevices API credentials required. Set SBERDEVICES_API_KEY environment variable."
                    .to_string(),
            );
        }

        if self.sample_rate != 8000 && self.sample_rate != 24000 {
            return Err(format!(
                "Invalid sample rate: {}. Must be 8000 or 24000 Hz",
                self.sample_rate
            ));
        }

        Ok(())
    }

    /// Get the Authorization header for OAuth token request
    pub fn oauth_auth_header(&self) -> String {
        format!("Basic {}", self.client_credentials)
    }

    /// Get the full voice ID for API request
    pub fn voice_id(&self) -> String {
        self.voice.voice_id(self.sample_rate)
    }

    /// Build synthesis URL with query parameters. Honors `endpoint_override` (scheme+host swap,
    /// path+query preserved) so the mock e2e harness can redirect the synth POST to localhost.
    pub fn synthesis_url(&self) -> String {
        let url = format!(
            "{}?voice={}&format={}&sample_rate={}",
            SBER_TTS_SYNTHESIZE_URL,
            self.voice_id(),
            self.audio_format.as_str(),
            self.sample_rate
        );
        crate::core::tts::standard::override_rest_endpoint(&url, self.endpoint_override.as_deref())
    }

    /// OAuth token endpoint, honoring `endpoint_override` (scheme+host swap, path preserved) so the
    /// mock e2e harness can redirect the token POST to localhost.
    pub fn oauth_url(&self) -> String {
        crate::core::tts::standard::override_rest_endpoint(
            OAUTH_ENDPOINT,
            self.endpoint_override.as_deref(),
        )
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: SberDevices can express the output sample rate (re-validated to 8000/24000 Hz)
    // and SSML input mode (ssml -> ssml_input -> application/ssml Content-Type); the non-standard
    // timeout knobs flow through extras.
    #[test]
    fn from_standard_maps_sample_rate_ssml_and_extras() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("connection_timeout_secs".into(), serde_json::json!(15));
        extras.insert("request_timeout_secs".into(), serde_json::json!(45));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "sberdevices".into(),
                api_key: "test_client:test_secret".into(),
                voice_id: Some("Bys".into()),
                sample_rate: Some(24000),
                ..Default::default()
            },
            features: TtsFeatures {
                sample_rate: Some(8000),
                ssml: Some(true), // SSML input mode -> ssml_input (now wired)
                speed: Some(1.2), // capability gap: SberDevices has no speed field, must be ignored
                ..Default::default()
            },
            extras: crate::core::stt::standard::ProviderExtras(extras),
        };
        let cfg = SberTtsConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.sample_rate, 8000); // feature override re-validated to supported set
        assert_eq!(cfg.voice, SberTtsVoice::Bys);
        assert!(cfg.ssml_input); // ssml feature mapped
        assert_eq!(cfg.synthesis_content_type(), "application/ssml");
        assert_eq!(cfg.connection_timeout_secs, 15); // extras passthrough
        assert_eq!(cfg.request_timeout_secs, 45); // extras passthrough
    }

    #[test]
    fn test_voice_from_str() {
        assert_eq!(SberTtsVoice::from_str("Nec").unwrap(), SberTtsVoice::Nec);
        assert_eq!(SberTtsVoice::from_str("nec").unwrap(), SberTtsVoice::Nec);
        assert_eq!(
            SberTtsVoice::from_str("Natalia").unwrap(),
            SberTtsVoice::Nec
        );
        assert_eq!(SberTtsVoice::from_str("Bys").unwrap(), SberTtsVoice::Bys);
        assert_eq!(SberTtsVoice::from_str("Boris").unwrap(), SberTtsVoice::Bys);
        assert_eq!(SberTtsVoice::from_str("May").unwrap(), SberTtsVoice::May);
        assert_eq!(SberTtsVoice::from_str("Tur").unwrap(), SberTtsVoice::Tur);
        assert_eq!(SberTtsVoice::from_str("Ost").unwrap(), SberTtsVoice::Ost);
        assert_eq!(SberTtsVoice::from_str("Pon").unwrap(), SberTtsVoice::Pon);
        assert_eq!(SberTtsVoice::from_str("Kin").unwrap(), SberTtsVoice::Kin);
        assert_eq!(SberTtsVoice::from_str("Kira").unwrap(), SberTtsVoice::Kin);
    }

    #[test]
    fn test_voice_with_sample_rate_suffix() {
        assert_eq!(
            SberTtsVoice::from_str("Nec_24000").unwrap(),
            SberTtsVoice::Nec
        );
        assert_eq!(
            SberTtsVoice::from_str("Bys_8000").unwrap(),
            SberTtsVoice::Bys
        );
    }

    #[test]
    fn test_voice_invalid() {
        assert!(SberTtsVoice::from_str("invalid").is_err());
        assert!(SberTtsVoice::from_str("unknown").is_err());
    }

    #[test]
    fn test_voice_as_str() {
        assert_eq!(SberTtsVoice::Nec.as_str(), "Nec");
        assert_eq!(SberTtsVoice::Bys.as_str(), "Bys");
        assert_eq!(SberTtsVoice::May.as_str(), "May");
        assert_eq!(SberTtsVoice::Tur.as_str(), "Tur");
        assert_eq!(SberTtsVoice::Ost.as_str(), "Ost");
        assert_eq!(SberTtsVoice::Pon.as_str(), "Pon");
        assert_eq!(SberTtsVoice::Kin.as_str(), "Kin");
    }

    #[test]
    fn test_voice_id_with_sample_rate() {
        assert_eq!(SberTtsVoice::Nec.voice_id(24000), "Nec_24000");
        assert_eq!(SberTtsVoice::Bys.voice_id(8000), "Bys_8000");
    }

    #[test]
    fn test_voice_gender() {
        assert_eq!(SberTtsVoice::Nec.gender(), "female");
        assert_eq!(SberTtsVoice::Bys.gender(), "male");
        assert_eq!(SberTtsVoice::May.gender(), "female");
        assert_eq!(SberTtsVoice::Tur.gender(), "male");
        assert_eq!(SberTtsVoice::Ost.gender(), "female");
        assert_eq!(SberTtsVoice::Pon.gender(), "male");
        assert_eq!(SberTtsVoice::Kin.gender(), "female");
    }

    #[test]
    fn test_voice_language() {
        assert_eq!(SberTtsVoice::Nec.language(), "ru-RU");
        assert_eq!(SberTtsVoice::Bys.language(), "ru-RU");
        assert_eq!(SberTtsVoice::Kin.language(), "en-US");
    }

    #[test]
    fn test_voice_all() {
        let voices = SberTtsVoice::all();
        assert_eq!(voices.len(), 7);
        assert!(voices.contains(&SberTtsVoice::Nec));
        assert!(voices.contains(&SberTtsVoice::Kin));
    }

    #[test]
    fn test_voices_for_language() {
        let russian_voices = SberTtsVoice::voices_for_language("ru-RU");
        assert_eq!(russian_voices.len(), 6);
        assert!(!russian_voices.contains(&SberTtsVoice::Kin));

        let english_voices = SberTtsVoice::voices_for_language("en-US");
        assert_eq!(english_voices.len(), 1);
        assert!(english_voices.contains(&SberTtsVoice::Kin));
    }

    #[test]
    fn test_audio_format_from_str() {
        assert_eq!(
            SberTtsAudioFormat::from_str("wav").unwrap(),
            SberTtsAudioFormat::Wav
        );
        assert_eq!(
            SberTtsAudioFormat::from_str("pcm").unwrap(),
            SberTtsAudioFormat::Wav
        );
        assert_eq!(
            SberTtsAudioFormat::from_str("opus").unwrap(),
            SberTtsAudioFormat::Opus
        );
        assert_eq!(
            SberTtsAudioFormat::from_str("mp3").unwrap(),
            SberTtsAudioFormat::Mp3
        );
        assert_eq!(
            SberTtsAudioFormat::from_str("mpeg").unwrap(),
            SberTtsAudioFormat::Mp3
        );
    }

    #[test]
    fn test_audio_format_invalid() {
        assert!(SberTtsAudioFormat::from_str("flac").is_err());
        assert!(SberTtsAudioFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_audio_format_as_str() {
        assert_eq!(SberTtsAudioFormat::Wav.as_str(), "wav");
        assert_eq!(SberTtsAudioFormat::Opus.as_str(), "opus");
        assert_eq!(SberTtsAudioFormat::Mp3.as_str(), "mp3");
    }

    #[test]
    fn test_audio_format_mime_type() {
        assert_eq!(SberTtsAudioFormat::Wav.mime_type(), "audio/wav");
        assert_eq!(SberTtsAudioFormat::Opus.mime_type(), "audio/opus");
        assert_eq!(SberTtsAudioFormat::Mp3.mime_type(), "audio/mpeg");
    }

    #[test]
    fn test_scope_from_str() {
        assert_eq!(
            SberTtsScope::from_str("SALUTE_SPEECH_PERS").unwrap(),
            SberTtsScope::Personal
        );
        assert_eq!(
            SberTtsScope::from_str("PERS").unwrap(),
            SberTtsScope::Personal
        );
        assert_eq!(
            SberTtsScope::from_str("SALUTE_SPEECH_CORP").unwrap(),
            SberTtsScope::Corporate
        );
        assert_eq!(
            SberTtsScope::from_str("SALUTE_SPEECH_B2B").unwrap(),
            SberTtsScope::B2B
        );
        assert_eq!(
            SberTtsScope::from_str("SBER_SPEECH").unwrap(),
            SberTtsScope::Legacy
        );
    }

    #[test]
    fn test_scope_as_str() {
        assert_eq!(SberTtsScope::Personal.as_str(), "SALUTE_SPEECH_PERS");
        assert_eq!(SberTtsScope::Corporate.as_str(), "SALUTE_SPEECH_CORP");
        assert_eq!(SberTtsScope::B2B.as_str(), "SALUTE_SPEECH_B2B");
        assert_eq!(SberTtsScope::Legacy.as_str(), "SBER_SPEECH");
    }

    #[test]
    fn test_config_from_base() {
        let base = TTSConfig {
            api_key: "test_client:test_secret".to_string(),
            voice_id: Some("Nec".to_string()),
            audio_format: Some("wav".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        };

        let config = SberTtsConfig::from_base(base).unwrap();

        // Should be Base64 encoded
        use base64::{Engine, engine::general_purpose::STANDARD};
        let expected = STANDARD.encode("test_client:test_secret".as_bytes());
        assert_eq!(config.client_credentials, expected);
        assert_eq!(config.voice, SberTtsVoice::Nec);
        assert_eq!(config.audio_format, SberTtsAudioFormat::Wav);
        assert_eq!(config.sample_rate, 24000);
    }

    #[test]
    fn test_config_from_base_with_encoded_credentials() {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let encoded = STANDARD.encode("client:secret".as_bytes());

        let base = TTSConfig {
            api_key: encoded.clone(),
            voice_id: Some("Bys".to_string()),
            sample_rate: Some(8000),
            ..Default::default()
        };

        let config = SberTtsConfig::from_base(base).unwrap();

        // Should use as-is (no colon means already encoded)
        assert_eq!(config.client_credentials, encoded);
        assert_eq!(config.voice, SberTtsVoice::Bys);
        assert_eq!(config.sample_rate, 8000);
    }

    #[test]
    fn test_config_requires_credentials() {
        let base = TTSConfig {
            api_key: String::new(),
            ..Default::default()
        };

        let result = SberTtsConfig::from_base(base);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("credentials"));
    }

    #[test]
    fn test_config_invalid_sample_rate() {
        let base = TTSConfig {
            api_key: "test:secret".to_string(),
            voice_id: None,           // Use default SberDevices voice
            sample_rate: Some(16000), // Invalid - only 8000 or 24000
            ..Default::default()
        };

        let result = SberTtsConfig::from_base(base);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("sample rate"));
    }

    #[test]
    fn test_config_defaults() {
        let base = TTSConfig {
            api_key: "test:secret".to_string(),
            voice_id: None,
            audio_format: None,
            sample_rate: None,
            ..Default::default()
        };

        let config = SberTtsConfig::from_base(base).unwrap();
        assert_eq!(config.voice, SberTtsVoice::Nec);
        assert_eq!(config.audio_format, SberTtsAudioFormat::Wav);
        assert_eq!(config.sample_rate, DEFAULT_SAMPLE_RATE);
        assert_eq!(config.scope, SberTtsScope::Personal);
    }

    #[test]
    fn test_config_with_scope() {
        let base = TTSConfig {
            api_key: "test:secret".to_string(),
            voice_id: None, // Use default SberDevices voice
            model: "SALUTE_SPEECH_CORP".to_string(),
            ..Default::default()
        };

        let config = SberTtsConfig::from_base(base).unwrap();
        assert_eq!(config.scope, SberTtsScope::Corporate);
    }

    #[test]
    fn test_oauth_auth_header() {
        let base = TTSConfig {
            api_key: "test:secret".to_string(),
            voice_id: None, // Use default SberDevices voice
            ..Default::default()
        };

        let config = SberTtsConfig::from_base(base).unwrap();
        let header = config.oauth_auth_header();

        assert!(header.starts_with("Basic "));
        assert!(header.len() > 10);
    }

    #[test]
    fn test_voice_id() {
        let base = TTSConfig {
            api_key: "test:secret".to_string(),
            voice_id: Some("Nec".to_string()),
            sample_rate: Some(24000),
            ..Default::default()
        };

        let config = SberTtsConfig::from_base(base).unwrap();
        assert_eq!(config.voice_id(), "Nec_24000");
    }

    #[test]
    fn test_synthesis_url() {
        let base = TTSConfig {
            api_key: "test:secret".to_string(),
            voice_id: Some("Bys".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(8000),
            ..Default::default()
        };

        let config = SberTtsConfig::from_base(base).unwrap();
        let url = config.synthesis_url();

        assert!(url.contains("voice=Bys_8000"));
        assert!(url.contains("format=mp3"));
        assert!(url.contains("sample_rate=8000"));
    }

    #[test]
    fn test_config_validation() {
        let mut config = SberTtsConfig::default();
        assert!(config.validate().is_err());

        config.client_credentials = "test_credentials".to_string();
        assert!(config.validate().is_ok());

        config.sample_rate = 16000; // Invalid
        assert!(config.validate().is_err());
    }
}
