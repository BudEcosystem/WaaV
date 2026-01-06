//! IBM Watson Speech-to-Text configuration.
//!
//! This module defines the configuration structures and enums specific to
//! IBM Watson Speech-to-Text API.

use crate::core::stt::base::STTConfig;
use serde::{Deserialize, Serialize};
use url::form_urlencoded;

// =============================================================================
// Constants
// =============================================================================

/// Default IBM Watson STT model for English (US) multimedia content.
pub const DEFAULT_MODEL: &str = "en-US_Multimedia";

/// IBM Watson IAM authentication endpoint.
pub const IBM_IAM_URL: &str = "https://iam.cloud.ibm.com/identity/token";

/// Default inactivity timeout in seconds (30 seconds).
pub const DEFAULT_INACTIVITY_TIMEOUT: i32 = 30;

// =============================================================================
// Region Configuration
// =============================================================================

/// IBM Watson Speech-to-Text service regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IbmRegion {
    /// Dallas, Texas (US South)
    #[default]
    UsSouth,
    /// Washington, D.C. (US East)
    UsEast,
    /// Frankfurt, Germany (EU Central)
    EuDe,
    /// London, UK (EU GB)
    EuGb,
    /// Sydney, Australia (AU SYD)
    AuSyd,
    /// Tokyo, Japan (JP TOK)
    JpTok,
    /// Seoul, South Korea (KR SEO)
    KrSeo,
}

impl IbmRegion {
    /// Get the Speech-to-Text API hostname for this region.
    pub fn stt_hostname(&self) -> &'static str {
        match self {
            Self::UsSouth => "api.us-south.speech-to-text.watson.cloud.ibm.com",
            Self::UsEast => "api.us-east.speech-to-text.watson.cloud.ibm.com",
            Self::EuDe => "api.eu-de.speech-to-text.watson.cloud.ibm.com",
            Self::EuGb => "api.eu-gb.speech-to-text.watson.cloud.ibm.com",
            Self::AuSyd => "api.au-syd.speech-to-text.watson.cloud.ibm.com",
            Self::JpTok => "api.jp-tok.speech-to-text.watson.cloud.ibm.com",
            Self::KrSeo => "api.kr-seo.speech-to-text.watson.cloud.ibm.com",
        }
    }

    /// Get the Text-to-Speech API hostname for this region.
    pub fn tts_hostname(&self) -> &'static str {
        match self {
            Self::UsSouth => "api.us-south.text-to-speech.watson.cloud.ibm.com",
            Self::UsEast => "api.us-east.text-to-speech.watson.cloud.ibm.com",
            Self::EuDe => "api.eu-de.text-to-speech.watson.cloud.ibm.com",
            Self::EuGb => "api.eu-gb.text-to-speech.watson.cloud.ibm.com",
            Self::AuSyd => "api.au-syd.text-to-speech.watson.cloud.ibm.com",
            Self::JpTok => "api.jp-tok.text-to-speech.watson.cloud.ibm.com",
            Self::KrSeo => "api.kr-seo.text-to-speech.watson.cloud.ibm.com",
        }
    }

    /// Get the region code string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::UsSouth => "us-south",
            Self::UsEast => "us-east",
            Self::EuDe => "eu-de",
            Self::EuGb => "eu-gb",
            Self::AuSyd => "au-syd",
            Self::JpTok => "jp-tok",
            Self::KrSeo => "kr-seo",
        }
    }
}

impl std::fmt::Display for IbmRegion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Model Configuration
// =============================================================================

/// IBM Watson Speech-to-Text models.
///
/// IBM provides two types of models:
/// - Multimedia: Optimized for high-quality audio (16kHz+)
/// - Telephony: Optimized for telephone audio (8kHz)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IbmModel {
    // English models
    EnUsMultimedia,
    EnUsTelephony,
    EnGbMultimedia,
    EnGbTelephony,
    EnAuMultimedia,
    EnAuTelephony,

    // Spanish models
    EsEsMultimedia,
    EsEsTelephony,
    EsLaMultimedia,
    EsLaTelephony,

    // French models
    FrFrMultimedia,
    FrFrTelephony,
    FrCaMultimedia,
    FrCaTelephony,

    // German models
    DeDeMultimedia,
    DeDeTelephony,

    // Italian models
    ItItMultimedia,
    ItItTelephony,

    // Portuguese models
    PtBrMultimedia,
    PtBrTelephony,

    // Japanese models
    JaJpMultimedia,
    JaJpTelephony,

    // Korean models
    KoKrMultimedia,
    KoKrTelephony,

    // Chinese models
    ZhCnMultimedia,
    ZhCnTelephony,

    // Dutch models
    NlNlMultimedia,
    NlNlTelephony,

    // Arabic models
    ArMsMultimedia,
    ArMsTelephony,

    // Hindi models
    HiInMultimedia,
    HiInTelephony,

    /// Custom model (user-specified model name)
    Custom(String),
}

impl IbmModel {
    /// Get the model identifier string for the API.
    pub fn as_str(&self) -> &str {
        match self {
            Self::EnUsMultimedia => "en-US_Multimedia",
            Self::EnUsTelephony => "en-US_Telephony",
            Self::EnGbMultimedia => "en-GB_Multimedia",
            Self::EnGbTelephony => "en-GB_Telephony",
            Self::EnAuMultimedia => "en-AU_Multimedia",
            Self::EnAuTelephony => "en-AU_Telephony",
            Self::EsEsMultimedia => "es-ES_Multimedia",
            Self::EsEsTelephony => "es-ES_Telephony",
            Self::EsLaMultimedia => "es-LA_Multimedia",
            Self::EsLaTelephony => "es-LA_Telephony",
            Self::FrFrMultimedia => "fr-FR_Multimedia",
            Self::FrFrTelephony => "fr-FR_Telephony",
            Self::FrCaMultimedia => "fr-CA_Multimedia",
            Self::FrCaTelephony => "fr-CA_Telephony",
            Self::DeDeMultimedia => "de-DE_Multimedia",
            Self::DeDeTelephony => "de-DE_Telephony",
            Self::ItItMultimedia => "it-IT_Multimedia",
            Self::ItItTelephony => "it-IT_Telephony",
            Self::PtBrMultimedia => "pt-BR_Multimedia",
            Self::PtBrTelephony => "pt-BR_Telephony",
            Self::JaJpMultimedia => "ja-JP_Multimedia",
            Self::JaJpTelephony => "ja-JP_Telephony",
            Self::KoKrMultimedia => "ko-KR_Multimedia",
            Self::KoKrTelephony => "ko-KR_Telephony",
            Self::ZhCnMultimedia => "zh-CN_Multimedia",
            Self::ZhCnTelephony => "zh-CN_Telephony",
            Self::NlNlMultimedia => "nl-NL_Multimedia",
            Self::NlNlTelephony => "nl-NL_Telephony",
            Self::ArMsMultimedia => "ar-MS_Multimedia",
            Self::ArMsTelephony => "ar-MS_Telephony",
            Self::HiInMultimedia => "hi-IN_Multimedia",
            Self::HiInTelephony => "hi-IN_Telephony",
            Self::Custom(name) => name,
        }
    }

    /// Get recommended sample rate for this model.
    pub fn recommended_sample_rate(&self) -> u32 {
        match self {
            // Telephony models are optimized for 8kHz
            Self::EnUsTelephony
            | Self::EnGbTelephony
            | Self::EnAuTelephony
            | Self::EsEsTelephony
            | Self::EsLaTelephony
            | Self::FrFrTelephony
            | Self::FrCaTelephony
            | Self::DeDeTelephony
            | Self::ItItTelephony
            | Self::PtBrTelephony
            | Self::JaJpTelephony
            | Self::KoKrTelephony
            | Self::ZhCnTelephony
            | Self::NlNlTelephony
            | Self::ArMsTelephony
            | Self::HiInTelephony => 8000,
            // Multimedia models support 16kHz
            _ => 16000,
        }
    }
}

impl Default for IbmModel {
    fn default() -> Self {
        Self::EnUsMultimedia
    }
}

impl std::fmt::Display for IbmModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Audio Encoding
// =============================================================================

/// Audio encoding formats supported by IBM Watson STT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IbmAudioEncoding {
    /// Linear PCM 16-bit little-endian signed integer.
    #[default]
    Linear16,
    /// Mu-law encoded audio.
    Mulaw,
    /// A-law encoded audio.
    Alaw,
    /// FLAC encoded audio.
    Flac,
    /// Opus encoded in OGG container.
    OggOpus,
    /// Opus encoded in WebM container.
    WebmOpus,
    /// MP3 encoded audio.
    Mp3,
}

impl IbmAudioEncoding {
    /// Get the content-type MIME string for this encoding.
    pub fn content_type(&self, sample_rate: u32) -> String {
        match self {
            Self::Linear16 => format!("audio/l16;rate={};channels=1", sample_rate),
            Self::Mulaw => format!("audio/mulaw;rate={}", sample_rate),
            Self::Alaw => format!("audio/alaw;rate={}", sample_rate),
            Self::Flac => "audio/flac".to_string(),
            Self::OggOpus => "audio/ogg;codecs=opus".to_string(),
            Self::WebmOpus => "audio/webm;codecs=opus".to_string(),
            Self::Mp3 => "audio/mp3".to_string(),
        }
    }
}

// =============================================================================
// IBM Watson STT Configuration
// =============================================================================

/// IBM Watson Speech-to-Text configuration.
///
/// This struct extends the base STT configuration with IBM-specific settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IbmWatsonSTTConfig {
    /// Base STT configuration (api_key, language, sample_rate, etc.)
    #[serde(flatten)]
    pub base: STTConfig,

    /// IBM Cloud region for the service.
    #[serde(default)]
    pub region: IbmRegion,

    /// Service instance ID (required for WebSocket URL).
    /// Found in IBM Cloud service credentials.
    pub instance_id: String,

    /// Speech recognition model to use.
    #[serde(default)]
    pub model: IbmModel,

    /// Audio encoding format.
    #[serde(default)]
    pub encoding: IbmAudioEncoding,

    /// Enable interim (partial) recognition results.
    #[serde(default = "default_interim_results")]
    pub interim_results: bool,

    /// Enable word-level timestamps in results.
    #[serde(default)]
    pub word_timestamps: bool,

    /// Enable word-level confidence scores.
    #[serde(default)]
    pub word_confidence: bool,

    /// Enable speaker diarization (speaker labels).
    #[serde(default)]
    pub speaker_labels: bool,

    /// Enable smart formatting (dates, times, numbers, etc.).
    #[serde(default)]
    pub smart_formatting: bool,

    /// Profanity filter mode.
    #[serde(default)]
    pub profanity_filter: bool,

    /// Redaction of sensitive data (PII).
    #[serde(default)]
    pub redaction: bool,

    /// Inactivity timeout in seconds (default: 30).
    /// Connection closes if no audio received within this time.
    #[serde(default = "default_inactivity_timeout")]
    pub inactivity_timeout: i32,

    /// Custom language model ID (for domain-specific models).
    pub language_model_id: Option<String>,

    /// Custom acoustic model ID.
    pub acoustic_model_id: Option<String>,

    /// Background audio suppression level (0.0 to 1.0).
    pub background_audio_suppression: Option<f32>,

    /// Speech detector sensitivity (0.0 to 1.0).
    pub speech_detector_sensitivity: Option<f32>,

    /// End-of-phrase silence time in seconds (0.0 to 120.0).
    pub end_of_phrase_silence_time: Option<f32>,

    /// Split transcript at phrase end.
    #[serde(default)]
    pub split_transcript_at_phrase_end: bool,

    /// Low latency mode for faster interim results (may reduce accuracy).
    #[serde(default)]
    pub low_latency: bool,

    /// Character insertion bias (-1.0 to 1.0).
    pub character_insertion_bias: Option<f32>,
}

fn default_interim_results() -> bool {
    true
}

fn default_inactivity_timeout() -> i32 {
    DEFAULT_INACTIVITY_TIMEOUT
}

impl Default for IbmWatsonSTTConfig {
    fn default() -> Self {
        Self {
            base: STTConfig::default(),
            region: IbmRegion::default(),
            instance_id: String::new(),
            model: IbmModel::default(),
            encoding: IbmAudioEncoding::default(),
            interim_results: true,
            word_timestamps: false,
            word_confidence: false,
            speaker_labels: false,
            smart_formatting: false,
            profanity_filter: false,
            redaction: false,
            inactivity_timeout: DEFAULT_INACTIVITY_TIMEOUT,
            language_model_id: None,
            acoustic_model_id: None,
            background_audio_suppression: None,
            speech_detector_sensitivity: None,
            end_of_phrase_silence_time: None,
            split_transcript_at_phrase_end: false,
            low_latency: false,
            character_insertion_bias: None,
        }
    }
}

impl IbmWatsonSTTConfig {
    /// Create IBM Watson STT configuration from base STT config.
    ///
    /// # Arguments
    /// * `base` - Base STT configuration
    /// * `instance_id` - IBM Cloud service instance ID
    pub fn from_base(base: STTConfig, instance_id: String) -> Self {
        // Determine model based on language in base config
        let model = match base.language.as_str() {
            "en-US" => IbmModel::EnUsMultimedia,
            "en-GB" => IbmModel::EnGbMultimedia,
            "en-AU" => IbmModel::EnAuMultimedia,
            "es-ES" => IbmModel::EsEsMultimedia,
            "es-LA" | "es-MX" => IbmModel::EsLaMultimedia,
            "fr-FR" => IbmModel::FrFrMultimedia,
            "fr-CA" => IbmModel::FrCaMultimedia,
            "de-DE" => IbmModel::DeDeMultimedia,
            "it-IT" => IbmModel::ItItMultimedia,
            "pt-BR" => IbmModel::PtBrMultimedia,
            "ja-JP" => IbmModel::JaJpMultimedia,
            "ko-KR" => IbmModel::KoKrMultimedia,
            "zh-CN" => IbmModel::ZhCnMultimedia,
            "nl-NL" => IbmModel::NlNlMultimedia,
            "ar-MS" | "ar" => IbmModel::ArMsMultimedia,
            "hi-IN" | "hi" => IbmModel::HiInMultimedia,
            _ => IbmModel::EnUsMultimedia,
        };

        // Determine encoding based on base config
        let encoding = match base.encoding.as_str() {
            "linear16" | "pcm" => IbmAudioEncoding::Linear16,
            "mulaw" | "mu-law" => IbmAudioEncoding::Mulaw,
            "alaw" | "a-law" => IbmAudioEncoding::Alaw,
            "flac" => IbmAudioEncoding::Flac,
            "ogg-opus" | "opus" => IbmAudioEncoding::OggOpus,
            "webm-opus" => IbmAudioEncoding::WebmOpus,
            "mp3" => IbmAudioEncoding::Mp3,
            _ => IbmAudioEncoding::Linear16,
        };

        Self {
            base,
            region: IbmRegion::default(),
            instance_id,
            model,
            encoding,
            interim_results: true,
            word_timestamps: false,
            word_confidence: false,
            speaker_labels: false,
            smart_formatting: false,
            profanity_filter: false,
            redaction: false,
            inactivity_timeout: DEFAULT_INACTIVITY_TIMEOUT,
            language_model_id: None,
            acoustic_model_id: None,
            background_audio_suppression: None,
            speech_detector_sensitivity: None,
            end_of_phrase_silence_time: None,
            split_transcript_at_phrase_end: false,
            low_latency: false,
            character_insertion_bias: None,
        }
    }

    /// Build the WebSocket URL for connecting to IBM Watson STT.
    pub fn build_websocket_url(&self, access_token: &str) -> String {
        let base_url = format!(
            "wss://{}/instances/{}/v1/recognize",
            self.region.stt_hostname(),
            self.instance_id
        );

        // URL-encode helper using form_urlencoded
        fn encode(s: &str) -> String {
            form_urlencoded::byte_serialize(s.as_bytes()).collect()
        }

        // Build query parameters
        let mut params = vec![
            format!("access_token={}", encode(access_token)),
            format!("model={}", encode(self.model.as_str())),
        ];

        // Add optional parameters
        if let Some(ref lm_id) = self.language_model_id {
            params.push(format!("language_customization_id={}", encode(lm_id)));
        }

        if let Some(ref am_id) = self.acoustic_model_id {
            params.push(format!("acoustic_customization_id={}", encode(am_id)));
        }

        format!("{}?{}", base_url, params.join("&"))
    }

    /// Build the start recognition message for WebSocket.
    pub fn build_start_message(&self) -> serde_json::Value {
        let mut msg = serde_json::json!({
            "action": "start",
            "content-type": self.encoding.content_type(self.base.sample_rate),
            "interim_results": self.interim_results,
            "timestamps": self.word_timestamps,
            "word_confidence": self.word_confidence,
            "speaker_labels": self.speaker_labels,
            "smart_formatting": self.smart_formatting,
            "profanity_filter": self.profanity_filter,
            "redaction": self.redaction,
            "inactivity_timeout": self.inactivity_timeout,
            "split_transcript_at_phrase_end": self.split_transcript_at_phrase_end,
            "low_latency": self.low_latency,
        });

        // Add optional parameters if set
        if let Some(bas) = self.background_audio_suppression {
            msg["background_audio_suppression"] = serde_json::json!(bas);
        }
        if let Some(sds) = self.speech_detector_sensitivity {
            msg["speech_detector_sensitivity"] = serde_json::json!(sds);
        }
        if let Some(eps) = self.end_of_phrase_silence_time {
            msg["end_of_phrase_silence_time"] = serde_json::json!(eps);
        }
        if let Some(cib) = self.character_insertion_bias {
            msg["character_insertion_bias"] = serde_json::json!(cib);
        }

        msg
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_region_hostnames() {
        assert_eq!(
            IbmRegion::UsSouth.stt_hostname(),
            "api.us-south.speech-to-text.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::EuDe.stt_hostname(),
            "api.eu-de.speech-to-text.watson.cloud.ibm.com"
        );
        assert_eq!(
            IbmRegion::JpTok.tts_hostname(),
            "api.jp-tok.text-to-speech.watson.cloud.ibm.com"
        );
    }

    #[test]
    fn test_model_names() {
        assert_eq!(IbmModel::EnUsMultimedia.as_str(), "en-US_Multimedia");
        assert_eq!(IbmModel::EnUsTelephony.as_str(), "en-US_Telephony");
        assert_eq!(IbmModel::DeDeMultimedia.as_str(), "de-DE_Multimedia");
        assert_eq!(
            IbmModel::Custom("custom-model-123".to_string()).as_str(),
            "custom-model-123"
        );
    }

    #[test]
    fn test_model_sample_rates() {
        assert_eq!(IbmModel::EnUsMultimedia.recommended_sample_rate(), 16000);
        assert_eq!(IbmModel::EnUsTelephony.recommended_sample_rate(), 8000);
    }

    #[test]
    fn test_audio_encoding_content_type() {
        assert_eq!(
            IbmAudioEncoding::Linear16.content_type(16000),
            "audio/l16;rate=16000;channels=1"
        );
        assert_eq!(
            IbmAudioEncoding::Mulaw.content_type(8000),
            "audio/mulaw;rate=8000"
        );
        assert_eq!(IbmAudioEncoding::Flac.content_type(16000), "audio/flac");
        assert_eq!(
            IbmAudioEncoding::OggOpus.content_type(16000),
            "audio/ogg;codecs=opus"
        );
    }

    #[test]
    fn test_default_config() {
        let config = IbmWatsonSTTConfig::default();
        assert_eq!(config.region, IbmRegion::UsSouth);
        assert_eq!(config.model, IbmModel::EnUsMultimedia);
        assert!(config.interim_results);
        assert_eq!(config.inactivity_timeout, 30);
    }

    #[test]
    fn test_from_base_config() {
        let base = STTConfig {
            api_key: "test-api-key".to_string(),
            language: "de-DE".to_string(),
            sample_rate: 16000,
            ..Default::default()
        };

        let config = IbmWatsonSTTConfig::from_base(base, "test-instance-id".to_string());

        assert_eq!(config.model, IbmModel::DeDeMultimedia);
        assert_eq!(config.instance_id, "test-instance-id");
    }

    #[test]
    fn test_build_websocket_url() {
        let config = IbmWatsonSTTConfig {
            region: IbmRegion::UsSouth,
            instance_id: "test-instance-123".to_string(),
            model: IbmModel::EnUsMultimedia,
            ..Default::default()
        };

        let url = config.build_websocket_url("test-token");
        assert!(url.starts_with("wss://api.us-south.speech-to-text.watson.cloud.ibm.com"));
        assert!(url.contains("instances/test-instance-123"));
        assert!(url.contains("access_token=test-token"));
        assert!(url.contains("model=en-US_Multimedia"));
    }

    #[test]
    fn test_build_start_message() {
        let config = IbmWatsonSTTConfig {
            base: STTConfig {
                sample_rate: 16000,
                ..Default::default()
            },
            interim_results: true,
            word_timestamps: true,
            smart_formatting: true,
            ..Default::default()
        };

        let msg = config.build_start_message();
        assert_eq!(msg["action"], "start");
        assert_eq!(msg["interim_results"], true);
        assert_eq!(msg["timestamps"], true);
        assert_eq!(msg["smart_formatting"], true);
    }
}
