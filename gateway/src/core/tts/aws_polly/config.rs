//! Configuration types for Amazon Polly TTS API.
//!
//! This module defines configuration options for Amazon Polly's
//! text-to-speech service. The configuration supports:
//! - AWS authentication (access key/secret or IAM roles)
//! - Voice selection (60+ voices across 30+ languages)
//! - Engine selection (standard, neural, long-form, generative — or none, Polly's default)
//! - Audio output formats (pcm, mp3, ogg_vorbis, ogg_opus)
//!
//! # Example
//!
//! ```rust,ignore
//! use waav_gateway::core::tts::aws_polly::{AwsPollyTTSConfig, PollyVoice, PollyEngine};
//!
//! let config = AwsPollyTTSConfig {
//!     voice: PollyVoice::Joanna,
//!     engine: Some(PollyEngine::Neural),
//!     output_format: PollyOutputFormat::Pcm,
//!     sample_rate: Some(16000),
//!     ..Default::default()
//! };
//! ```

use serde::{Deserialize, Serialize};

use crate::core::stt::aws_transcribe::AwsRegion;
use crate::core::tts::base::TTSConfig;

fn validate_aws_polly_tts_endpoint(source: &str, endpoint: &str) -> Result<(), String> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }

    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES)
        .map_err(|msg| format!("{source} rejected (SSRF protection): {msg}"))
}

// =============================================================================
// Polly Engine
// =============================================================================

/// Amazon Polly synthesis engine options.
///
/// Different engines provide different quality/latency trade-offs:
/// - **Standard**: Basic TTS, lowest latency, good for simple use cases
/// - **Neural**: High-quality neural voices, recommended for most applications
/// - **LongForm**: Optimized for longer content like audiobooks
/// - **Generative**: Latest generative AI voices with best quality
///
/// There is deliberately no `Default`: `Engine` is optional on `SynthesizeSpeech` and Polly
/// applies `standard` when it is absent, so "no engine chosen" is `Option::None` and is sent as
/// nothing (see [`PollyEngine::from_model`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PollyEngine {
    /// Standard TTS engine
    #[serde(rename = "standard")]
    Standard,
    /// Neural TTS engine (recommended)
    #[serde(rename = "neural")]
    Neural,
    /// Long-form TTS engine (for audiobooks, articles)
    #[serde(rename = "long-form")]
    LongForm,
    /// Generative AI TTS engine (highest quality)
    #[serde(rename = "generative")]
    Generative,
}

impl PollyEngine {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Neural => "neural",
            Self::LongForm => "long-form",
            Self::Generative => "generative",
        }
    }

    /// The engine a deployment's `model` names, if it names one.
    ///
    /// Vendor contract (SynthesizeSpeech `Engine`): optional; when absent Polly selects
    /// `standard`. So an empty model sends NO engine — it used to send `neural`, an engine nobody
    /// chose. The four catalog names select their engine, case-insensitively (`longform` /
    /// `long_form` are accepted spellings of `long-form`). Any other model is an error naming it:
    /// it used to become `neural` silently, so a typo, or another vendor's model id, synthesised
    /// with an engine the caller never asked for — and billed at neural rates.
    pub fn from_model(model: &str) -> Result<Option<Self>, String> {
        let model = model.trim();
        if model.is_empty() {
            return Ok(None);
        }
        match model.to_ascii_lowercase().as_str() {
            "standard" => Ok(Some(Self::Standard)),
            "neural" => Ok(Some(Self::Neural)),
            "long-form" | "longform" | "long_form" => Ok(Some(Self::LongForm)),
            "generative" => Ok(Some(Self::Generative)),
            _ => Err(format!(
                "Amazon Polly has no engine {model:?}: the model must be one of standard, \
                 neural, long-form, generative, or empty for Polly's default (standard)"
            )),
        }
    }
}

impl std::fmt::Display for PollyEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Polly Output Format
// =============================================================================

/// Audio output formats supported by Amazon Polly.
///
/// # Format Details
///
/// - **Pcm**: Raw uncompressed audio, lowest latency, 16-bit signed little-endian mono (default:
///   WaaV's canonical `linear16`; Polly itself has no default, `OutputFormat` is required)
/// - **Mp3**: Compressed audio, good for streaming
/// - **OggVorbis**: Open-source compression, good quality/size ratio
/// - **OggOpus**: Opus in an Ogg container (what OpenAI calls `opus`); 48 kHz only
///
/// Polly's `mulaw`/`alaw` (8 kHz telephony) and the `json` speech-marks stream are not modelled
/// here; speech marks switch the request to `json` in the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PollyOutputFormat {
    /// MP3 format (compressed)
    #[serde(rename = "mp3")]
    Mp3,
    /// OGG Vorbis format (compressed)
    #[serde(rename = "ogg_vorbis")]
    OggVorbis,
    /// Ogg Opus format (compressed, 48 kHz only)
    #[serde(rename = "ogg_opus")]
    OggOpus,
    /// PCM format (uncompressed, 16-bit signed little-endian)
    #[default]
    #[serde(rename = "pcm")]
    Pcm,
}

/// What `from_requested` accepts, for its error message.
const REQUESTABLE_FORMATS: &str =
    "linear16 (or pcm), wav (sent as pcm), mp3, opus (ogg_opus), ogg_vorbis";

impl PollyOutputFormat {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Mp3 => "mp3",
            Self::OggVorbis => "ogg_vorbis",
            Self::OggOpus => "ogg_opus",
            Self::Pcm => "pcm",
        }
    }

    /// Get the MIME type for this format.
    #[inline]
    pub fn mime_type(&self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::OggVorbis | Self::OggOpus => "audio/ogg",
            Self::Pcm => "audio/pcm",
        }
    }

    /// Polly's own sample rate when no `SampleRate` is sent, for the engine that will run.
    ///
    /// Vendor contract (SynthesizeSpeech `SampleRate`): pcm defaults to 16000 and ogg_opus is
    /// 48000 only. mp3/ogg_vorbis depend on the ENGINE: 22050 for `standard` — also what runs when
    /// no engine is sent — and 24000 for neural, long-form and generative.
    pub fn default_sample_rate_for(&self, engine: Option<PollyEngine>) -> u32 {
        match self {
            Self::Pcm => 16000,
            Self::OggOpus => 48000,
            Self::Mp3 | Self::OggVorbis => match engine {
                None | Some(PollyEngine::Standard) => 22050,
                Some(PollyEngine::Neural | PollyEngine::LongForm | PollyEngine::Generative) => {
                    24000
                }
            },
        }
    }

    /// Polly's default sample rate for this format with no engine sent (the `standard` engine).
    #[inline]
    pub fn default_sample_rate(&self) -> u32 {
        self.default_sample_rate_for(None)
    }

    /// The `SampleRate` values Polly accepts for this format.
    pub fn supported_sample_rates(&self) -> &'static [u32] {
        match self {
            Self::Mp3 | Self::OggVorbis => &[8000, 16000, 22050, 24000, 44100, 48000],
            Self::OggOpus => &[48000],
            Self::Pcm => &[8000, 16000],
        }
    }

    /// The Polly output format for a WaaV / OpenAI format name — explicitly, never by fallback.
    ///
    /// `linear16`, `pcm` and `wav` are Polly's `pcm` (raw 16-bit mono; for `wav` the OpenAI route
    /// adds the RIFF header at the rate the provider reports). `mp3` is `mp3`, `opus` is Polly's
    /// `ogg_opus` (OpenAI's `opus` is Ogg-contained too) and `ogg_vorbis` is `ogg_vorbis`. WaaV's
    /// other spellings of 16-bit PCM (`pcm16`, `pcm_s16le`) and the long-standing aliases `mpeg`,
    /// `raw`, `vorbis` and `ogg` (Polly's original Ogg output was Vorbis) still resolve. Anything else — `aac`, `flac`, `mulaw`, a typo — is an error naming
    /// the format: every unrecognised name used to become mp3, and `from_standard` ignored the
    /// requested format altogether, so an mp3 request got raw PCM served as `audio/mpeg`.
    pub fn from_requested(format: &str) -> Result<Self, String> {
        match format.trim().to_ascii_lowercase().as_str() {
            "linear16" | "pcm" | "pcm16" | "pcm_s16le" | "wav" | "raw" => Ok(Self::Pcm),
            "mp3" | "mpeg" => Ok(Self::Mp3),
            "opus" | "ogg_opus" | "ogg-opus" => Ok(Self::OggOpus),
            "ogg_vorbis" | "vorbis" | "ogg" => Ok(Self::OggVorbis),
            other => Err(format!(
                "Amazon Polly cannot produce {other:?} audio; supported formats: \
                 {REQUESTABLE_FORMATS}"
            )),
        }
    }

    /// [`Self::from_requested`] for an optional `audio_format`: unset or blank is `pcm`, WaaV's
    /// canonical `linear16` (Polly has no default of its own — `OutputFormat` is required).
    pub fn from_audio_format(format: Option<&str>) -> Result<Self, String> {
        match format.map(str::trim).filter(|f| !f.is_empty()) {
            Some(f) => Self::from_requested(f),
            None => Ok(Self::Pcm),
        }
    }

    /// The `SampleRate` to SEND for a requested rate — `None` means "send none".
    ///
    /// * **pcm**: Polly accepts only 8000 and 16000. The OpenAI route asks for 24000 (OpenAI's
    ///   `pcm` is 24 kHz by definition), and passing that through failed every pcm request in
    ///   validation. A requested 8000/16000 is kept; anything else — or nothing — sends 16000,
    ///   explicitly, so the rate the provider REPORTS (and the route writes into the WAV header /
    ///   `x-sample-rate`) is exactly the rate Polly produced.
    /// * **ogg_opus**: 48000 is the only valid value, so it is always sent.
    /// * **mp3 / ogg_vorbis**: a supported requested rate is sent; otherwise none is, and Polly
    ///   applies its engine default (the container carries its own rate either way).
    pub fn resolve_sample_rate(&self, requested: Option<u32>) -> Option<u32> {
        let supported = requested.filter(|r| self.supported_sample_rates().contains(r));
        match self {
            Self::Pcm => Some(supported.unwrap_or(16000)),
            Self::OggOpus => Some(48000),
            Self::Mp3 | Self::OggVorbis => supported,
        }
    }
}

impl std::fmt::Display for PollyOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Polly Voice
// =============================================================================

/// Popular Amazon Polly voices.
///
/// This is a subset of the 60+ voices available. For the complete list,
/// see: https://docs.aws.amazon.com/polly/latest/dg/voicelist.html
///
/// Neural voices are recommended for most applications.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PollyVoice {
    // US English Neural Voices
    /// Joanna - US English, Female, Neural (most popular)
    #[default]
    Joanna,
    /// Matthew - US English, Male, Neural
    Matthew,
    /// Salli - US English, Female, Neural
    Salli,
    /// Kendra - US English, Female, Neural
    Kendra,
    /// Kimberly - US English, Female, Neural
    Kimberly,
    /// Joey - US English, Male, Neural
    Joey,
    /// Ruth - US English, Female, Neural/Generative
    Ruth,
    /// Stephen - US English, Male, Neural
    Stephen,
    /// Kevin - US English, Male, Child, Neural
    Kevin,
    /// Ivy - US English, Female, Child, Neural
    Ivy,
    /// Justin - US English, Male, Child, Neural
    Justin,

    // UK English Neural Voices
    /// Amy - British English, Female, Neural
    Amy,
    /// Emma - British English, Female, Neural
    Emma,
    /// Brian - British English, Male, Neural
    Brian,
    /// Arthur - British English, Male, Neural
    Arthur,

    // Australian English Neural Voices
    /// Olivia - Australian English, Female, Neural
    Olivia,

    // Other Languages - Popular Neural Voices
    /// Léa - French, Female, Neural
    Lea,
    /// Hans - German, Male, Neural
    Hans,
    /// Vicki - German, Female, Neural
    Vicki,
    /// Mizuki - Japanese, Female, Neural
    Mizuki,
    /// Takumi - Japanese, Male, Neural
    Takumi,
    /// Seoyeon - Korean, Female, Neural
    Seoyeon,
    /// Zhiyu - Mandarin Chinese, Female, Neural
    Zhiyu,
    /// Camila - Portuguese (Brazilian), Female, Neural
    Camila,
    /// Vitoria - Portuguese (Brazilian), Female, Neural
    Vitoria,
    /// Lupe - Spanish (US), Female, Neural
    Lupe,
    /// Pedro - Spanish (US), Male, Neural
    Pedro,
    /// Lucia - Spanish (Castilian), Female, Neural
    Lucia,
    /// Enrique - Spanish (Castilian), Male, Neural
    Enrique,
    /// Mia - Spanish (Mexican), Female, Neural
    Mia,
    /// Bianca - Italian, Female, Neural
    Bianca,
    /// Adriano - Italian, Male, Neural
    Adriano,
    /// Ola - Polish, Female, Neural
    Ola,
    /// Kajal - Hindi, Female, Neural
    Kajal,
    /// Aria - New Zealand English, Female, Neural
    Aria,

    /// Custom voice ID (for voices not in this enum)
    #[serde(rename = "custom")]
    Custom(String),
}

impl PollyVoice {
    /// Convert to AWS voice ID string.
    pub fn as_str(&self) -> &str {
        match self {
            // US English
            Self::Joanna => "Joanna",
            Self::Matthew => "Matthew",
            Self::Salli => "Salli",
            Self::Kendra => "Kendra",
            Self::Kimberly => "Kimberly",
            Self::Joey => "Joey",
            Self::Ruth => "Ruth",
            Self::Stephen => "Stephen",
            Self::Kevin => "Kevin",
            Self::Ivy => "Ivy",
            Self::Justin => "Justin",
            // UK English
            Self::Amy => "Amy",
            Self::Emma => "Emma",
            Self::Brian => "Brian",
            Self::Arthur => "Arthur",
            // Australian
            Self::Olivia => "Olivia",
            // Other languages
            Self::Lea => "Léa",
            Self::Hans => "Hans",
            Self::Vicki => "Vicki",
            Self::Mizuki => "Mizuki",
            Self::Takumi => "Takumi",
            Self::Seoyeon => "Seoyeon",
            Self::Zhiyu => "Zhiyu",
            Self::Camila => "Camila",
            Self::Vitoria => "Vitoria",
            Self::Lupe => "Lupe",
            Self::Pedro => "Pedro",
            Self::Lucia => "Lucia",
            Self::Enrique => "Enrique",
            Self::Mia => "Mia",
            Self::Bianca => "Bianca",
            Self::Adriano => "Adriano",
            Self::Ola => "Ola",
            Self::Kajal => "Kajal",
            Self::Aria => "Aria",
            // Custom
            Self::Custom(id) => id,
        }
    }

    /// Get the default language code for this voice.
    pub fn language_code(&self) -> &'static str {
        match self {
            // US English
            Self::Joanna
            | Self::Matthew
            | Self::Salli
            | Self::Kendra
            | Self::Kimberly
            | Self::Joey
            | Self::Ruth
            | Self::Stephen
            | Self::Kevin
            | Self::Ivy
            | Self::Justin => "en-US",
            // UK English
            Self::Amy | Self::Emma | Self::Brian | Self::Arthur => "en-GB",
            // Australian
            Self::Olivia => "en-AU",
            // French
            Self::Lea => "fr-FR",
            // German
            Self::Hans | Self::Vicki => "de-DE",
            // Japanese
            Self::Mizuki | Self::Takumi => "ja-JP",
            // Korean
            Self::Seoyeon => "ko-KR",
            // Chinese
            Self::Zhiyu => "cmn-CN",
            // Portuguese
            Self::Camila | Self::Vitoria => "pt-BR",
            // Spanish
            Self::Lupe | Self::Pedro => "es-US",
            Self::Lucia | Self::Enrique => "es-ES",
            Self::Mia => "es-MX",
            // Italian
            Self::Bianca | Self::Adriano => "it-IT",
            // Polish
            Self::Ola => "pl-PL",
            // Hindi
            Self::Kajal => "hi-IN",
            // New Zealand
            Self::Aria => "en-NZ",
            // Custom - default to US English
            Self::Custom(_) => "en-US",
        }
    }

    /// Check if this voice supports the neural engine.
    pub fn supports_neural(&self) -> bool {
        // Most modern Polly voices support neural
        !matches!(self, Self::Custom(_))
    }

    /// Parse from string, with fallback to Custom voice if not recognized.
    ///
    /// Note: For unrecognized voices, preserves the original case to support
    /// custom voice IDs that may be case-sensitive.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "joanna" => Self::Joanna,
            "matthew" => Self::Matthew,
            "salli" => Self::Salli,
            "kendra" => Self::Kendra,
            "kimberly" => Self::Kimberly,
            "joey" => Self::Joey,
            "ruth" => Self::Ruth,
            "stephen" => Self::Stephen,
            "kevin" => Self::Kevin,
            "ivy" => Self::Ivy,
            "justin" => Self::Justin,
            "amy" => Self::Amy,
            "emma" => Self::Emma,
            "brian" => Self::Brian,
            "arthur" => Self::Arthur,
            "olivia" => Self::Olivia,
            "lea" | "léa" => Self::Lea,
            "hans" => Self::Hans,
            "vicki" => Self::Vicki,
            "mizuki" => Self::Mizuki,
            "takumi" => Self::Takumi,
            "seoyeon" => Self::Seoyeon,
            "zhiyu" => Self::Zhiyu,
            "camila" => Self::Camila,
            "vitoria" => Self::Vitoria,
            "lupe" => Self::Lupe,
            "pedro" => Self::Pedro,
            "lucia" => Self::Lucia,
            "enrique" => Self::Enrique,
            "mia" => Self::Mia,
            "bianca" => Self::Bianca,
            "adriano" => Self::Adriano,
            "ola" => Self::Ola,
            "kajal" => Self::Kajal,
            "aria" => Self::Aria,
            // Preserve original case for custom voices
            _ => Self::Custom(s.to_string()),
        }
    }

    /// Get all common voices for a language.
    pub fn voices_for_language(language: &str) -> Vec<PollyVoice> {
        match language.to_lowercase().as_str() {
            "en-us" | "en_us" => vec![
                Self::Joanna,
                Self::Matthew,
                Self::Salli,
                Self::Kendra,
                Self::Joey,
                Self::Ruth,
            ],
            "en-gb" | "en_gb" => vec![Self::Amy, Self::Emma, Self::Brian, Self::Arthur],
            "en-au" | "en_au" => vec![Self::Olivia],
            "de-de" | "de_de" => vec![Self::Hans, Self::Vicki],
            "fr-fr" | "fr_fr" => vec![Self::Lea],
            "ja-jp" | "ja_jp" => vec![Self::Mizuki, Self::Takumi],
            "es-us" | "es_us" => vec![Self::Lupe, Self::Pedro],
            _ => vec![Self::Joanna],
        }
    }
}

impl std::fmt::Display for PollyVoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Text Type
// =============================================================================

/// Input text type for Amazon Polly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TextType {
    /// Plain text input
    #[default]
    #[serde(rename = "text")]
    Text,
    /// SSML (Speech Synthesis Markup Language) input
    #[serde(rename = "ssml")]
    Ssml,
}

impl TextType {
    /// Convert to AWS API string.
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Ssml => "ssml",
        }
    }

    /// Parse from string.
    pub fn from_str_or_default(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "ssml" => Self::Ssml,
            _ => Self::Text,
        }
    }
}

impl std::fmt::Display for TextType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Maximum text length for SynthesizeSpeech API (characters).
pub const MAX_TEXT_LENGTH: usize = 3000;

/// Maximum total input length including SSML tags (characters).
pub const MAX_TOTAL_LENGTH: usize = 6000;

/// Configuration for Amazon Polly TTS.
///
/// This configuration extends the base TTS configuration with
/// Amazon Polly-specific options.
///
/// # Authentication
///
/// AWS credentials can be provided via:
/// 1. `aws_access_key_id` and `aws_secret_access_key` fields
/// 2. Environment variables: `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`
/// 3. IAM instance profile (for EC2/ECS/Lambda)
/// 4. AWS credentials file (`~/.aws/credentials`)
///
/// # Best Practices
///
/// - Use neural engine for best quality
/// - Use PCM format for lowest latency in real-time applications
/// - Keep text under 3000 characters per request
/// - Use SSML for fine-grained control over pronunciation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsPollyTTSConfig {
    /// Base TTS configuration
    #[serde(flatten)]
    pub base: TTSConfig,

    /// AWS region for the Polly service
    #[serde(default)]
    pub region: AwsRegion,

    /// AWS access key ID (optional if using IAM roles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_access_key_id: Option<String>,

    /// AWS secret access key (optional if using IAM roles)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_secret_access_key: Option<String>,

    /// AWS session token for temporary credentials (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aws_session_token: Option<String>,

    /// Voice to use for synthesis
    #[serde(default)]
    pub voice: PollyVoice,

    /// TTS engine to use. `None` sends no `Engine` and Polly applies `standard`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<PollyEngine>,

    /// Audio output format
    #[serde(default)]
    pub output_format: PollyOutputFormat,

    /// Input text type (plain text or SSML)
    #[serde(default)]
    pub text_type: TextType,

    /// Language code override (optional, defaults to voice's language)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_code: Option<String>,

    /// Custom lexicon names to apply
    #[serde(default)]
    pub lexicon_names: Vec<String>,

    /// Requested speech-mark types (`word` / `sentence` / `viseme` / `ssml`). When non-empty,
    /// Polly's `SynthesizeSpeech` returns a JSON metadata stream of timing/viseme/SSML marks
    /// instead of audio (AWS forbids audio + marks in one request — they are a separate `Json`
    /// output-format call). Canonical lowercase names; the provider maps them to the SDK
    /// `SpeechMarkType` enum on the wire.
    /// Ref: <https://docs.aws.amazon.com/polly/latest/dg/API_SynthesizeSpeech.html#polly-SynthesizeSpeech-request-SpeechMarkTypes>
    #[serde(default)]
    pub speech_mark_types: Vec<String>,

    /// Override the AWS Polly endpoint base URL (e.g. a localhost mock for e2e tests).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_override: Option<String>,
}

impl Default for AwsPollyTTSConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig {
                provider: "aws-polly".to_string(),
                api_key: String::new(), // Not used, AWS uses access keys
                voice_id: Some("Joanna".to_string()),
                model: "neural".to_string(),
                speaking_rate: Some(1.0),
                audio_format: Some("pcm".to_string()),
                sample_rate: Some(16000),
                connection_timeout: Some(30),
                request_timeout: Some(60),
                pronunciations: Vec::new(),
                request_pool_size: Some(4),
                emotion_config: None,
                api_base: None,
            },
            region: AwsRegion::default(),
            aws_access_key_id: None,
            aws_secret_access_key: None,
            aws_session_token: None,
            voice: PollyVoice::default(),
            // Matches `base.model` above; a config RESOLVED from a request instead takes the
            // engine its model names, or none (`from_base`).
            engine: Some(PollyEngine::Neural),
            output_format: PollyOutputFormat::Pcm,
            text_type: TextType::default(),
            language_code: None,
            lexicon_names: Vec::new(),
            speech_mark_types: Vec::new(),
            endpoint_override: None,
        }
    }
}

/// The valid AWS Polly speech-mark type names. Used to filter the `speech_mark_types` extra so a
/// typo cannot silently reach the wire as an `Unknown` SDK variant.
pub const VALID_SPEECH_MARK_TYPES: [&str; 4] = ["word", "sentence", "viseme", "ssml"];

impl AwsPollyTTSConfig {
    /// Resolve a flat [`TTSConfig`] into the Polly request it describes — the one resolution both
    /// the flat (`AwsPollyTTS::new`) and the standardized (`from_standard`) paths go through, so
    /// the two cannot drift:
    ///
    /// * `voice_id` → [`PollyVoice`] (unset keeps Joanna; Polly requires a voice);
    /// * `model` → [`PollyEngine::from_model`]: empty sends no engine, an unknown one is an error;
    /// * `audio_format` → [`PollyOutputFormat::from_audio_format`]: explicit, unsupported is an
    ///   error;
    /// * `sample_rate` → [`PollyOutputFormat::resolve_sample_rate`]: what is SENT, and
    ///   `base.sample_rate` is rewritten to it so the rate the provider reports is the rate
    ///   Polly produced.
    pub fn from_base(mut base: TTSConfig) -> Result<Self, String> {
        let voice = base
            .voice_id
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .map(PollyVoice::from_str_or_default)
            .unwrap_or_default();
        let engine = PollyEngine::from_model(&base.model)?;
        let output_format = PollyOutputFormat::from_audio_format(base.audio_format.as_deref())?;
        let requested_rate = base.sample_rate;
        let sent_rate = output_format.resolve_sample_rate(requested_rate);
        if requested_rate.is_some() && requested_rate != sent_rate {
            tracing::debug!(
                requested = ?requested_rate,
                sent = ?sent_rate,
                format = output_format.as_str(),
                "Amazon Polly: requested sample rate is not valid for this output format"
            );
        }
        base.sample_rate = sent_rate;
        Ok(Self {
            base,
            voice,
            engine,
            output_format,
            ..Default::default()
        })
    }

    /// The sample rate of the audio Polly returns for this config: the rate sent, else Polly's
    /// documented default for the format and engine. This is what the provider REPORTS on every
    /// [`AudioData`](crate::core::tts::base::AudioData) chunk — the OpenAI route writes it into the
    /// WAV header and `x-sample-rate` — so it must equal what Polly produced.
    pub fn output_sample_rate(&self) -> u32 {
        self.base
            .sample_rate
            .unwrap_or_else(|| self.output_format.default_sample_rate_for(self.engine))
    }

    /// Build from the standardized config (W1 keystone). Maps the TTS features Polly can express
    /// to real fields: `ssml` toggles the input [`TextType`], `language` overrides the language
    /// code, and `sample_rate` sets the output rate. The non-standard `region` is read from the
    /// `provider_extras` passthrough. ElevenLabs-style voice settings, emotion, instructions,
    /// speed/pitch/volume and word timestamps have no dedicated Polly field (Polly expresses those
    /// via SSML prosody), so they are skipped.
    ///
    /// Voice, engine, output format and sample rate resolve through [`Self::from_base`]. Fails
    /// on a format Polly cannot produce, an unknown engine, a malformed region, or a partial set
    /// of credential extras.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, String> {
        let f = &std.features;
        // The canonical feature's rate wins over the flat one (it is the caller's choice on the
        // standardized path); either way it is resolved against the output format below.
        let mut base = std.base.clone();
        if let Some(rate) = f.sample_rate {
            base.sample_rate = Some(rate);
        }
        // Map the standardized voice/model/format onto Polly's dedicated `voice`/`engine`/
        // `output_format` fields (these are what the request builder actually reads). This used
        // to leave `output_format` at its PCM default whatever `audio_format` asked for.
        let mut cfg = Self::from_base(base)?;
        if let Some(region) = AwsRegion::from_extra(std.extras.0.get("region"))? {
            cfg.region = region;
        }
        if let Some(true) = f.ssml {
            cfg.text_type = TextType::Ssml;
        }
        if let Some(language) = &f.language {
            cfg.language_code = Some(language.clone());
        }
        // Speech marks (word / sentence / viseme / ssml timing & metadata). There is no shared
        // `TtsFeatures` field for this Polly-specific knob, so it flows through the `extras`
        // passthrough as a string array; only the four valid AWS names are kept so a typo cannot
        // reach the wire as an SDK `Unknown` variant.
        if let Some(marks) = std
            .extras
            .0
            .get("speech_mark_types")
            .and_then(|v| v.as_array())
        {
            cfg.speech_mark_types = marks
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_ascii_lowercase())
                .filter(|s| VALID_SPEECH_MARK_TYPES.contains(&s.as_str()))
                .collect();
        }
        // Endpoint override (mock harness): point the AWS SDK Polly client at a localhost mock.
        cfg.endpoint_override = std.endpoint_override().map(String::from);
        // AWS credentials flow through the standardized path via the `extras` passthrough; without
        // this the standard path could never authenticate (the explicit-credential branch in
        // `init_client` was unreachable from `from_standard`).
        if let Some(k) = std
            .extras
            .0
            .get("aws_access_key_id")
            .and_then(|v| v.as_str())
        {
            cfg.aws_access_key_id = Some(k.to_string());
        }
        if let Some(k) = std
            .extras
            .0
            .get("aws_secret_access_key")
            .and_then(|v| v.as_str())
        {
            cfg.aws_secret_access_key = Some(k.to_string());
        }
        if let Some(k) = std
            .extras
            .0
            .get("aws_session_token")
            .and_then(|v| v.as_str())
        {
            cfg.aws_session_token = Some(k.to_string());
        }
        // A partial set would fall through to the SDK default chain — the gateway's own identity.
        crate::core::stt::aws_transcribe::validate_explicit_credentials(
            &cfg.aws_access_key_id,
            &cfg.aws_secret_access_key,
            &cfg.aws_session_token,
        )?;
        Ok(cfg)
    }

    /// Create a new configuration with the given voice.
    pub fn with_voice(voice: PollyVoice) -> Self {
        let mut config = Self::default();
        config.base.voice_id = Some(voice.as_str().to_string());
        config.voice = voice;
        config
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<(), String> {
        // Validate sample rate for the output format
        if let Some(rate) = self.base.sample_rate {
            let supported = self.output_format.supported_sample_rates();
            if !supported.contains(&rate) {
                return Err(format!(
                    "Sample rate {} is not supported for {} format. Supported rates: {:?}",
                    rate,
                    self.output_format.as_str(),
                    supported
                ));
            }
        }

        // Validate lexicon count (max 5)
        if self.lexicon_names.len() > 5 {
            return Err("Maximum 5 lexicons can be applied per request".to_string());
        }

        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_aws_polly_tts_endpoint("endpoint_override", endpoint)?;
        }

        Ok(())
    }

    /// Check if explicit AWS credentials are provided.
    pub fn has_explicit_credentials(&self) -> bool {
        self.aws_access_key_id.is_some() && self.aws_secret_access_key.is_some()
    }

    /// Get the effective language code (voice default or override).
    pub fn effective_language_code(&self) -> &str {
        self.language_code
            .as_deref()
            .unwrap_or_else(|| self.voice.language_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // W1 keystone: the standardized features Polly can express (SSML input, language override,
    // output sample rate) reach their real fields, and the non-standard region flows through the
    // provider_extras passthrough.
    #[test]
    fn from_standard_maps_ssml_language_rate_and_region() {
        use crate::core::stt::standard::ProviderExtras;
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("region".into(), serde_json::json!("eu-west-1"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "aws-polly".into(),
                // 22050 is an mp3 rate; for pcm it would resolve to 16000 (see
                // `pcm_rate_is_clamped_to_what_polly_accepts`).
                audio_format: Some("mp3".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(true),
                language: Some("en-GB".into()),
                sample_rate: Some(22050),
                ..Default::default()
            },
            extras: ProviderExtras(extras),
        };
        let cfg = AwsPollyTTSConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.text_type, TextType::Ssml);
        assert_eq!(cfg.language_code, Some("en-GB".to_string()));
        assert_eq!(cfg.base.sample_rate, Some(22050));
        assert_eq!(cfg.region.as_str(), "eu-west-1");
    }

    fn std_for(
        audio_format: Option<&str>,
        model: &str,
        sample_rate: Option<u32>,
    ) -> crate::core::tts::standard::StandardTTSConfig {
        crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "aws-polly".into(),
            voice_id: Some("Joanna".into()),
            model: model.into(),
            audio_format: audio_format.map(String::from),
            sample_rate,
            ..Default::default()
        })
    }

    /// The requested format selects Polly's OutputFormat. `from_standard` used to leave it at
    /// PCM for every request, so an mp3/opus caller got raw PCM served as audio/mpeg.
    #[test]
    fn requested_format_selects_the_polly_output_format() {
        for (requested, expected) in [
            (Some("linear16"), PollyOutputFormat::Pcm),
            (Some("pcm"), PollyOutputFormat::Pcm),
            (Some("wav"), PollyOutputFormat::Pcm),
            (Some("mp3"), PollyOutputFormat::Mp3),
            (Some("opus"), PollyOutputFormat::OggOpus),
            (Some("ogg_opus"), PollyOutputFormat::OggOpus),
            (Some("ogg_vorbis"), PollyOutputFormat::OggVorbis),
            (Some("MP3"), PollyOutputFormat::Mp3),
            (None, PollyOutputFormat::Pcm),
        ] {
            let cfg = AwsPollyTTSConfig::from_standard(&std_for(requested, "", None))
                .unwrap_or_else(|e| panic!("{requested:?}: {e}"));
            assert_eq!(cfg.output_format, expected, "{requested:?}");
            assert_eq!(
                AwsPollyTTSConfig::from_base(std_for(requested, "", None).base)
                    .unwrap()
                    .output_format,
                expected,
                "flat path, {requested:?}"
            );
        }
    }

    /// `wav` used to parse to mp3 (the unknown-name fallback) on the flat path.
    #[test]
    fn wav_is_pcm_not_mp3() {
        assert_eq!(
            PollyOutputFormat::from_requested("wav"),
            Ok(PollyOutputFormat::Pcm)
        );
    }

    /// A format Polly has no codec for is refused, naming it — never served as something else.
    #[test]
    fn unsupported_format_is_a_configuration_error() {
        for bad in ["aac", "flac", "mulaw", "webm", "bogus"] {
            let err =
                AwsPollyTTSConfig::from_standard(&std_for(Some(bad), "", None)).expect_err(bad);
            assert!(err.contains(bad), "{bad}: {err}");
            assert!(err.contains("mp3") && err.contains("opus"), "{err}");
            assert!(PollyOutputFormat::from_requested(bad).is_err(), "{bad}");
        }
    }

    /// Polly pcm accepts only 8000/16000. The OpenAI route asks for 24000; that used to fail
    /// validation on every pcm request. 8000/16000 are kept, anything else (or nothing) sends
    /// 16000 — and the config's reported rate is that same value.
    #[test]
    fn pcm_rate_is_clamped_to_what_polly_accepts() {
        for (requested, sent) in [
            (Some(24000), 16000),
            (Some(22050), 16000),
            (Some(48000), 16000),
            (Some(16000), 16000),
            (Some(8000), 8000),
            (None, 16000),
        ] {
            for format in ["linear16", "wav"] {
                let cfg = AwsPollyTTSConfig::from_standard(&std_for(Some(format), "", requested))
                    .unwrap();
                assert_eq!(cfg.base.sample_rate, Some(sent), "{format} {requested:?}");
                assert_eq!(cfg.output_sample_rate(), sent, "{format} {requested:?}");
                assert!(cfg.validate().is_ok(), "{format} {requested:?}");
            }
        }
    }

    /// mp3/ogg_vorbis take 44100/48000 too. A rate Polly rejects is not sent; the reported rate is
    /// then Polly's default for the engine that will run (22050 standard, 24000 otherwise).
    #[test]
    fn compressed_rates_follow_polly_and_the_reported_default_follows_the_engine() {
        for rate in [8000, 16000, 22050, 24000, 44100, 48000] {
            let cfg =
                AwsPollyTTSConfig::from_standard(&std_for(Some("mp3"), "", Some(rate))).unwrap();
            assert_eq!(cfg.base.sample_rate, Some(rate));
            assert!(cfg.validate().is_ok(), "mp3 {rate}");
        }
        let cfg = AwsPollyTTSConfig::from_standard(&std_for(Some("mp3"), "", Some(11025))).unwrap();
        assert_eq!(cfg.base.sample_rate, None);
        assert_eq!(cfg.output_sample_rate(), 22050, "no engine sent = standard");
        let cfg =
            AwsPollyTTSConfig::from_standard(&std_for(Some("ogg_vorbis"), "neural", None)).unwrap();
        assert_eq!(cfg.base.sample_rate, None);
        assert_eq!(cfg.output_sample_rate(), 24000, "neural default");
    }

    /// ogg_opus has exactly one valid rate, so it is always sent and reported.
    #[test]
    fn opus_is_always_48000() {
        for requested in [None, Some(24000), Some(48000)] {
            let cfg =
                AwsPollyTTSConfig::from_standard(&std_for(Some("opus"), "", requested)).unwrap();
            assert_eq!(cfg.base.sample_rate, Some(48000));
            assert_eq!(cfg.output_sample_rate(), 48000);
            assert!(cfg.validate().is_ok());
        }
    }

    /// Engine: omitted for an empty model (Polly applies `standard`); the catalog names map; an
    /// unknown model is an error. Both used to become `neural`.
    #[test]
    fn engine_is_optional_and_unknown_models_are_refused() {
        let engine_for = |model: &str| {
            AwsPollyTTSConfig::from_standard(&std_for(Some("mp3"), model, None)).map(|c| c.engine)
        };
        assert_eq!(engine_for(""), Ok(None));
        assert_eq!(engine_for("  "), Ok(None));
        assert_eq!(engine_for("standard"), Ok(Some(PollyEngine::Standard)));
        assert_eq!(engine_for("neural"), Ok(Some(PollyEngine::Neural)));
        assert_eq!(engine_for("Long-Form"), Ok(Some(PollyEngine::LongForm)));
        assert_eq!(engine_for("generative"), Ok(Some(PollyEngine::Generative)));
        let err = engine_for("aura-2").expect_err("an unknown model must not become neural");
        assert!(err.contains("aura-2"), "{err}");
    }

    /// A region outside the old enum reaches the config verbatim; a malformed one is refused.
    #[test]
    fn region_extra_is_passed_through_or_refused() {
        use crate::core::stt::standard::ProviderExtras;
        let with_region = |region: &str| {
            let mut extras = serde_json::Map::new();
            extras.insert("region".into(), serde_json::json!(region));
            crate::core::tts::standard::StandardTTSConfig {
                extras: ProviderExtras(extras),
                ..std_for(Some("mp3"), "", None)
            }
        };
        let cfg = AwsPollyTTSConfig::from_standard(&with_region("eu-north-1")).unwrap();
        assert_eq!(cfg.region.as_str(), "eu-north-1");
        let err = AwsPollyTTSConfig::from_standard(&with_region("Stockholm"))
            .expect_err("a malformed region must not become us-east-1");
        assert!(err.contains("Stockholm"), "{err}");
    }

    /// A key id without its secret must not fall through to the gateway's default-chain identity.
    #[test]
    fn partial_credential_extras_are_refused() {
        use crate::core::stt::standard::ProviderExtras;
        let mut extras = serde_json::Map::new();
        extras.insert("aws_access_key_id".into(), serde_json::json!("AKIAONLY"));
        let std = crate::core::tts::standard::StandardTTSConfig {
            extras: ProviderExtras(extras),
            ..std_for(Some("mp3"), "", None)
        };
        let err = AwsPollyTTSConfig::from_standard(&std).expect_err("partial credentials");
        assert!(!err.contains("AKIAONLY"), "{err}");
    }

    #[test]
    fn test_polly_engine() {
        assert_eq!(PollyEngine::Neural.as_str(), "neural");
        assert_eq!(PollyEngine::Standard.as_str(), "standard");
        assert_eq!(
            PollyEngine::from_model("long-form"),
            Ok(Some(PollyEngine::LongForm))
        );
        assert_eq!(PollyEngine::from_model(""), Ok(None));
        assert!(PollyEngine::from_model("unknown").is_err());
    }

    #[test]
    fn test_polly_output_format() {
        assert_eq!(PollyOutputFormat::Mp3.as_str(), "mp3");
        assert_eq!(PollyOutputFormat::Pcm.as_str(), "pcm");
        assert_eq!(PollyOutputFormat::Pcm.mime_type(), "audio/pcm");
        assert_eq!(PollyOutputFormat::Pcm.default_sample_rate(), 16000);
        assert!(
            PollyOutputFormat::Pcm
                .supported_sample_rates()
                .contains(&16000)
        );
        assert_eq!(PollyOutputFormat::OggOpus.as_str(), "ogg_opus");
        assert_eq!(PollyOutputFormat::OggOpus.mime_type(), "audio/ogg");
        assert_eq!(
            PollyOutputFormat::OggOpus.supported_sample_rates(),
            &[48000]
        );
        assert_eq!(PollyOutputFormat::default(), PollyOutputFormat::Pcm);
    }

    #[test]
    fn test_polly_voice() {
        assert_eq!(PollyVoice::Joanna.as_str(), "Joanna");
        assert_eq!(PollyVoice::Joanna.language_code(), "en-US");
        assert!(PollyVoice::Joanna.supports_neural());
        assert_eq!(
            PollyVoice::from_str_or_default("matthew"),
            PollyVoice::Matthew
        );
    }

    #[test]
    fn test_polly_voice_custom() {
        let custom = PollyVoice::from_str_or_default("CustomVoice123");
        assert!(matches!(custom, PollyVoice::Custom(_)));
        assert_eq!(custom.as_str(), "CustomVoice123");
    }

    #[test]
    fn test_voices_for_language() {
        let us_voices = PollyVoice::voices_for_language("en-US");
        assert!(us_voices.contains(&PollyVoice::Joanna));
        assert!(us_voices.contains(&PollyVoice::Matthew));

        let de_voices = PollyVoice::voices_for_language("de-DE");
        assert!(de_voices.contains(&PollyVoice::Hans));
    }

    #[test]
    fn test_config_default() {
        let config = AwsPollyTTSConfig::default();
        assert_eq!(config.voice, PollyVoice::Joanna);
        assert_eq!(config.engine, Some(PollyEngine::Neural));
        assert_eq!(config.output_format, PollyOutputFormat::Pcm);
        assert_eq!(config.base.sample_rate, Some(16000));
    }

    #[test]
    fn test_config_validation_valid() {
        let config = AwsPollyTTSConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_validation_invalid_sample_rate() {
        let mut config = AwsPollyTTSConfig::default();
        config.output_format = PollyOutputFormat::Pcm;
        config.base.sample_rate = Some(44100); // Not supported for PCM

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Sample rate"));
    }

    #[test]
    fn test_config_validation_too_many_lexicons() {
        let mut config = AwsPollyTTSConfig::default();
        config.lexicon_names = vec![
            "lex1".to_string(),
            "lex2".to_string(),
            "lex3".to_string(),
            "lex4".to_string(),
            "lex5".to_string(),
            "lex6".to_string(), // 6th lexicon - should fail
        ];

        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("lexicons"));
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = AwsPollyTTSConfig::default();

        config.endpoint_override = Some("https://polly-proxy.example.com".to_string());
        assert!(config.validate().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.contains("SSRF protection"));

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate()
            .expect_err("file endpoint_override must be rejected");
        assert!(err.contains("URL scheme"));

        config.endpoint_override = Some("ws://polly-proxy.example.com".to_string());
        let err = config
            .validate()
            .expect_err("WebSocket endpoint_override must be rejected for REST Polly");
        assert!(err.contains("URL scheme"));

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "aws-polly".into(),
            sample_rate: Some(16000),
            ..Default::default()
        })
        .with_endpoint_override("file:///tmp/socket");
        let cfg = AwsPollyTTSConfig::from_standard(&std).unwrap();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_config_with_voice() {
        let config = AwsPollyTTSConfig::with_voice(PollyVoice::Amy);
        assert_eq!(config.voice, PollyVoice::Amy);
        assert_eq!(config.base.voice_id, Some("Amy".to_string()));
    }

    #[test]
    fn test_effective_language_code() {
        let config = AwsPollyTTSConfig::default();
        assert_eq!(config.effective_language_code(), "en-US");

        let mut config_override = AwsPollyTTSConfig::default();
        config_override.language_code = Some("en-GB".to_string());
        assert_eq!(config_override.effective_language_code(), "en-GB");
    }

    #[test]
    fn test_has_explicit_credentials() {
        let mut config = AwsPollyTTSConfig::default();
        assert!(!config.has_explicit_credentials());

        config.aws_access_key_id = Some("AKIAIOSFODNN7EXAMPLE".to_string());
        assert!(!config.has_explicit_credentials());

        config.aws_secret_access_key = Some("wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string());
        assert!(config.has_explicit_credentials());
    }

    #[test]
    fn test_text_type() {
        assert_eq!(TextType::Text.as_str(), "text");
        assert_eq!(TextType::Ssml.as_str(), "ssml");
        assert_eq!(TextType::from_str_or_default("ssml"), TextType::Ssml);
        assert_eq!(TextType::from_str_or_default("unknown"), TextType::Text);
    }
}
