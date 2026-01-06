//! IBM Watson Text-to-Speech configuration.
//!
//! This module defines the configuration structures and enums specific to
//! IBM Watson Text-to-Speech API.
//!
//! # Features
//!
//! - Multiple neural and enhanced neural voices
//! - Support for SSML (Speech Synthesis Markup Language)
//! - Multiple audio output formats
//! - Custom word pronunciations
//! - Voice transformation capabilities
//!
//! # References
//!
//! - [API Reference](https://cloud.ibm.com/apidocs/text-to-speech)
//! - [Voices Documentation](https://cloud.ibm.com/docs/text-to-speech?topic=text-to-speech-voices)

use crate::core::stt::ibm_watson::IbmRegion;
use crate::core::tts::base::TTSConfig;
use serde::{Deserialize, Serialize};

// =============================================================================
// Constants
// =============================================================================

/// IBM Watson IAM authentication endpoint (shared with STT).
pub const IBM_IAM_URL: &str = "https://iam.cloud.ibm.com/identity/token";

/// Default IBM Watson TTS voice for English (US).
pub const DEFAULT_VOICE: &str = "en-US_AllisonV3Voice";

/// Maximum text length for single synthesis request in bytes (5KB = 5120 bytes).
/// IBM Watson counts bytes, not characters, so multibyte UTF-8 chars count more.
pub const MAX_TEXT_LENGTH: usize = 5120;

/// Default sample rate for PCM audio (22050 Hz).
pub const DEFAULT_SAMPLE_RATE: u32 = 22050;

// =============================================================================
// Voice Configuration
// =============================================================================

/// IBM Watson Text-to-Speech voices.
///
/// IBM provides several voice types:
/// - **V3 Voices**: Standard neural voices (most languages)
/// - **Enhanced Neural Voices**: Higher quality voices (select languages)
///
/// Voices are available in multiple languages with various characteristics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum IbmVoice {
    // US English voices
    /// Allison - US English, Female, V3 Neural
    EnUsAllisonV3Voice,
    /// Emily - US English, Female, V3 Neural
    EnUsEmilyV3Voice,
    /// Henry - US English, Male, V3 Neural
    EnUsHenryV3Voice,
    /// Kevin - US English, Male, V3 Neural
    EnUsKevinV3Voice,
    /// Lisa - US English, Female, V3 Neural
    EnUsLisaV3Voice,
    /// Michael - US English, Male, V3 Neural
    EnUsMichaelV3Voice,
    /// Olivia - US English, Female, V3 Neural
    EnUsOliviaV3Voice,

    // UK English voices
    /// Charlotte - UK English, Female, V3 Neural
    EnGbCharlotteV3Voice,
    /// James - UK English, Male, V3 Neural
    EnGbJamesV3Voice,
    /// Kate - UK English, Female, V3 Neural
    EnGbKateV3Voice,

    // Australian English voices
    /// Craig - Australian English, Male, V3 Neural
    EnAuCraigV3Voice,
    /// Madison - Australian English, Female, V3 Neural
    EnAuMadisonV3Voice,

    // German voices
    /// Birgit - German, Female, V3 Neural
    DeDeBirgitV3Voice,
    /// Dieter - German, Male, V3 Neural
    DeDeDieterV3Voice,
    /// Erika - German, Female, V3 Neural
    DeDeErikaV3Voice,

    // Spanish (Castilian) voices
    /// Enrique - Spanish (Castilian), Male, V3 Neural
    EsEsEnriqueV3Voice,
    /// Laura - Spanish (Castilian), Female, V3 Neural
    EsEsLauraV3Voice,

    // Spanish (Latin American) voices
    /// Sofia - Spanish (Latin American), Female, V3 Neural
    EsLaSofiaV3Voice,

    // Spanish (North American) voices
    /// Sofia - Spanish (North American), Female, V3 Neural
    EsUsSofiaV3Voice,

    // French voices
    /// Nicolas - French, Male, V3 Neural
    FrFrNicolasV3Voice,
    /// Renee - French, Female, V3 Neural
    FrFrReneeV3Voice,

    // French Canadian voices
    /// Louise - French Canadian, Female, V3 Neural
    FrCaLouiseV3Voice,

    // Italian voices
    /// Francesca - Italian, Female, V3 Neural
    ItItFrancescaV3Voice,

    // Japanese voices
    /// Emi - Japanese, Female, V3 Neural
    JaJpEmiV3Voice,

    // Korean voices
    /// Hyunjun - Korean, Male, V3 Neural
    KoKrHyunjunV3Voice,
    /// Siwoo - Korean, Male, V3 Neural
    KoKrSiwooV3Voice,
    /// Youngmi - Korean, Female, V3 Neural
    KoKrYoungmiV3Voice,
    /// Yuna - Korean, Female, V3 Neural
    KoKrYunaV3Voice,

    // Dutch voices
    /// Emma - Dutch, Female, V3 Neural
    NlNlEmmaV3Voice,
    /// Liam - Dutch, Male, V3 Neural
    NlNlLiamV3Voice,

    // Portuguese (Brazilian) voices
    /// Isabela - Portuguese (Brazilian), Female, V3 Neural
    PtBrIsabelaV3Voice,

    // Chinese (Mandarin) voices
    /// LiNa - Chinese (Mandarin), Female, V3 Neural
    ZhCnLiNaV3Voice,
    /// WangWei - Chinese (Mandarin), Male, V3 Neural
    ZhCnWangWeiV3Voice,
    /// ZhangJing - Chinese (Mandarin), Female, V3 Neural
    ZhCnZhangJingV3Voice,

    /// Custom voice ID (for voices not in this enum)
    #[serde(rename = "custom")]
    Custom(String),
}

impl IbmVoice {
    /// Get the voice identifier string for the API.
    pub fn as_str(&self) -> &str {
        match self {
            // US English
            Self::EnUsAllisonV3Voice => "en-US_AllisonV3Voice",
            Self::EnUsEmilyV3Voice => "en-US_EmilyV3Voice",
            Self::EnUsHenryV3Voice => "en-US_HenryV3Voice",
            Self::EnUsKevinV3Voice => "en-US_KevinV3Voice",
            Self::EnUsLisaV3Voice => "en-US_LisaV3Voice",
            Self::EnUsMichaelV3Voice => "en-US_MichaelV3Voice",
            Self::EnUsOliviaV3Voice => "en-US_OliviaV3Voice",
            // UK English
            Self::EnGbCharlotteV3Voice => "en-GB_CharlotteV3Voice",
            Self::EnGbJamesV3Voice => "en-GB_JamesV3Voice",
            Self::EnGbKateV3Voice => "en-GB_KateV3Voice",
            // Australian English
            Self::EnAuCraigV3Voice => "en-AU_CraigV3Voice",
            Self::EnAuMadisonV3Voice => "en-AU_MadisonV3Voice",
            // German
            Self::DeDeBirgitV3Voice => "de-DE_BirgitV3Voice",
            Self::DeDeDieterV3Voice => "de-DE_DieterV3Voice",
            Self::DeDeErikaV3Voice => "de-DE_ErikaV3Voice",
            // Spanish (Castilian)
            Self::EsEsEnriqueV3Voice => "es-ES_EnriqueV3Voice",
            Self::EsEsLauraV3Voice => "es-ES_LauraV3Voice",
            // Spanish (Latin American)
            Self::EsLaSofiaV3Voice => "es-LA_SofiaV3Voice",
            // Spanish (North American)
            Self::EsUsSofiaV3Voice => "es-US_SofiaV3Voice",
            // French
            Self::FrFrNicolasV3Voice => "fr-FR_NicolasV3Voice",
            Self::FrFrReneeV3Voice => "fr-FR_ReneeV3Voice",
            // French Canadian
            Self::FrCaLouiseV3Voice => "fr-CA_LouiseV3Voice",
            // Italian
            Self::ItItFrancescaV3Voice => "it-IT_FrancescaV3Voice",
            // Japanese
            Self::JaJpEmiV3Voice => "ja-JP_EmiV3Voice",
            // Korean
            Self::KoKrHyunjunV3Voice => "ko-KR_HyunjunV3Voice",
            Self::KoKrSiwooV3Voice => "ko-KR_SiwooV3Voice",
            Self::KoKrYoungmiV3Voice => "ko-KR_YoungmiV3Voice",
            Self::KoKrYunaV3Voice => "ko-KR_YunaV3Voice",
            // Dutch
            Self::NlNlEmmaV3Voice => "nl-NL_EmmaV3Voice",
            Self::NlNlLiamV3Voice => "nl-NL_LiamV3Voice",
            // Portuguese (Brazilian)
            Self::PtBrIsabelaV3Voice => "pt-BR_IsabelaV3Voice",
            // Chinese (Mandarin)
            Self::ZhCnLiNaV3Voice => "zh-CN_LiNaVoice",
            Self::ZhCnWangWeiV3Voice => "zh-CN_WangWeiVoice",
            Self::ZhCnZhangJingV3Voice => "zh-CN_ZhangJingVoice",
            // Custom
            Self::Custom(id) => id,
        }
    }

    /// Get the language code for this voice.
    pub fn language_code(&self) -> &'static str {
        match self {
            // US English
            Self::EnUsAllisonV3Voice
            | Self::EnUsEmilyV3Voice
            | Self::EnUsHenryV3Voice
            | Self::EnUsKevinV3Voice
            | Self::EnUsLisaV3Voice
            | Self::EnUsMichaelV3Voice
            | Self::EnUsOliviaV3Voice => "en-US",
            // UK English
            Self::EnGbCharlotteV3Voice | Self::EnGbJamesV3Voice | Self::EnGbKateV3Voice => "en-GB",
            // Australian English
            Self::EnAuCraigV3Voice | Self::EnAuMadisonV3Voice => "en-AU",
            // German
            Self::DeDeBirgitV3Voice | Self::DeDeDieterV3Voice | Self::DeDeErikaV3Voice => "de-DE",
            // Spanish (Castilian)
            Self::EsEsEnriqueV3Voice | Self::EsEsLauraV3Voice => "es-ES",
            // Spanish (Latin American)
            Self::EsLaSofiaV3Voice => "es-LA",
            // Spanish (North American)
            Self::EsUsSofiaV3Voice => "es-US",
            // French
            Self::FrFrNicolasV3Voice | Self::FrFrReneeV3Voice => "fr-FR",
            // French Canadian
            Self::FrCaLouiseV3Voice => "fr-CA",
            // Italian
            Self::ItItFrancescaV3Voice => "it-IT",
            // Japanese
            Self::JaJpEmiV3Voice => "ja-JP",
            // Korean
            Self::KoKrHyunjunV3Voice
            | Self::KoKrSiwooV3Voice
            | Self::KoKrYoungmiV3Voice
            | Self::KoKrYunaV3Voice => "ko-KR",
            // Dutch
            Self::NlNlEmmaV3Voice | Self::NlNlLiamV3Voice => "nl-NL",
            // Portuguese (Brazilian)
            Self::PtBrIsabelaV3Voice => "pt-BR",
            // Chinese (Mandarin)
            Self::ZhCnLiNaV3Voice | Self::ZhCnWangWeiV3Voice | Self::ZhCnZhangJingV3Voice => {
                "zh-CN"
            }
            // Custom - default to US English
            Self::Custom(_) => "en-US",
        }
    }

    /// Parse from string, with fallback to Custom voice if not recognized.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            // US English
            "en-US_AllisonV3Voice" | "AllisonV3" | "allison" => Self::EnUsAllisonV3Voice,
            "en-US_EmilyV3Voice" | "EmilyV3" | "emily" => Self::EnUsEmilyV3Voice,
            "en-US_HenryV3Voice" | "HenryV3" | "henry" => Self::EnUsHenryV3Voice,
            "en-US_KevinV3Voice" | "KevinV3" | "kevin" => Self::EnUsKevinV3Voice,
            "en-US_LisaV3Voice" | "LisaV3" | "lisa" => Self::EnUsLisaV3Voice,
            "en-US_MichaelV3Voice" | "MichaelV3" | "michael" => Self::EnUsMichaelV3Voice,
            "en-US_OliviaV3Voice" | "OliviaV3" | "olivia" => Self::EnUsOliviaV3Voice,
            // UK English
            "en-GB_CharlotteV3Voice" | "CharlotteV3" | "charlotte" => Self::EnGbCharlotteV3Voice,
            "en-GB_JamesV3Voice" | "JamesV3" | "james" => Self::EnGbJamesV3Voice,
            "en-GB_KateV3Voice" | "KateV3" | "kate" => Self::EnGbKateV3Voice,
            // Australian English
            "en-AU_CraigV3Voice" | "CraigV3" | "craig" => Self::EnAuCraigV3Voice,
            "en-AU_MadisonV3Voice" | "MadisonV3" | "madison" => Self::EnAuMadisonV3Voice,
            // German
            "de-DE_BirgitV3Voice" | "BirgitV3" | "birgit" => Self::DeDeBirgitV3Voice,
            "de-DE_DieterV3Voice" | "DieterV3" | "dieter" => Self::DeDeDieterV3Voice,
            "de-DE_ErikaV3Voice" | "ErikaV3" | "erika" => Self::DeDeErikaV3Voice,
            // Spanish (Castilian)
            "es-ES_EnriqueV3Voice" | "EnriqueV3" | "enrique" => Self::EsEsEnriqueV3Voice,
            "es-ES_LauraV3Voice" | "LauraV3" | "laura" => Self::EsEsLauraV3Voice,
            // Spanish (Latin American)
            "es-LA_SofiaV3Voice" => Self::EsLaSofiaV3Voice,
            // Spanish (North American)
            "es-US_SofiaV3Voice" | "SofiaV3" | "sofia" => Self::EsUsSofiaV3Voice,
            // French
            "fr-FR_NicolasV3Voice" | "NicolasV3" | "nicolas" => Self::FrFrNicolasV3Voice,
            "fr-FR_ReneeV3Voice" | "ReneeV3" | "renee" => Self::FrFrReneeV3Voice,
            // French Canadian
            "fr-CA_LouiseV3Voice" | "LouiseV3" | "louise" => Self::FrCaLouiseV3Voice,
            // Italian
            "it-IT_FrancescaV3Voice" | "FrancescaV3" | "francesca" => Self::ItItFrancescaV3Voice,
            // Japanese
            "ja-JP_EmiV3Voice" | "EmiV3" | "emi" => Self::JaJpEmiV3Voice,
            // Korean
            "ko-KR_HyunjunV3Voice" | "HyunjunV3" | "hyunjun" => Self::KoKrHyunjunV3Voice,
            "ko-KR_SiwooV3Voice" | "SiwooV3" | "siwoo" => Self::KoKrSiwooV3Voice,
            "ko-KR_YoungmiV3Voice" | "YoungmiV3" | "youngmi" => Self::KoKrYoungmiV3Voice,
            "ko-KR_YunaV3Voice" | "YunaV3" | "yuna" => Self::KoKrYunaV3Voice,
            // Dutch
            "nl-NL_EmmaV3Voice" | "EmmaV3" | "emma" => Self::NlNlEmmaV3Voice,
            "nl-NL_LiamV3Voice" | "LiamV3" | "liam" => Self::NlNlLiamV3Voice,
            // Portuguese (Brazilian)
            "pt-BR_IsabelaV3Voice" | "IsabelaV3" | "isabela" => Self::PtBrIsabelaV3Voice,
            // Chinese (Mandarin)
            "zh-CN_LiNaVoice" | "LiNa" | "lina" => Self::ZhCnLiNaV3Voice,
            "zh-CN_WangWeiVoice" | "WangWei" | "wangwei" => Self::ZhCnWangWeiV3Voice,
            "zh-CN_ZhangJingVoice" | "ZhangJing" | "zhangjing" => Self::ZhCnZhangJingV3Voice,
            // Custom
            _ => Self::Custom(s.to_string()),
        }
    }

    /// Get all voices for a specific language.
    pub fn voices_for_language(language: &str) -> Vec<IbmVoice> {
        match language.to_lowercase().as_str() {
            "en-us" | "en_us" => vec![
                Self::EnUsAllisonV3Voice,
                Self::EnUsEmilyV3Voice,
                Self::EnUsHenryV3Voice,
                Self::EnUsKevinV3Voice,
                Self::EnUsLisaV3Voice,
                Self::EnUsMichaelV3Voice,
                Self::EnUsOliviaV3Voice,
            ],
            "en-gb" | "en_gb" => vec![
                Self::EnGbCharlotteV3Voice,
                Self::EnGbJamesV3Voice,
                Self::EnGbKateV3Voice,
            ],
            "en-au" | "en_au" => vec![Self::EnAuCraigV3Voice, Self::EnAuMadisonV3Voice],
            "de-de" | "de_de" => vec![
                Self::DeDeBirgitV3Voice,
                Self::DeDeDieterV3Voice,
                Self::DeDeErikaV3Voice,
            ],
            "es-es" | "es_es" => vec![Self::EsEsEnriqueV3Voice, Self::EsEsLauraV3Voice],
            "es-la" | "es_la" => vec![Self::EsLaSofiaV3Voice],
            "es-us" | "es_us" => vec![Self::EsUsSofiaV3Voice],
            "fr-fr" | "fr_fr" => vec![Self::FrFrNicolasV3Voice, Self::FrFrReneeV3Voice],
            "fr-ca" | "fr_ca" => vec![Self::FrCaLouiseV3Voice],
            "it-it" | "it_it" => vec![Self::ItItFrancescaV3Voice],
            "ja-jp" | "ja_jp" => vec![Self::JaJpEmiV3Voice],
            "ko-kr" | "ko_kr" => vec![
                Self::KoKrHyunjunV3Voice,
                Self::KoKrSiwooV3Voice,
                Self::KoKrYoungmiV3Voice,
                Self::KoKrYunaV3Voice,
            ],
            "nl-nl" | "nl_nl" => vec![Self::NlNlEmmaV3Voice, Self::NlNlLiamV3Voice],
            "pt-br" | "pt_br" => vec![Self::PtBrIsabelaV3Voice],
            "zh-cn" | "zh_cn" => vec![
                Self::ZhCnLiNaV3Voice,
                Self::ZhCnWangWeiV3Voice,
                Self::ZhCnZhangJingV3Voice,
            ],
            _ => vec![Self::EnUsAllisonV3Voice],
        }
    }
}

impl Default for IbmVoice {
    fn default() -> Self {
        Self::EnUsAllisonV3Voice
    }
}

impl std::fmt::Display for IbmVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Audio Output Format
// =============================================================================

/// Audio output formats supported by IBM Watson TTS.
///
/// # Format Details
///
/// - **Wav**: PCM in WAV container (default)
/// - **Mp3**: MPEG Layer-3 compressed audio
/// - **OggOpus**: Opus codec in OGG container (best quality/size)
/// - **OggVorbis**: Vorbis codec in OGG container
/// - **Flac**: Free Lossless Audio Codec
/// - **Webm**: WebM container with Opus
/// - **L16**: Raw 16-bit PCM (Linear16)
/// - **Mulaw**: μ-law companded audio (telephony)
/// - **Alaw**: A-law companded audio (telephony)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum IbmOutputFormat {
    /// WAV format (PCM in WAV container)
    #[default]
    #[serde(rename = "audio/wav")]
    Wav,
    /// MP3 format (compressed)
    #[serde(rename = "audio/mp3")]
    Mp3,
    /// OGG with Opus codec (best quality/size ratio)
    #[serde(rename = "audio/ogg;codecs=opus")]
    OggOpus,
    /// OGG with Vorbis codec
    #[serde(rename = "audio/ogg;codecs=vorbis")]
    OggVorbis,
    /// FLAC format (lossless compression)
    #[serde(rename = "audio/flac")]
    Flac,
    /// WebM with Opus codec
    #[serde(rename = "audio/webm;codecs=opus")]
    Webm,
    /// Raw 16-bit PCM (Linear16)
    #[serde(rename = "audio/l16")]
    L16,
    /// μ-law companded audio (8kHz telephony)
    #[serde(rename = "audio/mulaw")]
    Mulaw,
    /// A-law companded audio (8kHz telephony)
    #[serde(rename = "audio/alaw")]
    Alaw,
    /// Basic audio format (μ-law, 8kHz)
    #[serde(rename = "audio/basic")]
    Basic,
}

impl IbmOutputFormat {
    /// Get the Accept header value for this format.
    pub fn accept_header(&self, sample_rate: Option<u32>) -> String {
        match self {
            Self::Wav => {
                if let Some(rate) = sample_rate {
                    format!("audio/wav;rate={}", rate)
                } else {
                    "audio/wav".to_string()
                }
            }
            Self::Mp3 => "audio/mp3".to_string(),
            Self::OggOpus => {
                if let Some(rate) = sample_rate {
                    format!("audio/ogg;codecs=opus;rate={}", rate)
                } else {
                    "audio/ogg;codecs=opus".to_string()
                }
            }
            Self::OggVorbis => "audio/ogg;codecs=vorbis".to_string(),
            Self::Flac => "audio/flac".to_string(),
            Self::Webm => {
                if let Some(rate) = sample_rate {
                    format!("audio/webm;codecs=opus;rate={}", rate)
                } else {
                    "audio/webm;codecs=opus".to_string()
                }
            }
            Self::L16 => {
                let rate = sample_rate.unwrap_or(DEFAULT_SAMPLE_RATE);
                format!("audio/l16;rate={}", rate)
            }
            Self::Mulaw => {
                let rate = sample_rate.unwrap_or(8000);
                format!("audio/mulaw;rate={}", rate)
            }
            Self::Alaw => {
                let rate = sample_rate.unwrap_or(8000);
                format!("audio/alaw;rate={}", rate)
            }
            Self::Basic => "audio/basic".to_string(),
        }
    }

    /// Get the file extension for this format.
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::OggOpus | Self::OggVorbis => "ogg",
            Self::Flac => "flac",
            Self::Webm => "webm",
            Self::L16 => "raw",
            Self::Mulaw | Self::Alaw | Self::Basic => "raw",
        }
    }

    /// Get the default sample rate for this format.
    pub fn default_sample_rate(&self) -> u32 {
        match self {
            Self::Mulaw | Self::Alaw | Self::Basic => 8000,
            _ => DEFAULT_SAMPLE_RATE,
        }
    }

    /// Check if this format requires a specific sample rate.
    pub fn requires_sample_rate(&self) -> bool {
        matches!(self, Self::L16 | Self::Mulaw | Self::Alaw)
    }

    /// Parse from string, with fallback to Wav.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "wav" | "audio/wav" => Self::Wav,
            "mp3" | "audio/mp3" | "mpeg" => Self::Mp3,
            "ogg-opus" | "opus" | "audio/ogg;codecs=opus" => Self::OggOpus,
            "ogg-vorbis" | "vorbis" | "audio/ogg;codecs=vorbis" => Self::OggVorbis,
            "flac" | "audio/flac" => Self::Flac,
            "webm" | "audio/webm" | "webm-opus" => Self::Webm,
            "l16" | "pcm" | "linear16" | "raw" | "audio/l16" => Self::L16,
            "mulaw" | "mu-law" | "ulaw" | "audio/mulaw" => Self::Mulaw,
            "alaw" | "a-law" | "audio/alaw" => Self::Alaw,
            "basic" | "audio/basic" => Self::Basic,
            _ => Self::default(),
        }
    }

    /// Get the format name as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::OggOpus => "ogg-opus",
            Self::OggVorbis => "ogg-vorbis",
            Self::Flac => "flac",
            Self::Webm => "webm",
            Self::L16 => "l16",
            Self::Mulaw => "mulaw",
            Self::Alaw => "alaw",
            Self::Basic => "basic",
        }
    }
}

impl std::fmt::Display for IbmOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// IBM Watson TTS Configuration
// =============================================================================

/// Configuration for IBM Watson Text-to-Speech.
///
/// This configuration extends the base TTS configuration with
/// IBM Watson-specific options.
///
/// # Authentication
///
/// IBM Watson TTS uses IAM (Identity and Access Management) tokens for
/// authentication. You need:
/// 1. API key from IBM Cloud
/// 2. Service instance ID
/// 3. Region where your service is deployed
///
/// The API key is stored in `base.api_key` and exchanged for an IAM bearer
/// token at runtime.
///
/// # Example
///
/// ```rust,ignore
/// use waav_gateway::core::tts::ibm_watson::{IbmWatsonTTSConfig, IbmVoice, IbmOutputFormat};
///
/// let config = IbmWatsonTTSConfig {
///     instance_id: "your-instance-id".to_string(),
///     voice: IbmVoice::EnUsAllisonV3Voice,
///     output_format: IbmOutputFormat::OggOpus,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IbmWatsonTTSConfig {
    /// Base TTS configuration
    #[serde(flatten)]
    pub base: TTSConfig,

    /// IBM Cloud region for the service.
    #[serde(default)]
    pub region: IbmRegion,

    /// Service instance ID (required).
    /// Found in IBM Cloud service credentials.
    pub instance_id: String,

    /// Voice to use for synthesis.
    #[serde(default)]
    pub voice: IbmVoice,

    /// Audio output format.
    #[serde(default)]
    pub output_format: IbmOutputFormat,

    /// Speaking rate adjustment (-100% to +100%).
    /// 0 = normal, 50 = 50% faster, -50 = 50% slower.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_percentage: Option<i32>,

    /// Pitch adjustment (-100% to +100%).
    /// 0 = normal, 50 = 50% higher, -50 = 50% lower.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pitch_percentage: Option<i32>,

    /// Custom pronunciation dictionaries (customization IDs).
    #[serde(default)]
    pub customization_ids: Vec<String>,

    /// Enable spell out mode (spell words letter by letter).
    #[serde(default)]
    pub spell_out_mode: Option<String>,
}

impl Default for IbmWatsonTTSConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig {
                provider: "ibm-watson".to_string(),
                api_key: String::new(),
                voice_id: Some(DEFAULT_VOICE.to_string()),
                model: "v3".to_string(),
                speaking_rate: Some(1.0),
                audio_format: Some("wav".to_string()),
                sample_rate: Some(DEFAULT_SAMPLE_RATE),
                connection_timeout: Some(30),
                request_timeout: Some(60),
                pronunciations: Vec::new(),
                request_pool_size: Some(4),
            },
            region: IbmRegion::default(),
            instance_id: String::new(),
            voice: IbmVoice::default(),
            output_format: IbmOutputFormat::default(),
            rate_percentage: None,
            pitch_percentage: None,
            customization_ids: Vec::new(),
            spell_out_mode: None,
        }
    }
}

impl IbmWatsonTTSConfig {
    /// Create a new configuration with the given voice.
    pub fn with_voice(voice: IbmVoice) -> Self {
        let mut config = Self::default();
        config.base.voice_id = Some(voice.as_str().to_string());
        config.voice = voice;
        config
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Instance ID is required
        if self.instance_id.is_empty() {
            return Err("Instance ID is required".to_string());
        }

        // API key is required
        if self.base.api_key.is_empty() {
            return Err("API key is required".to_string());
        }

        // Validate rate percentage if set
        if let Some(rate) = self.rate_percentage {
            if !(-100..=100).contains(&rate) {
                return Err(format!(
                    "Rate percentage must be between -100 and 100, got {}",
                    rate
                ));
            }
        }

        // Validate pitch percentage if set
        if let Some(pitch) = self.pitch_percentage {
            if !(-100..=100).contains(&pitch) {
                return Err(format!(
                    "Pitch percentage must be between -100 and 100, got {}",
                    pitch
                ));
            }
        }

        // Validate customization count (max 2)
        if self.customization_ids.len() > 2 {
            return Err("Maximum 2 customization IDs allowed per request".to_string());
        }

        Ok(())
    }

    /// Build the synthesis URL for the TTS API.
    pub fn build_synthesis_url(&self) -> String {
        format!(
            "https://{}/instances/{}/v1/synthesize",
            self.region.tts_hostname(),
            self.instance_id
        )
    }

    /// Build query parameters for the synthesis request.
    pub fn build_query_params(&self) -> Vec<(&'static str, String)> {
        let mut params = vec![("voice".to_string(), self.voice.as_str().to_string())];

        // Add customization IDs if present
        for customization_id in &self.customization_ids {
            params.push(("customization_id".to_string(), customization_id.clone()));
        }

        // Add spell out mode if set
        if let Some(ref mode) = self.spell_out_mode {
            params.push(("spell_out_mode".to_string(), mode.clone()));
        }

        // Return as static str references with values
        params
            .into_iter()
            .map(|(k, v)| {
                let key: &'static str = match k.as_str() {
                    "voice" => "voice",
                    "customization_id" => "customization_id",
                    "spell_out_mode" => "spell_out_mode",
                    _ => "voice",
                };
                (key, v)
            })
            .collect()
    }

    /// Get the Accept header for the request.
    pub fn accept_header(&self) -> String {
        self.output_format.accept_header(self.base.sample_rate)
    }

    /// Get the effective sample rate.
    pub fn effective_sample_rate(&self) -> u32 {
        self.base
            .sample_rate
            .unwrap_or_else(|| self.output_format.default_sample_rate())
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_voice_names() {
        assert_eq!(
            IbmVoice::EnUsAllisonV3Voice.as_str(),
            "en-US_AllisonV3Voice"
        );
        assert_eq!(
            IbmVoice::EnGbCharlotteV3Voice.as_str(),
            "en-GB_CharlotteV3Voice"
        );
        assert_eq!(
            IbmVoice::Custom("custom-voice-123".to_string()).as_str(),
            "custom-voice-123"
        );
    }

    #[test]
    fn test_voice_language_codes() {
        assert_eq!(IbmVoice::EnUsAllisonV3Voice.language_code(), "en-US");
        assert_eq!(IbmVoice::DeDeBirgitV3Voice.language_code(), "de-DE");
        assert_eq!(IbmVoice::JaJpEmiV3Voice.language_code(), "ja-JP");
    }

    #[test]
    fn test_voice_parsing() {
        assert_eq!(
            IbmVoice::from_str_or_default("en-US_AllisonV3Voice"),
            IbmVoice::EnUsAllisonV3Voice
        );
        assert_eq!(
            IbmVoice::from_str_or_default("allison"),
            IbmVoice::EnUsAllisonV3Voice
        );
        assert!(matches!(
            IbmVoice::from_str_or_default("unknown-voice"),
            IbmVoice::Custom(_)
        ));
    }

    #[test]
    fn test_voices_for_language() {
        let us_voices = IbmVoice::voices_for_language("en-US");
        assert!(us_voices.contains(&IbmVoice::EnUsAllisonV3Voice));
        assert!(us_voices.contains(&IbmVoice::EnUsMichaelV3Voice));

        let de_voices = IbmVoice::voices_for_language("de-DE");
        assert!(de_voices.contains(&IbmVoice::DeDeBirgitV3Voice));
    }

    #[test]
    fn test_output_format() {
        assert_eq!(IbmOutputFormat::Wav.as_str(), "wav");
        assert_eq!(IbmOutputFormat::OggOpus.as_str(), "ogg-opus");
        assert_eq!(IbmOutputFormat::L16.extension(), "raw");
        assert_eq!(IbmOutputFormat::Mp3.extension(), "mp3");
    }

    #[test]
    fn test_output_format_accept_header() {
        assert_eq!(
            IbmOutputFormat::Wav.accept_header(Some(22050)),
            "audio/wav;rate=22050"
        );
        assert_eq!(IbmOutputFormat::Mp3.accept_header(None), "audio/mp3");
        assert_eq!(
            IbmOutputFormat::L16.accept_header(Some(16000)),
            "audio/l16;rate=16000"
        );
        assert_eq!(
            IbmOutputFormat::Mulaw.accept_header(None),
            "audio/mulaw;rate=8000"
        );
    }

    #[test]
    fn test_output_format_parsing() {
        assert_eq!(
            IbmOutputFormat::from_str_or_default("wav"),
            IbmOutputFormat::Wav
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("opus"),
            IbmOutputFormat::OggOpus
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("pcm"),
            IbmOutputFormat::L16
        );
        assert_eq!(
            IbmOutputFormat::from_str_or_default("unknown"),
            IbmOutputFormat::Wav
        );
    }

    #[test]
    fn test_config_default() {
        let config = IbmWatsonTTSConfig::default();
        assert_eq!(config.voice, IbmVoice::EnUsAllisonV3Voice);
        assert_eq!(config.output_format, IbmOutputFormat::Wav);
        assert_eq!(config.region, IbmRegion::UsSouth);
    }

    #[test]
    fn test_config_with_voice() {
        let config = IbmWatsonTTSConfig::with_voice(IbmVoice::EnGbCharlotteV3Voice);
        assert_eq!(config.voice, IbmVoice::EnGbCharlotteV3Voice);
        assert_eq!(
            config.base.voice_id,
            Some("en-GB_CharlotteV3Voice".to_string())
        );
    }

    #[test]
    fn test_config_validation_valid() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_missing_instance_id() {
        let mut config = IbmWatsonTTSConfig::default();
        config.base.api_key = "test-api-key".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Instance ID"));
    }

    #[test]
    fn test_config_validation_missing_api_key() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn test_config_validation_invalid_rate() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();
        config.rate_percentage = Some(150);

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Rate percentage"));
    }

    #[test]
    fn test_config_validation_too_many_customizations() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance".to_string();
        config.base.api_key = "test-api-key".to_string();
        config.customization_ids = vec![
            "cust1".to_string(),
            "cust2".to_string(),
            "cust3".to_string(),
        ];

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("customization"));
    }

    #[test]
    fn test_build_synthesis_url() {
        let mut config = IbmWatsonTTSConfig::default();
        config.instance_id = "test-instance-123".to_string();
        config.region = IbmRegion::EuDe;

        let url = config.build_synthesis_url();
        assert!(url.contains("api.eu-de.text-to-speech.watson.cloud.ibm.com"));
        assert!(url.contains("instances/test-instance-123"));
        assert!(url.contains("/v1/synthesize"));
    }

    #[test]
    fn test_build_query_params() {
        let config = IbmWatsonTTSConfig::default();
        let params = config.build_query_params();

        assert!(params.iter().any(|(k, _)| *k == "voice"));
    }

    #[test]
    fn test_accept_header() {
        let mut config = IbmWatsonTTSConfig::default();
        config.output_format = IbmOutputFormat::OggOpus;
        config.base.sample_rate = Some(24000);

        let header = config.accept_header();
        assert!(header.contains("audio/ogg;codecs=opus"));
        assert!(header.contains("rate=24000"));
    }

    #[test]
    fn test_effective_sample_rate() {
        let config = IbmWatsonTTSConfig::default();
        assert_eq!(config.effective_sample_rate(), DEFAULT_SAMPLE_RATE);

        let mut config_custom = IbmWatsonTTSConfig::default();
        config_custom.base.sample_rate = Some(16000);
        assert_eq!(config_custom.effective_sample_rate(), 16000);
    }
}
