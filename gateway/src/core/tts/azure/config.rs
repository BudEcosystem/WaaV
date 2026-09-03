//! Configuration types for Microsoft Azure Text-to-Speech API.
//!
//! This module contains all configuration-related types including:
//! - Audio encoding format specifications
//! - Provider-specific configuration options
//! - SSML generation utilities
//!
//! Note: The `AzureRegion` type is defined in `crate::core::providers::azure`
//! and should be used from there.

use crate::core::providers::azure::AzureRegion;
use crate::core::tts::base::{TTSConfig, TTSError};
use serde::{Deserialize, Serialize};

fn validate_azure_tts_endpoint(source: &str, endpoint: &str) -> Result<(), TTSError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Ok(());
    }

    crate::core::net::validate_url_for_ssrf(endpoint, crate::core::net::HTTP_URL_SCHEMES).map_err(
        |msg| TTSError::InvalidConfiguration(format!("{source} rejected (SSRF protection): {msg}")),
    )
}

fn validate_azure_provider_url(source: &str, url: &str) -> Result<(), TTSError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(TTSError::InvalidConfiguration(format!(
            "{source} rejected (SSRF protection): empty URL"
        )));
    }

    crate::core::net::validate_url_for_ssrf(url, crate::core::net::HTTP_URL_SCHEMES).map_err(
        |msg| TTSError::InvalidConfiguration(format!("{source} rejected (SSRF protection): {msg}")),
    )
}

/// HTTP header name for Azure TTS output format.
pub const AZURE_OUTPUT_FORMAT_HEADER: &str = "X-Microsoft-OutputFormat";

/// Default Azure TTS endpoint URL pattern.
///
/// Azure TTS uses regional endpoints in the format:
/// `https://{region}.tts.speech.microsoft.com/cognitiveservices/v1`
///
/// This constant provides the default (eastus) endpoint for compatibility with
/// other TTS providers. For specific regions, use `AzureRegion::tts_rest_url()`.
pub const AZURE_TTS_URL: &str = "https://eastus.tts.speech.microsoft.com/cognitiveservices/v1";

// =============================================================================
// Audio Encoding
// =============================================================================

/// Azure Text-to-Speech audio output format options.
///
/// These formats map to Azure's `X-Microsoft-OutputFormat` header values.
/// Formats are grouped into categories:
///
/// - **Raw PCM**: Uncompressed audio for real-time streaming
/// - **MP3**: Compressed audio for storage/bandwidth optimization
/// - **Opus**: Low-latency compressed audio for real-time applications
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::tts::azure::AzureAudioEncoding;
///
/// let format = AzureAudioEncoding::Raw24Khz16BitMonoPcm;
/// assert_eq!(format.as_str(), "raw-24khz-16bit-mono-pcm");
/// assert_eq!(format.sample_rate(), 24000);
/// assert!(format.is_pcm());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AzureAudioEncoding {
    // =========================================================================
    // Raw PCM Formats (streaming, no container)
    // =========================================================================
    /// 8kHz, 8-bit μ-law mono (telephony)
    Raw8Khz8BitMonoMulaw,
    /// 8kHz, 8-bit A-law mono (telephony)
    Raw8Khz8BitMonoAlaw,
    /// 8kHz, 16-bit PCM mono
    Raw8Khz16BitMonoPcm,
    /// 16kHz, 16-bit PCM mono
    Raw16Khz16BitMonoPcm,
    /// 22.05kHz, 16-bit PCM mono
    Raw22050Hz16BitMonoPcm,
    /// 24kHz, 16-bit PCM mono (recommended for most use cases)
    #[default]
    Raw24Khz16BitMonoPcm,
    /// 44.1kHz, 16-bit PCM mono (CD quality)
    Raw44100Hz16BitMonoPcm,
    /// 48kHz, 16-bit PCM mono (highest quality)
    Raw48Khz16BitMonoPcm,

    // =========================================================================
    // MP3 Streaming Formats
    // =========================================================================
    /// 16kHz, 32kbps MP3 mono
    Audio16Khz32KbitrateMonoMp3,
    /// 16kHz, 64kbps MP3 mono
    Audio16Khz64KbitrateMonoMp3,
    /// 24kHz, 48kbps MP3 mono
    Audio24Khz48KbitrateMonoMp3,
    /// 24kHz, 96kbps MP3 mono
    Audio24Khz96KbitrateMonoMp3,
    /// 48kHz, 96kbps MP3 mono
    Audio48Khz96KbitrateMonoMp3,
    /// 48kHz, 192kbps MP3 mono (highest MP3 quality)
    Audio48Khz192KbitrateMonoMp3,

    // =========================================================================
    // Opus Formats (low-latency streaming)
    // =========================================================================
    /// 16kHz, 16-bit, 32kbps Opus mono
    Audio16Khz16Bit32KbpsMonoOpus,
    /// 24kHz, 16-bit, 24kbps Opus mono
    Audio24Khz16Bit24KbpsMonoOpus,
    /// 24kHz, 16-bit, 48kbps Opus mono
    Audio24Khz16Bit48KbpsMonoOpus,
}

impl AzureAudioEncoding {
    /// Returns the Azure API format string for the `X-Microsoft-OutputFormat` header.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::azure::AzureAudioEncoding;
    ///
    /// assert_eq!(AzureAudioEncoding::Raw24Khz16BitMonoPcm.as_str(), "raw-24khz-16bit-mono-pcm");
    /// assert_eq!(AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.as_str(), "audio-24khz-96kbitrate-mono-mp3");
    /// ```
    #[inline]
    pub fn as_str(&self) -> &'static str {
        match self {
            // Raw PCM formats
            Self::Raw8Khz8BitMonoMulaw => "raw-8khz-8bit-mono-mulaw",
            Self::Raw8Khz8BitMonoAlaw => "raw-8khz-8bit-mono-alaw",
            Self::Raw8Khz16BitMonoPcm => "raw-8khz-16bit-mono-pcm",
            Self::Raw16Khz16BitMonoPcm => "raw-16khz-16bit-mono-pcm",
            Self::Raw22050Hz16BitMonoPcm => "raw-22050hz-16bit-mono-pcm",
            Self::Raw24Khz16BitMonoPcm => "raw-24khz-16bit-mono-pcm",
            Self::Raw44100Hz16BitMonoPcm => "raw-44100hz-16bit-mono-pcm",
            Self::Raw48Khz16BitMonoPcm => "raw-48khz-16bit-mono-pcm",
            // MP3 formats
            Self::Audio16Khz32KbitrateMonoMp3 => "audio-16khz-32kbitrate-mono-mp3",
            Self::Audio16Khz64KbitrateMonoMp3 => "audio-16khz-64kbitrate-mono-mp3",
            Self::Audio24Khz48KbitrateMonoMp3 => "audio-24khz-48kbitrate-mono-mp3",
            Self::Audio24Khz96KbitrateMonoMp3 => "audio-24khz-96kbitrate-mono-mp3",
            Self::Audio48Khz96KbitrateMonoMp3 => "audio-48khz-96kbitrate-mono-mp3",
            Self::Audio48Khz192KbitrateMonoMp3 => "audio-48khz-192kbitrate-mono-mp3",
            // Opus formats
            Self::Audio16Khz16Bit32KbpsMonoOpus => "audio-16khz-16bit-32kbps-mono-opus",
            Self::Audio24Khz16Bit24KbpsMonoOpus => "audio-24khz-16bit-24kbps-mono-opus",
            Self::Audio24Khz16Bit48KbpsMonoOpus => "audio-24khz-16bit-48kbps-mono-opus",
        }
    }

    /// Returns the sample rate in Hz for this audio format.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::azure::AzureAudioEncoding;
    ///
    /// assert_eq!(AzureAudioEncoding::Raw8Khz16BitMonoPcm.sample_rate(), 8000);
    /// assert_eq!(AzureAudioEncoding::Raw24Khz16BitMonoPcm.sample_rate(), 24000);
    /// assert_eq!(AzureAudioEncoding::Audio48Khz192KbitrateMonoMp3.sample_rate(), 48000);
    /// ```
    #[inline]
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Raw8Khz8BitMonoMulaw | Self::Raw8Khz8BitMonoAlaw | Self::Raw8Khz16BitMonoPcm => {
                8000
            }

            Self::Raw16Khz16BitMonoPcm
            | Self::Audio16Khz32KbitrateMonoMp3
            | Self::Audio16Khz64KbitrateMonoMp3
            | Self::Audio16Khz16Bit32KbpsMonoOpus => 16000,

            Self::Raw22050Hz16BitMonoPcm => 22050,

            Self::Raw24Khz16BitMonoPcm
            | Self::Audio24Khz48KbitrateMonoMp3
            | Self::Audio24Khz96KbitrateMonoMp3
            | Self::Audio24Khz16Bit24KbpsMonoOpus
            | Self::Audio24Khz16Bit48KbpsMonoOpus => 24000,

            Self::Raw44100Hz16BitMonoPcm => 44100,

            Self::Raw48Khz16BitMonoPcm
            | Self::Audio48Khz96KbitrateMonoMp3
            | Self::Audio48Khz192KbitrateMonoMp3 => 48000,
        }
    }

    /// Returns true if this is a raw PCM format (uncompressed audio).
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::azure::AzureAudioEncoding;
    ///
    /// assert!(AzureAudioEncoding::Raw24Khz16BitMonoPcm.is_pcm());
    /// assert!(!AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.is_pcm());
    /// assert!(!AzureAudioEncoding::Audio24Khz16Bit48KbpsMonoOpus.is_pcm());
    /// ```
    #[inline]
    pub fn is_pcm(&self) -> bool {
        matches!(
            self,
            Self::Raw8Khz16BitMonoPcm
                | Self::Raw16Khz16BitMonoPcm
                | Self::Raw22050Hz16BitMonoPcm
                | Self::Raw24Khz16BitMonoPcm
                | Self::Raw44100Hz16BitMonoPcm
                | Self::Raw48Khz16BitMonoPcm
        )
    }

    /// Returns true if this is a μ-law or A-law format (telephony).
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::azure::AzureAudioEncoding;
    ///
    /// assert!(AzureAudioEncoding::Raw8Khz8BitMonoMulaw.is_telephony());
    /// assert!(AzureAudioEncoding::Raw8Khz8BitMonoAlaw.is_telephony());
    /// assert!(!AzureAudioEncoding::Raw24Khz16BitMonoPcm.is_telephony());
    /// ```
    #[inline]
    pub fn is_telephony(&self) -> bool {
        matches!(self, Self::Raw8Khz8BitMonoMulaw | Self::Raw8Khz8BitMonoAlaw)
    }

    /// Returns the appropriate MIME content type for this audio format.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::azure::AzureAudioEncoding;
    ///
    /// assert_eq!(AzureAudioEncoding::Raw24Khz16BitMonoPcm.content_type(), "audio/pcm");
    /// assert_eq!(AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.content_type(), "audio/mpeg");
    /// assert_eq!(AzureAudioEncoding::Audio24Khz16Bit48KbpsMonoOpus.content_type(), "audio/opus");
    /// assert_eq!(AzureAudioEncoding::Raw8Khz8BitMonoMulaw.content_type(), "audio/mulaw");
    /// ```
    #[inline]
    pub fn content_type(&self) -> &'static str {
        match self {
            Self::Raw8Khz8BitMonoMulaw => "audio/mulaw",
            Self::Raw8Khz8BitMonoAlaw => "audio/alaw",
            Self::Raw8Khz16BitMonoPcm
            | Self::Raw16Khz16BitMonoPcm
            | Self::Raw22050Hz16BitMonoPcm
            | Self::Raw24Khz16BitMonoPcm
            | Self::Raw44100Hz16BitMonoPcm
            | Self::Raw48Khz16BitMonoPcm => "audio/pcm",
            Self::Audio16Khz32KbitrateMonoMp3
            | Self::Audio16Khz64KbitrateMonoMp3
            | Self::Audio24Khz48KbitrateMonoMp3
            | Self::Audio24Khz96KbitrateMonoMp3
            | Self::Audio48Khz96KbitrateMonoMp3
            | Self::Audio48Khz192KbitrateMonoMp3 => "audio/mpeg",
            Self::Audio16Khz16Bit32KbpsMonoOpus
            | Self::Audio24Khz16Bit24KbpsMonoOpus
            | Self::Audio24Khz16Bit48KbpsMonoOpus => "audio/opus",
        }
    }

    /// Converts a base config format string and sample rate to the appropriate Azure encoding.
    ///
    /// This method maps common format string variations to the correct enum variant:
    /// - "linear16", "pcm", "wav" → Raw PCM format at specified sample rate
    /// - "mp3" → MP3 format at specified sample rate
    /// - "mulaw", "ulaw" → `Raw8Khz8BitMonoMulaw`
    /// - "alaw" → `Raw8Khz8BitMonoAlaw`
    /// - "opus" → Opus format at specified sample rate
    /// - Unknown formats default to `Raw24Khz16BitMonoPcm`
    ///
    /// # Arguments
    ///
    /// * `format` - The audio format string from base config
    /// * `sample_rate` - The sample rate to match
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::azure::AzureAudioEncoding;
    ///
    /// assert_eq!(
    ///     AzureAudioEncoding::from_format_string("pcm", 24000),
    ///     AzureAudioEncoding::Raw24Khz16BitMonoPcm
    /// );
    /// assert_eq!(
    ///     AzureAudioEncoding::from_format_string("mp3", 24000),
    ///     AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3
    /// );
    /// assert_eq!(
    ///     AzureAudioEncoding::from_format_string("mulaw", 8000),
    ///     AzureAudioEncoding::Raw8Khz8BitMonoMulaw
    /// );
    /// ```
    pub fn from_format_string(format: &str, sample_rate: u32) -> Self {
        match format.to_lowercase().as_str() {
            "linear16" | "pcm" | "wav" => Self::pcm_for_sample_rate(sample_rate),
            "mp3" => Self::mp3_for_sample_rate(sample_rate),
            "mulaw" | "ulaw" => Self::Raw8Khz8BitMonoMulaw,
            "alaw" => Self::Raw8Khz8BitMonoAlaw,
            "opus" => Self::opus_for_sample_rate(sample_rate),
            _ => Self::default(),
        }
    }

    /// Select the best PCM format for a given sample rate.
    fn pcm_for_sample_rate(sample_rate: u32) -> Self {
        match sample_rate {
            0..=8000 => Self::Raw8Khz16BitMonoPcm,
            8001..=16000 => Self::Raw16Khz16BitMonoPcm,
            16001..=22050 => Self::Raw22050Hz16BitMonoPcm,
            22051..=24000 => Self::Raw24Khz16BitMonoPcm,
            24001..=44100 => Self::Raw44100Hz16BitMonoPcm,
            _ => Self::Raw48Khz16BitMonoPcm,
        }
    }

    /// Select the best MP3 format for a given sample rate (using highest bitrate).
    fn mp3_for_sample_rate(sample_rate: u32) -> Self {
        match sample_rate {
            0..=16000 => Self::Audio16Khz64KbitrateMonoMp3,
            16001..=24000 => Self::Audio24Khz96KbitrateMonoMp3,
            _ => Self::Audio48Khz192KbitrateMonoMp3,
        }
    }

    /// Select the best Opus format for a given sample rate (using highest bitrate).
    fn opus_for_sample_rate(sample_rate: u32) -> Self {
        match sample_rate {
            0..=16000 => Self::Audio16Khz16Bit32KbpsMonoOpus,
            _ => Self::Audio24Khz16Bit48KbpsMonoOpus,
        }
    }
}

// =============================================================================
// SSML Generation
// =============================================================================

/// Escapes special XML characters in text for use in SSML.
///
/// Replaces the following characters:
/// - `&` → `&amp;`
/// - `<` → `&lt;`
/// - `>` → `&gt;`
/// - `"` → `&quot;`
/// - `'` → `&apos;`
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::tts::azure::escape_xml;
///
/// assert_eq!(escape_xml("Hello & goodbye"), "Hello &amp; goodbye");
/// assert_eq!(escape_xml("<script>alert('xss')</script>"), "&lt;script&gt;alert(&apos;xss&apos;)&lt;/script&gt;");
/// ```
pub fn escape_xml(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&apos;"),
            _ => result.push(c),
        }
    }
    result
}

/// The Microsoft TTS SSML namespace, required on `<speak>` for any `mstts:*` element
/// (such as `mstts:express-as`). Confirmed against the Azure Speech SSML reference
/// (`speech-synthesis-markup-voice`, doc ms.date 2026-01-30): every `mstts:express-as`
/// example declares `xmlns:mstts="https://www.w3.org/2001/mstts"` on the root element.
pub const AZURE_MSTTS_NAMESPACE: &str = "https://www.w3.org/2001/mstts";

/// Escapes a value for use inside a double-quoted XML attribute.
///
/// Thin alias over [`escape_xml`] (which already escapes `"`, `&`, `<`, `>`, `'`) so the
/// intent at each attribute-emission site is explicit. Every attribute value composed into the
/// Azure SSML body below is passed through this so arbitrary `extras`/feature strings cannot break
/// SSML well-formedness or inject markup.
#[inline]
fn escape_attr(value: &str) -> String {
    escape_xml(value)
}

/// All the additional Azure SSML knobs that are documented on the `cognitiveservices/v1` REST
/// synthesis endpoint (which consumes SSML) but were previously unreachable through config.
///
/// Every field here is emitted into the SSML body by [`build_ssml_with_options`]. The endpoint is
/// controlled entirely by the SSML body plus four headers, so the SSML body IS the wire — these
/// are real wire parameters, confirmed against the Azure Speech SSML reference (doc ms.date
/// 2026-01-30, `speech-synthesis-markup-voice` / `-pronunciation`). Fields are `Option`/empty by
/// default so an all-default `AzureSsmlOptions` reproduces the original minimal SSML.
///
/// Voice-gated attributes (style/styledegree/role/emphasis) are silently ignored by the service on
/// voices that don't support them (per the docs), so emitting them unconditionally is safe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AzureSsmlOptions {
    // ---- <prosody> attributes (wrap the spoken text, alongside rate) -----------------------
    /// `<prosody pitch="...">` — baseline pitch (e.g. "+10%", "-2st", "high", "600Hz").
    pub pitch: Option<String>,
    /// `<prosody volume="...">` — volume level (e.g. "+20%", "loud", "75").
    pub volume: Option<String>,
    /// `<prosody contour="...">` — pitch contour (e.g. "(0%,+20Hz) (100%,-10Hz)").
    pub contour: Option<String>,
    /// `<prosody range="...">` — pitch range (e.g. "+20%", "x-high").
    pub range: Option<String>,

    // ---- <mstts:express-as> attributes (alongside style/emotion) ---------------------------
    /// `<mstts:express-as styledegree="...">` — style intensity (0.01–2).
    pub style_degree: Option<String>,
    /// `<mstts:express-as role="...">` — role-play (e.g. "YoungAdultFemale").
    pub role: Option<String>,

    // ---- inline text-wrapping elements -----------------------------------------------------
    /// `<emphasis level="...">` — word/sentence emphasis (reduced|none|moderate|strong).
    pub emphasis: Option<String>,
    /// `<say-as interpret-as="..." format="..." detail="...">` text normalization.
    pub say_as_interpret_as: Option<String>,
    /// `format` attribute for `<say-as>` (only emitted when `say_as_interpret_as` is set).
    pub say_as_format: Option<String>,
    /// `detail` attribute for `<say-as>` (only emitted when `say_as_interpret_as` is set).
    pub say_as_detail: Option<String>,
    /// `<phoneme alphabet="..." ph="...">` — phonetic pronunciation. Both required together.
    pub phoneme_alphabet: Option<String>,
    /// `ph` attribute for `<phoneme>` (only emitted together with `phoneme_alphabet`).
    pub phoneme_ph: Option<String>,
    /// `<sub alias="...">` — substitution alias (spoken form replaces written text).
    pub sub_alias: Option<String>,

    // ---- voice-level siblings (children of <voice>, before/around the text) ----------------
    /// `<voice ... effect="...">` — audio effect processor (eq_car|eq_telecomhp8k).
    pub effect: Option<String>,
    /// `<lexicon uri="..."/>` — custom lexicon URI (first child of `<voice>`).
    pub lexicon_uri: Option<String>,
    /// `<mstts:audioduration value="..."/>` — target output duration (e.g. "20s", "2000ms").
    pub audio_duration: Option<String>,
    /// `<mstts:silence type="..." value="..."/>` — configurable pauses/silence.
    pub silence_type: Option<String>,
    /// `value` for `<mstts:silence>` (only emitted together with `silence_type`).
    pub silence_value: Option<String>,
    /// `<mstts:ttsembedding speakerProfileId="...">` — personal-voice speaker profile.
    pub speaker_profile_id: Option<String>,

    // ---- <speak>-level child ---------------------------------------------------------------
    /// `<mstts:backgroundaudio src="..." volume="..." fadein="..." fadeout="..."/>` — first
    /// child of `<speak>`. Only the `src` is required; the rest are optional sub-fields.
    pub background_audio_src: Option<String>,
    /// `volume` for `<mstts:backgroundaudio>`.
    pub background_audio_volume: Option<String>,
    /// `fadein` (ms) for `<mstts:backgroundaudio>`.
    pub background_audio_fadein: Option<String>,
    /// `fadeout` (ms) for `<mstts:backgroundaudio>`.
    pub background_audio_fadeout: Option<String>,
}

impl AzureSsmlOptions {
    /// True if any option requires the `xmlns:mstts` namespace on `<speak>` (i.e. any `mstts:*`
    /// element is emitted). express-as is handled separately via the `emotion` argument.
    fn needs_mstts_namespace(&self) -> bool {
        self.style_degree.is_some()
            || self.role.is_some()
            || self.audio_duration.is_some()
            || self.silence_type.is_some()
            || self.speaker_profile_id.is_some()
            || self.background_audio_src.is_some()
    }
}

/// Builds an SSML document for Azure TTS.
///
/// Wraps the provided text in a valid SSML document with the specified voice
/// and language. Optionally includes a prosody element for speaking rate control
/// and an `mstts:express-as` wrapper for emotional/styled delivery.
///
/// # Emotion (`mstts:express-as`)
///
/// When `emotion` is `Some(style)`, the (prosody-wrapped) text is further wrapped in
/// `<mstts:express-as style="...">`, and the `xmlns:mstts` namespace is declared on the
/// `<speak>` element. This is the only wire vector Azure exposes for emotion on the
/// `cognitiveservices/v1` REST synthesis endpoint — the endpoint consumes SSML, and the
/// `style` attribute is the documented control (Azure SSML reference, doc ms.date
/// 2026-01-30). `express-as` is supported only on a subset of neural voices; for voices
/// that do not support the supplied style the service silently ignores the element and
/// falls back to neutral speech (per the docs: "If the style value is missing or invalid,
/// the entire `mstts:express-as` element is ignored"). Emitting it unconditionally is
/// therefore safe — it never breaks synthesis on unsupported voices.
///
/// # Arguments
///
/// * `text` - The text to synthesize (will be XML-escaped)
/// * `voice_name` - Azure voice name (e.g., "en-US-JennyNeural")
/// * `language` - BCP-47 language code (e.g., "en-US")
/// * `speaking_rate` - Optional speaking rate multiplier (0.5 to 2.0, where 1.0 is normal)
/// * `emotion` - Optional `mstts:express-as` style (e.g., "cheerful", "sad", "angry")
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::tts::azure::build_ssml;
///
/// let ssml = build_ssml("Hello world!", "en-US-JennyNeural", "en-US", None, None);
/// assert!(ssml.contains("<speak"));
/// assert!(ssml.contains("en-US-JennyNeural"));
/// assert!(ssml.contains("Hello world!"));
///
/// let ssml_with_rate = build_ssml("Fast speech", "en-US-JennyNeural", "en-US", Some(1.5), None);
/// assert!(ssml_with_rate.contains("rate=\"150%\""));
///
/// let ssml_emo = build_ssml("Yay!", "en-US-JennyNeural", "en-US", None, Some("cheerful"));
/// assert!(ssml_emo.contains("mstts:express-as style=\"cheerful\""));
/// assert!(ssml_emo.contains("xmlns:mstts"));
/// ```
pub fn build_ssml(
    text: &str,
    voice_name: &str,
    language: &str,
    speaking_rate: Option<f32>,
    emotion: Option<&str>,
) -> String {
    build_ssml_with_options(
        text,
        voice_name,
        language,
        speaking_rate,
        emotion,
        &AzureSsmlOptions::default(),
    )
}

/// Builds an SSML document for Azure TTS with the full set of [`AzureSsmlOptions`].
///
/// This is the wire surface for every advanced Azure SSML feature: the `cognitiveservices/v1`
/// REST synthesis endpoint consumes the returned SSML as the request body, so each attribute
/// emitted here is an actual wire parameter (Azure Speech SSML reference, doc ms.date 2026-01-30).
///
/// Composition order, innermost → outermost around the text:
/// 1. `<sub alias>` / `<say-as>` / `<phoneme>` normalize/replace the literal text (mutually the
///    innermost wrap; applied in that precedence if several are set).
/// 2. `<prosody>` (rate/pitch/volume/contour/range) wraps that.
/// 3. `<emphasis level>` wraps the prosody.
/// 4. `<mstts:express-as style/styledegree/role>` wraps the emphasis (when emotion/style set).
/// Voice-level siblings (`<lexicon>`, `<mstts:audioduration>`, `<mstts:silence>`,
/// `<mstts:ttsembedding>`) and the `<voice effect>` attribute sit on/inside `<voice>`, and
/// `<mstts:backgroundaudio>` is the first child of `<speak>`.
pub fn build_ssml_with_options(
    text: &str,
    voice_name: &str,
    language: &str,
    speaking_rate: Option<f32>,
    emotion: Option<&str>,
    opts: &AzureSsmlOptions,
) -> String {
    // ---- innermost: normalize/replace the literal text ------------------------------------
    // Precedence sub > phoneme > say-as so a single explicit substitution wins; each escapes its
    // own attribute values.
    let escaped_text = escape_xml(text);
    let mut inner = if let Some(alias) = opts.sub_alias.as_deref().filter(|s| !s.is_empty()) {
        format!("<sub alias=\"{}\">{escaped_text}</sub>", escape_attr(alias))
    } else if let (Some(alphabet), Some(ph)) = (
        opts.phoneme_alphabet.as_deref().filter(|s| !s.is_empty()),
        opts.phoneme_ph.as_deref().filter(|s| !s.is_empty()),
    ) {
        format!(
            "<phoneme alphabet=\"{}\" ph=\"{}\">{escaped_text}</phoneme>",
            escape_attr(alphabet),
            escape_attr(ph)
        )
    } else if let Some(interpret) = opts
        .say_as_interpret_as
        .as_deref()
        .filter(|s| !s.is_empty())
    {
        let format_attr = opts
            .say_as_format
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|f| format!(" format=\"{}\"", escape_attr(f)))
            .unwrap_or_default();
        let detail_attr = opts
            .say_as_detail
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|d| format!(" detail=\"{}\"", escape_attr(d)))
            .unwrap_or_default();
        format!(
            "<say-as interpret-as=\"{}\"{format_attr}{detail_attr}>{escaped_text}</say-as>",
            escape_attr(interpret)
        )
    } else {
        escaped_text
    };

    // ---- <prosody>: rate + pitch + volume + contour + range -------------------------------
    let mut prosody_attrs = String::new();
    if let Some(rate) = speaking_rate
        && (rate - 1.0).abs() > 0.01
    {
        let rate_percent = (rate * 100.0).round() as i32;
        prosody_attrs.push_str(&format!(" rate=\"{rate_percent}%\""));
    }
    if let Some(pitch) = opts.pitch.as_deref().filter(|s| !s.is_empty()) {
        prosody_attrs.push_str(&format!(" pitch=\"{}\"", escape_attr(pitch)));
    }
    if let Some(volume) = opts.volume.as_deref().filter(|s| !s.is_empty()) {
        prosody_attrs.push_str(&format!(" volume=\"{}\"", escape_attr(volume)));
    }
    if let Some(contour) = opts.contour.as_deref().filter(|s| !s.is_empty()) {
        prosody_attrs.push_str(&format!(" contour=\"{}\"", escape_attr(contour)));
    }
    if let Some(range) = opts.range.as_deref().filter(|s| !s.is_empty()) {
        prosody_attrs.push_str(&format!(" range=\"{}\"", escape_attr(range)));
    }
    if !prosody_attrs.is_empty() {
        inner = format!("<prosody{prosody_attrs}>{inner}</prosody>");
    }

    // ---- <emphasis level> -----------------------------------------------------------------
    if let Some(level) = opts.emphasis.as_deref().filter(|s| !s.is_empty()) {
        inner = format!(
            "<emphasis level=\"{}\">{inner}</emphasis>",
            escape_attr(level)
        );
    }

    // ---- <mstts:express-as style/styledegree/role> ----------------------------------------
    let express_as_present = emotion.is_some_and(|s| !s.is_empty())
        || opts.style_degree.is_some()
        || opts.role.is_some();
    if express_as_present {
        let mut attrs = String::new();
        if let Some(style) = emotion.filter(|s| !s.is_empty()) {
            attrs.push_str(&format!(" style=\"{}\"", escape_attr(style)));
        }
        if let Some(degree) = opts.style_degree.as_deref().filter(|s| !s.is_empty()) {
            attrs.push_str(&format!(" styledegree=\"{}\"", escape_attr(degree)));
        }
        if let Some(role) = opts.role.as_deref().filter(|s| !s.is_empty()) {
            attrs.push_str(&format!(" role=\"{}\"", escape_attr(role)));
        }
        inner = format!("<mstts:express-as{attrs}>{inner}</mstts:express-as>");
    }

    // ---- voice-level siblings (prefix children of <voice>) --------------------------------
    let mut voice_prefix = String::new();
    if let Some(uri) = opts.lexicon_uri.as_deref().filter(|s| !s.is_empty()) {
        voice_prefix.push_str(&format!("<lexicon uri=\"{}\"/>", escape_attr(uri)));
    }
    if let Some(duration) = opts.audio_duration.as_deref().filter(|s| !s.is_empty()) {
        voice_prefix.push_str(&format!(
            "<mstts:audioduration value=\"{}\"/>",
            escape_attr(duration)
        ));
    }
    if let (Some(stype), Some(svalue)) = (
        opts.silence_type.as_deref().filter(|s| !s.is_empty()),
        opts.silence_value.as_deref().filter(|s| !s.is_empty()),
    ) {
        voice_prefix.push_str(&format!(
            "<mstts:silence type=\"{}\" value=\"{}\"/>",
            escape_attr(stype),
            escape_attr(svalue)
        ));
    }

    // ---- <mstts:ttsembedding speakerProfileId> wraps the voiced content (personal voice) --
    if let Some(profile) = opts.speaker_profile_id.as_deref().filter(|s| !s.is_empty()) {
        inner = format!(
            "<mstts:ttsembedding speakerProfileId=\"{}\">{inner}</mstts:ttsembedding>",
            escape_attr(profile)
        );
    }

    // ---- <voice ... effect> attribute -----------------------------------------------------
    let effect_attr = opts
        .effect
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|e| format!(" effect=\"{}\"", escape_attr(e)))
        .unwrap_or_default();

    // ---- <mstts:backgroundaudio> (first child of <speak>) ---------------------------------
    let background_audio = opts
        .background_audio_src
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|src| {
            let mut attrs = format!(" src=\"{}\"", escape_attr(src));
            if let Some(v) = opts
                .background_audio_volume
                .as_deref()
                .filter(|s| !s.is_empty())
            {
                attrs.push_str(&format!(" volume=\"{}\"", escape_attr(v)));
            }
            if let Some(f) = opts
                .background_audio_fadein
                .as_deref()
                .filter(|s| !s.is_empty())
            {
                attrs.push_str(&format!(" fadein=\"{}\"", escape_attr(f)));
            }
            if let Some(f) = opts
                .background_audio_fadeout
                .as_deref()
                .filter(|s| !s.is_empty())
            {
                attrs.push_str(&format!(" fadeout=\"{}\"", escape_attr(f)));
            }
            format!("<mstts:backgroundaudio{attrs}/>")
        })
        .unwrap_or_default();

    // ---- namespace: required for any mstts:* element (express-as OR an opts mstts element) -
    let mstts_ns = if (emotion.is_some_and(|s| !s.is_empty())
        || opts.style_degree.is_some()
        || opts.role.is_some())
        || opts.needs_mstts_namespace()
    {
        format!(" xmlns:mstts='{AZURE_MSTTS_NAMESPACE}'")
    } else {
        String::new()
    };

    format!(
        r#"<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis'{mstts_ns} xml:lang='{language}'>
    {background_audio}<voice name='{voice_name}'{effect_attr}>
        {voice_prefix}{inner}
    </voice>
</speak>"#,
    )
}

// =============================================================================
// Main Configuration
// =============================================================================

/// Configuration specific to Microsoft Azure Text-to-Speech API.
///
/// This configuration extends the base `TTSConfig` with Azure-specific
/// parameters for the REST API synthesis endpoint.
///
/// # Example
///
/// ```rust
/// use waav_gateway::core::stt::azure::AzureRegion;
/// use waav_gateway::core::tts::azure::{AzureTTSConfig, AzureAudioEncoding};
/// use waav_gateway::core::tts::TTSConfig;
///
/// let config = AzureTTSConfig {
///     base: TTSConfig {
///         api_key: "your-subscription-key".to_string(),
///         voice_id: Some("en-US-JennyNeural".to_string()),
///         ..Default::default()
///     },
///     region: AzureRegion::WestEurope,
///     output_format: AzureAudioEncoding::Raw24Khz16BitMonoPcm,
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone)]
pub struct AzureTTSConfig {
    /// Base TTS configuration (shared across all providers).
    ///
    /// Contains common settings like api_key, voice_id, sample_rate, etc.
    pub base: TTSConfig,

    /// Azure region for the Speech Service endpoint.
    ///
    /// Choose the region closest to your users for optimal latency.
    pub region: AzureRegion,

    /// Output audio format for synthesis results.
    ///
    /// Maps to the `X-Microsoft-OutputFormat` header value.
    pub output_format: AzureAudioEncoding,

    /// Whether to wrap text in SSML for synthesis.
    ///
    /// When `true` (default), plain text is wrapped in SSML with the
    /// configured voice and speaking rate. When `false`, the text is
    /// sent as-is (assumes caller provides valid SSML).
    pub use_ssml: bool,

    /// Optional `mstts:express-as` speaking style / emotion (e.g. "cheerful", "sad").
    ///
    /// When set (and `use_ssml` is true), `build_ssml_for_text` wraps the synthesized
    /// text in `<mstts:express-as style="...">` and declares the `xmlns:mstts`
    /// namespace. This is the documented Azure wire control for emotion on the
    /// `cognitiveservices/v1` REST synthesis endpoint (the endpoint consumes SSML).
    /// Only a subset of neural voices honor it; unsupported voices silently fall back
    /// to neutral speech, so it is safe to emit unconditionally. Populated from the
    /// standardized `TtsFeatures.emotion`.
    pub emotion: Option<String>,

    /// Full set of additional SSML knobs emitted into the synthesis body (pitch, volume,
    /// contour, range, styledegree, role, emphasis, say-as, phoneme, sub, lexicon,
    /// audioduration, silence, effect, ttsembedding, backgroundaudio). See
    /// [`AzureSsmlOptions`]. These reach the wire via `build_ssml_for_text` → `build_ssml_with_options`.
    pub ssml_options: AzureSsmlOptions,

    /// Optional custom-voice deployment id, sent as the `?deploymentId=...` query parameter on
    /// the synthesis URL. Populated from the `provider_extras` passthrough (`deploymentId`).
    pub deployment_id: Option<String>,

    /// Optional Entra / Bearer access token. When set, the request authenticates with
    /// `Authorization: Bearer <token>` instead of the `Ocp-Apim-Subscription-Key` header.
    /// Populated from the `provider_extras` passthrough (`auth_token`). This is a header-only
    /// credential and does NOT change the audio, so it is excluded from the cache-key hash.
    pub auth_token: Option<String>,

    /// Optional language override for the `<speak xml:lang>` attribute. When `None`,
    /// [`language_code`] derives the language from the voice name (the original behavior).
    /// Populated from the standardized `TtsFeatures.language`.
    ///
    /// [`language_code`]: AzureTTSConfig::language_code
    pub language_override: Option<String>,

    /// Optional endpoint base override redirecting the synthesis POST to a mock/proxy (W-T0).
    pub endpoint_override: Option<String>,
}

impl Default for AzureTTSConfig {
    fn default() -> Self {
        Self {
            base: TTSConfig::default(),
            region: AzureRegion::default(),
            output_format: AzureAudioEncoding::default(),
            use_ssml: true,
            emotion: None,
            ssml_options: AzureSsmlOptions::default(),
            deployment_id: None,
            auth_token: None,
            language_override: None,
            endpoint_override: None,
        }
    }
}

impl AzureTTSConfig {
    pub(crate) fn validate(&self) -> Result<(), TTSError> {
        self.validate_endpoint_override()?;
        self.validate_provider_fetched_urls()?;
        Ok(())
    }

    pub(crate) fn validate_endpoint_override(&self) -> Result<(), TTSError> {
        if let Some(endpoint) = self.endpoint_override.as_deref() {
            validate_azure_tts_endpoint("endpoint_override", endpoint)?;
        }
        Ok(())
    }

    pub(crate) fn validate_provider_fetched_urls(&self) -> Result<(), TTSError> {
        let options = &self.ssml_options;
        if let Some(url) = options.lexicon_uri.as_deref() {
            validate_azure_provider_url("lexicon_uri", url)?;
        }
        if let Some(url) = options.background_audio_src.as_deref() {
            validate_azure_provider_url("background_audio_src", url)?;
        }
        Ok(())
    }

    /// Creates an `AzureTTSConfig` from a base `TTSConfig` with default Azure settings.
    ///
    /// Maps the base configuration's audio format and sample rate to the
    /// appropriate Azure encoding.
    ///
    /// # Arguments
    ///
    /// * `base` - The base TTS configuration
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::{TTSConfig, azure::AzureTTSConfig};
    ///
    /// let base = TTSConfig {
    ///     voice_id: Some("en-US-JennyNeural".to_string()),
    ///     audio_format: Some("mp3".to_string()),
    ///     sample_rate: Some(24000),
    ///     ..Default::default()
    /// };
    ///
    /// let azure = AzureTTSConfig::from_base(base);
    /// ```
    pub fn from_base(base: TTSConfig) -> Self {
        let sample_rate = base.sample_rate.unwrap_or(24000);
        let output_format = base
            .audio_format
            .as_deref()
            .map(|f| AzureAudioEncoding::from_format_string(f, sample_rate))
            .unwrap_or_default();

        Self {
            base,
            region: AzureRegion::default(),
            output_format,
            use_ssml: true,
            emotion: None,
            ssml_options: AzureSsmlOptions::default(),
            deployment_id: None,
            auth_token: None,
            language_override: None,
            endpoint_override: None,
        }
    }

    /// Build from the standardized config (TTS W1 keystone). The `cognitiveservices/v1` REST synth
    /// endpoint is driven entirely by the SSML body plus four headers, so every Azure synthesis
    /// feature is expressed by emitting SSML (or, for `deploymentId`/auth, a URL/header). This maps:
    ///
    /// Typed [`TtsFeatures`] fields:
    /// - `ssml` → `use_ssml` flag
    /// - `speed` → `base.speaking_rate` → `<prosody rate>`
    /// - `emotion` → `<mstts:express-as style>`
    /// - `pitch` → `<prosody pitch>` (typed numeric → relative percentage, e.g. `+4%`)
    /// - `volume` → `<prosody volume>` (typed numeric → absolute level, e.g. `75`)
    /// - `language` → the `<speak xml:lang>` override (otherwise derived from the voice name)
    ///
    /// `provider_extras` passthrough (string values emitted verbatim into the SSML/URL/header):
    /// `region`, `style_degree`, `role`, `contour`, `range`, `emphasis`, `say_as_interpret_as`,
    /// `say_as_format`, `say_as_detail`, `phoneme_alphabet`, `phoneme_ph`, `sub_alias`,
    /// `lexicon_uri`, `audio_duration`, `silence_type`, `silence_value`, `effect`,
    /// `speaker_profile_id`, `background_audio_src` (+ `_volume`/`_fadein`/`_fadeout`),
    /// `deploymentId` (URL query), `auth_token` (Bearer header). Plus raw `pitch`/`volume` string
    /// overrides take precedence over the typed numerics when present.
    ///
    /// Document-structure SSML features that don't compose with wrapping a single plain-text body
    /// (multi-talker `mstts:dialog`, `mstts:markdown`, math `mstts:prompt`/MathML, and inserted
    /// recorded `<audio src>`) are intentionally NOT auto-emitted here — callers that need them
    /// pass full SSML with `use_ssml=false`-style raw bodies. See the per-field note in
    /// `from_standard` for the citation.
    pub fn from_standard(
        std: &crate::core::tts::standard::StandardTTSConfig,
    ) -> Result<Self, TTSError> {
        let f = &std.features;
        let extras = &std.extras.0;
        let mut cfg = Self::from_base(std.base.clone());
        if let Some(region_value) = extras.get("region") {
            let region = region_value.as_str().ok_or_else(|| {
                TTSError::InvalidConfiguration(
                    "Azure TTS provider_extras.region must be a string".to_string(),
                )
            })?;
            cfg.region = region.parse().map_err(|err| {
                TTSError::InvalidConfiguration(format!(
                    "Azure TTS provider_extras.region rejected: {err}"
                ))
            })?;
        }
        if let Some(s) = f.ssml {
            cfg.use_ssml = s;
        }
        // Map the standardized emotion into the Azure `mstts:express-as` style. This is wired
        // into the SSML body by `build_ssml_for_text` → `build_ssml_with_options`, which is the
        // only emotion wire vector Azure's `cognitiveservices/v1` REST synth endpoint exposes (it
        // consumes SSML; there is no URL/header param for emotion). express-as is voice-gated, but
        // unsupported voices silently ignore an unknown style and fall back to neutral speech
        // (Azure SSML reference, doc ms.date 2026-01-30), so passing it through is safe.
        if let Some(emotion) = f.emotion.as_ref().filter(|s| !s.is_empty()) {
            cfg.emotion = Some(emotion.clone());
        }
        // Fold the standardized speaking speed into the base rate so the SSML <prosody rate> path
        // actually applies it. (Review S4.)
        if let Some(speed) = f.speed {
            cfg.base.speaking_rate = Some(speed);
        }

        // ---- typed pitch/volume → <prosody pitch>/<prosody volume> ------------------------
        // Azure prosody pitch is a relative value; a bare numeric maps to a signed percentage
        // (`+4%`/`-3%`), matching Azure's documented relative-percentage form. A string `pitch`
        // extra (e.g. "high", "+2st", "600Hz") overrides the numeric if supplied.
        if let Some(pitch) = f.pitch.filter(|p| p.abs() > f32::EPSILON) {
            cfg.ssml_options.pitch = Some(format!("{:+}%", pitch.round() as i32));
        }
        // Azure prosody volume is an absolute level 0.0–100.0; map the numeric directly.
        if let Some(volume) = f.volume {
            cfg.ssml_options.volume = Some(format!("{volume}"));
        }
        // language → the document <speak xml:lang>. (Otherwise derived from the voice name.)
        // Carried through a dedicated field consumed in language_code().
        if let Some(language) = f.language.as_ref().filter(|s| !s.is_empty()) {
            cfg.language_override = Some(language.clone());
        }

        // ---- provider_extras string passthrough → AzureSsmlOptions ------------------------
        let opt = &mut cfg.ssml_options;
        let s = |k: &str| extras.get(k).and_then(|v| v.as_str()).map(str::to_string);
        // Raw pitch/volume string overrides win over the typed numerics above.
        if let Some(v) = s("pitch") {
            opt.pitch = Some(v);
        }
        if let Some(v) = s("volume") {
            opt.volume = Some(v);
        }
        opt.contour = s("contour").or(opt.contour.take());
        opt.range = s("range").or(opt.range.take());
        opt.style_degree = s("style_degree");
        opt.role = s("role");
        opt.emphasis = s("emphasis");
        opt.say_as_interpret_as = s("say_as_interpret_as");
        opt.say_as_format = s("say_as_format");
        opt.say_as_detail = s("say_as_detail");
        opt.phoneme_alphabet = s("phoneme_alphabet");
        opt.phoneme_ph = s("phoneme_ph");
        opt.sub_alias = s("sub_alias");
        opt.lexicon_uri = s("lexicon_uri");
        opt.audio_duration = s("audio_duration");
        opt.silence_type = s("silence_type");
        opt.silence_value = s("silence_value");
        opt.effect = s("effect");
        opt.speaker_profile_id = s("speaker_profile_id");
        opt.background_audio_src = s("background_audio_src");
        opt.background_audio_volume = s("background_audio_volume");
        opt.background_audio_fadein = s("background_audio_fadein");
        opt.background_audio_fadeout = s("background_audio_fadeout");

        // ---- URL / header credentials -----------------------------------------------------
        // deploymentId → ?deploymentId=... query param (custom-voice endpoint deployment).
        cfg.deployment_id = s("deploymentId").or_else(|| s("deployment_id"));
        // Entra / Bearer token → Authorization: Bearer header (alternative to subscription key).
        cfg.auth_token = s("auth_token");

        // NOTE (capability gaps): multi-talker `mstts:dialog`/`mstts:turn`, `mstts:markdown`, math
        // (`mstts:prompt domain="Math"` / MathML / `mstts:mathspeechverbosity`), and inserted
        // recorded `<audio src>` are document-structure SSML that replace/restructure the body
        // rather than wrap a single plain-text string, so they are not auto-emitted from a flat
        // text input here (Azure SSML reference, doc ms.date 2026-01-30). They remain reachable by
        // sending a full SSML body directly.

        // Endpoint override redirecting the synthesis POST to a mock/proxy (W-T0).
        cfg.endpoint_override = std.endpoint_override().map(String::from);

        Ok(cfg)
    }

    /// Creates an `AzureTTSConfig` with a specific region.
    ///
    /// # Arguments
    ///
    /// * `base` - The base TTS configuration
    /// * `region` - The Azure region to use
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::{TTSConfig, azure::AzureTTSConfig};
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let base = TTSConfig::default();
    /// let azure = AzureTTSConfig::with_region(base, AzureRegion::WestEurope);
    ///
    /// assert_eq!(azure.region, AzureRegion::WestEurope);
    /// ```
    pub fn with_region(base: TTSConfig, region: AzureRegion) -> Self {
        let mut config = Self::from_base(base);
        config.region = region;
        config
    }

    /// Builds the TTS synthesis endpoint URL for this configuration.
    ///
    /// Format: `https://{region}.tts.speech.microsoft.com/cognitiveservices/v1`
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::azure::AzureTTSConfig;
    /// use waav_gateway::core::providers::azure::AzureRegion;
    ///
    /// let config = AzureTTSConfig {
    ///     region: AzureRegion::WestEurope,
    ///     ..Default::default()
    /// };
    ///
    /// assert_eq!(
    ///     config.build_tts_url(),
    ///     "https://westeurope.tts.speech.microsoft.com/cognitiveservices/v1"
    /// );
    /// ```
    pub fn build_tts_url(&self) -> String {
        self.region.tts_rest_url()
    }

    /// Extracts the language code from the voice name.
    ///
    /// Azure voice names follow the pattern `{lang}-{region}-{name}Neural`,
    /// for example "en-US-JennyNeural" or "de-DE-KatjaNeural".
    ///
    /// # Returns
    ///
    /// The extracted language code, or "en-US" if extraction fails.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::{TTSConfig, azure::AzureTTSConfig};
    ///
    /// let config = AzureTTSConfig {
    ///     base: TTSConfig {
    ///         voice_id: Some("de-DE-KatjaNeural".to_string()),
    ///         ..Default::default()
    ///     },
    ///     ..Default::default()
    /// };
    ///
    /// assert_eq!(config.language_code(), "de-DE");
    /// ```
    pub fn language_code(&self) -> String {
        const DEFAULT_LANGUAGE: &str = "en-US";

        // An explicit standardized `language` override wins over voice-name derivation.
        if let Some(lang) = self.language_override.as_deref().filter(|s| !s.is_empty()) {
            return lang.to_string();
        }

        let voice_name = match &self.base.voice_id {
            Some(name) if !name.is_empty() => name,
            _ => return DEFAULT_LANGUAGE.to_string(),
        };

        // Split by '-' and try to extract language-region
        let parts: Vec<&str> = voice_name.split('-').collect();

        // Need at least 2 parts for language-region (e.g., "en-US")
        if parts.len() >= 2 {
            let first_part = parts[0];
            let second_part = parts[1];

            // If second part looks like a region code (2 uppercase letters), combine them
            if second_part.len() == 2 && second_part.chars().all(|c| c.is_ascii_uppercase()) {
                return format!("{first_part}-{second_part}");
            }
        }

        DEFAULT_LANGUAGE.to_string()
    }

    /// Returns the voice name for the API request.
    ///
    /// Checks `voice_id` first, falling back to `model` if `voice_id` is empty.
    /// Returns a default voice if both are empty.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::{TTSConfig, azure::AzureTTSConfig};
    ///
    /// let config = AzureTTSConfig {
    ///     base: TTSConfig {
    ///         voice_id: Some("en-US-JennyNeural".to_string()),
    ///         ..Default::default()
    ///     },
    ///     ..Default::default()
    /// };
    ///
    /// assert_eq!(config.voice_name(), "en-US-JennyNeural");
    /// ```
    pub fn voice_name(&self) -> &str {
        const DEFAULT_VOICE: &str = "en-US-JennyNeural";

        // Check voice_id first
        if let Some(voice_id) = &self.base.voice_id
            && !voice_id.is_empty()
        {
            return voice_id;
        }

        // Fall back to model
        if !self.base.model.is_empty() {
            return &self.base.model;
        }

        DEFAULT_VOICE
    }

    /// Builds the SSML document for a given text input.
    ///
    /// Uses the configuration's voice name, language, and speaking rate
    /// to construct a valid SSML document.
    ///
    /// # Arguments
    ///
    /// * `text` - The text to synthesize
    ///
    /// # Returns
    ///
    /// The SSML document as a string, or the original text if `use_ssml` is false.
    ///
    /// # Example
    ///
    /// ```rust
    /// use waav_gateway::core::tts::{TTSConfig, azure::AzureTTSConfig};
    ///
    /// let config = AzureTTSConfig {
    ///     base: TTSConfig {
    ///         voice_id: Some("en-US-JennyNeural".to_string()),
    ///         speaking_rate: Some(1.2),
    ///         ..Default::default()
    ///     },
    ///     use_ssml: true,
    ///     ..Default::default()
    /// };
    ///
    /// let ssml = config.build_ssml_for_text("Hello world!");
    /// assert!(ssml.contains("en-US-JennyNeural"));
    /// assert!(ssml.contains("rate=\"120%\""));
    /// ```
    pub fn build_ssml_for_text(&self, text: &str) -> String {
        if !self.use_ssml {
            return text.to_string();
        }

        build_ssml_with_options(
            text,
            self.voice_name(),
            &self.language_code(),
            self.base.speaking_rate,
            self.emotion.as_deref(),
            &self.ssml_options,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TTS W1 keystone: maps `ssml` → `use_ssml` and demonstrates provider_extras (region).
    #[test]
    fn from_standard_maps_ssml_and_region() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let mut extras = serde_json::Map::new();
        extras.insert("region".into(), serde_json::json!("westeurope"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "azure".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: TtsFeatures {
                ssml: Some(false),
                ..Default::default()
            },
            extras: crate::core::stt::standard::ProviderExtras(extras),
        };
        let cfg = AzureTTSConfig::from_standard(&std).unwrap();
        assert!(!cfg.use_ssml); // mapped from features.ssml
        assert_eq!(cfg.region, AzureRegion::WestEurope); // from provider_extras passthrough
        assert_eq!(cfg.base.api_key, "k"); // base carried through
    }

    #[test]
    fn from_standard_rejects_malformed_region_extra() {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};

        let mut extras = serde_json::Map::new();
        extras.insert("region".into(), serde_json::json!("west/europe"));
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "azure".into(),
                api_key: "k".into(),
                ..Default::default()
            },
            features: TtsFeatures::default(),
            extras: crate::core::stt::standard::ProviderExtras(extras),
        };

        let err = AzureTTSConfig::from_standard(&std)
            .expect_err("malformed Azure region extra must be rejected");
        assert!(
            err.to_string()
                .contains("Azure TTS provider_extras.region rejected"),
            "{err}"
        );
    }

    #[test]
    fn test_config_validation_rejects_ssrf_endpoint_override() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = AzureTTSConfig::from_base(TTSConfig {
            provider: "azure".into(),
            api_key: "k".into(),
            voice_id: Some("en-US-JennyNeural".to_string()),
            ..Default::default()
        });

        config.endpoint_override = Some("https://azure-proxy.example.com".to_string());
        assert!(config.validate_endpoint_override().is_ok());

        config.endpoint_override = Some("http://127.0.0.1:9000".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("loopback endpoint_override must be rejected");
        assert!(err.to_string().contains("SSRF protection"));

        config.endpoint_override = Some("file:///tmp/socket".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("file endpoint_override must be rejected");
        assert!(err.to_string().contains("URL scheme"));

        config.endpoint_override = Some("ws://azure-proxy.example.com".to_string());
        let err = config
            .validate_endpoint_override()
            .expect_err("WebSocket endpoint_override must be rejected for REST Azure");
        assert!(err.to_string().contains("URL scheme"));

        let std = crate::core::tts::standard::StandardTTSConfig::from_base(TTSConfig {
            provider: "azure".into(),
            api_key: "k".into(),
            voice_id: Some("en-US-JennyNeural".to_string()),
            ..Default::default()
        })
        .with_endpoint_override("file:///tmp/socket");
        let cfg = AzureTTSConfig::from_standard(&std).unwrap();
        assert!(cfg.validate_endpoint_override().is_err());
    }

    #[test]
    fn test_config_validation_rejects_ssrf_ssml_provider_urls() {
        let _env = crate::core::net::ssrf_env_lock();
        let mut config = AzureTTSConfig::from_base(TTSConfig {
            provider: "azure".into(),
            api_key: "k".into(),
            voice_id: Some("en-US-JennyNeural".to_string()),
            ..Default::default()
        });

        config.ssml_options.lexicon_uri = Some("https://example.com/lex.xml".to_string());
        config.ssml_options.background_audio_src = Some("https://example.com/bg.wav".to_string());
        assert!(config.validate_provider_fetched_urls().is_ok());

        config.ssml_options.lexicon_uri = Some("http://127.0.0.1:9000/lex.xml".to_string());
        let err = config
            .validate_provider_fetched_urls()
            .expect_err("loopback lexicon_uri must be rejected");
        assert!(err.to_string().contains("SSRF protection"));

        config.ssml_options.lexicon_uri = Some("file:///tmp/lex.xml".to_string());
        let err = config
            .validate_provider_fetched_urls()
            .expect_err("non-HTTP lexicon_uri must be rejected");
        assert!(err.to_string().contains("URL scheme"));

        config.ssml_options.lexicon_uri = Some("https://example.com/lex.xml".to_string());
        config.ssml_options.background_audio_src = Some("   ".to_string());
        let err = config
            .validate_provider_fetched_urls()
            .expect_err("blank background_audio_src must be rejected");
        assert!(err.to_string().contains("empty URL"));

        config.ssml_options.background_audio_src =
            Some("http://169.254.169.254/latest/meta-data".to_string());
        let err = config
            .validate_provider_fetched_urls()
            .expect_err("metadata background_audio_src must be rejected");
        assert!(err.to_string().contains("SSRF protection"));
    }

    // =========================================================================
    // AzureAudioEncoding Tests
    // =========================================================================

    #[test]
    fn test_audio_encoding_as_str() {
        // PCM formats
        assert_eq!(
            AzureAudioEncoding::Raw8Khz8BitMonoMulaw.as_str(),
            "raw-8khz-8bit-mono-mulaw"
        );
        assert_eq!(
            AzureAudioEncoding::Raw8Khz8BitMonoAlaw.as_str(),
            "raw-8khz-8bit-mono-alaw"
        );
        assert_eq!(
            AzureAudioEncoding::Raw8Khz16BitMonoPcm.as_str(),
            "raw-8khz-16bit-mono-pcm"
        );
        assert_eq!(
            AzureAudioEncoding::Raw16Khz16BitMonoPcm.as_str(),
            "raw-16khz-16bit-mono-pcm"
        );
        assert_eq!(
            AzureAudioEncoding::Raw22050Hz16BitMonoPcm.as_str(),
            "raw-22050hz-16bit-mono-pcm"
        );
        assert_eq!(
            AzureAudioEncoding::Raw24Khz16BitMonoPcm.as_str(),
            "raw-24khz-16bit-mono-pcm"
        );
        assert_eq!(
            AzureAudioEncoding::Raw44100Hz16BitMonoPcm.as_str(),
            "raw-44100hz-16bit-mono-pcm"
        );
        assert_eq!(
            AzureAudioEncoding::Raw48Khz16BitMonoPcm.as_str(),
            "raw-48khz-16bit-mono-pcm"
        );

        // MP3 formats
        assert_eq!(
            AzureAudioEncoding::Audio16Khz32KbitrateMonoMp3.as_str(),
            "audio-16khz-32kbitrate-mono-mp3"
        );
        assert_eq!(
            AzureAudioEncoding::Audio16Khz64KbitrateMonoMp3.as_str(),
            "audio-16khz-64kbitrate-mono-mp3"
        );
        assert_eq!(
            AzureAudioEncoding::Audio24Khz48KbitrateMonoMp3.as_str(),
            "audio-24khz-48kbitrate-mono-mp3"
        );
        assert_eq!(
            AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.as_str(),
            "audio-24khz-96kbitrate-mono-mp3"
        );
        assert_eq!(
            AzureAudioEncoding::Audio48Khz96KbitrateMonoMp3.as_str(),
            "audio-48khz-96kbitrate-mono-mp3"
        );
        assert_eq!(
            AzureAudioEncoding::Audio48Khz192KbitrateMonoMp3.as_str(),
            "audio-48khz-192kbitrate-mono-mp3"
        );

        // Opus formats
        assert_eq!(
            AzureAudioEncoding::Audio16Khz16Bit32KbpsMonoOpus.as_str(),
            "audio-16khz-16bit-32kbps-mono-opus"
        );
        assert_eq!(
            AzureAudioEncoding::Audio24Khz16Bit24KbpsMonoOpus.as_str(),
            "audio-24khz-16bit-24kbps-mono-opus"
        );
        assert_eq!(
            AzureAudioEncoding::Audio24Khz16Bit48KbpsMonoOpus.as_str(),
            "audio-24khz-16bit-48kbps-mono-opus"
        );
    }

    #[test]
    fn test_audio_encoding_sample_rate() {
        // 8kHz formats
        assert_eq!(AzureAudioEncoding::Raw8Khz8BitMonoMulaw.sample_rate(), 8000);
        assert_eq!(AzureAudioEncoding::Raw8Khz8BitMonoAlaw.sample_rate(), 8000);
        assert_eq!(AzureAudioEncoding::Raw8Khz16BitMonoPcm.sample_rate(), 8000);

        // 16kHz formats
        assert_eq!(
            AzureAudioEncoding::Raw16Khz16BitMonoPcm.sample_rate(),
            16000
        );
        assert_eq!(
            AzureAudioEncoding::Audio16Khz32KbitrateMonoMp3.sample_rate(),
            16000
        );
        assert_eq!(
            AzureAudioEncoding::Audio16Khz16Bit32KbpsMonoOpus.sample_rate(),
            16000
        );

        // 22.05kHz format
        assert_eq!(
            AzureAudioEncoding::Raw22050Hz16BitMonoPcm.sample_rate(),
            22050
        );

        // 24kHz formats
        assert_eq!(
            AzureAudioEncoding::Raw24Khz16BitMonoPcm.sample_rate(),
            24000
        );
        assert_eq!(
            AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.sample_rate(),
            24000
        );
        assert_eq!(
            AzureAudioEncoding::Audio24Khz16Bit48KbpsMonoOpus.sample_rate(),
            24000
        );

        // 44.1kHz format
        assert_eq!(
            AzureAudioEncoding::Raw44100Hz16BitMonoPcm.sample_rate(),
            44100
        );

        // 48kHz formats
        assert_eq!(
            AzureAudioEncoding::Raw48Khz16BitMonoPcm.sample_rate(),
            48000
        );
        assert_eq!(
            AzureAudioEncoding::Audio48Khz192KbitrateMonoMp3.sample_rate(),
            48000
        );
    }

    #[test]
    fn test_audio_encoding_is_pcm() {
        // PCM formats return true
        assert!(AzureAudioEncoding::Raw8Khz16BitMonoPcm.is_pcm());
        assert!(AzureAudioEncoding::Raw16Khz16BitMonoPcm.is_pcm());
        assert!(AzureAudioEncoding::Raw22050Hz16BitMonoPcm.is_pcm());
        assert!(AzureAudioEncoding::Raw24Khz16BitMonoPcm.is_pcm());
        assert!(AzureAudioEncoding::Raw44100Hz16BitMonoPcm.is_pcm());
        assert!(AzureAudioEncoding::Raw48Khz16BitMonoPcm.is_pcm());

        // Non-PCM formats return false
        assert!(!AzureAudioEncoding::Raw8Khz8BitMonoMulaw.is_pcm());
        assert!(!AzureAudioEncoding::Raw8Khz8BitMonoAlaw.is_pcm());
        assert!(!AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.is_pcm());
        assert!(!AzureAudioEncoding::Audio24Khz16Bit48KbpsMonoOpus.is_pcm());
    }

    #[test]
    fn test_audio_encoding_is_telephony() {
        assert!(AzureAudioEncoding::Raw8Khz8BitMonoMulaw.is_telephony());
        assert!(AzureAudioEncoding::Raw8Khz8BitMonoAlaw.is_telephony());

        assert!(!AzureAudioEncoding::Raw8Khz16BitMonoPcm.is_telephony());
        assert!(!AzureAudioEncoding::Raw24Khz16BitMonoPcm.is_telephony());
        assert!(!AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.is_telephony());
    }

    #[test]
    fn test_audio_encoding_content_type() {
        // PCM formats
        assert_eq!(
            AzureAudioEncoding::Raw24Khz16BitMonoPcm.content_type(),
            "audio/pcm"
        );
        assert_eq!(
            AzureAudioEncoding::Raw48Khz16BitMonoPcm.content_type(),
            "audio/pcm"
        );

        // Telephony formats
        assert_eq!(
            AzureAudioEncoding::Raw8Khz8BitMonoMulaw.content_type(),
            "audio/mulaw"
        );
        assert_eq!(
            AzureAudioEncoding::Raw8Khz8BitMonoAlaw.content_type(),
            "audio/alaw"
        );

        // MP3 formats
        assert_eq!(
            AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3.content_type(),
            "audio/mpeg"
        );
        assert_eq!(
            AzureAudioEncoding::Audio48Khz192KbitrateMonoMp3.content_type(),
            "audio/mpeg"
        );

        // Opus formats
        assert_eq!(
            AzureAudioEncoding::Audio24Khz16Bit48KbpsMonoOpus.content_type(),
            "audio/opus"
        );
    }

    #[test]
    fn test_audio_encoding_default() {
        assert_eq!(
            AzureAudioEncoding::default(),
            AzureAudioEncoding::Raw24Khz16BitMonoPcm
        );
    }

    #[test]
    fn test_audio_encoding_from_format_string() {
        // PCM formats
        assert_eq!(
            AzureAudioEncoding::from_format_string("linear16", 24000),
            AzureAudioEncoding::Raw24Khz16BitMonoPcm
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("pcm", 16000),
            AzureAudioEncoding::Raw16Khz16BitMonoPcm
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("wav", 48000),
            AzureAudioEncoding::Raw48Khz16BitMonoPcm
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("PCM", 8000),
            AzureAudioEncoding::Raw8Khz16BitMonoPcm
        );

        // MP3 formats
        assert_eq!(
            AzureAudioEncoding::from_format_string("mp3", 24000),
            AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("MP3", 16000),
            AzureAudioEncoding::Audio16Khz64KbitrateMonoMp3
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("mp3", 48000),
            AzureAudioEncoding::Audio48Khz192KbitrateMonoMp3
        );

        // Telephony formats
        assert_eq!(
            AzureAudioEncoding::from_format_string("mulaw", 8000),
            AzureAudioEncoding::Raw8Khz8BitMonoMulaw
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("ulaw", 8000),
            AzureAudioEncoding::Raw8Khz8BitMonoMulaw
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("alaw", 8000),
            AzureAudioEncoding::Raw8Khz8BitMonoAlaw
        );

        // Opus formats
        assert_eq!(
            AzureAudioEncoding::from_format_string("opus", 24000),
            AzureAudioEncoding::Audio24Khz16Bit48KbpsMonoOpus
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("opus", 16000),
            AzureAudioEncoding::Audio16Khz16Bit32KbpsMonoOpus
        );

        // Unknown format defaults
        assert_eq!(
            AzureAudioEncoding::from_format_string("unknown", 24000),
            AzureAudioEncoding::Raw24Khz16BitMonoPcm
        );
        assert_eq!(
            AzureAudioEncoding::from_format_string("", 24000),
            AzureAudioEncoding::Raw24Khz16BitMonoPcm
        );
    }

    // =========================================================================
    // SSML Generation Tests
    // =========================================================================

    #[test]
    fn test_escape_xml() {
        assert_eq!(escape_xml("Hello world"), "Hello world");
        assert_eq!(escape_xml("Hello & goodbye"), "Hello &amp; goodbye");
        assert_eq!(escape_xml("<script>"), "&lt;script&gt;");
        assert_eq!(escape_xml("He said \"hi\""), "He said &quot;hi&quot;");
        assert_eq!(escape_xml("It's nice"), "It&apos;s nice");
        assert_eq!(
            escape_xml("<a href=\"test\">link</a>"),
            "&lt;a href=&quot;test&quot;&gt;link&lt;/a&gt;"
        );
        assert_eq!(escape_xml(""), "");
    }

    #[test]
    fn test_build_ssml_basic() {
        let ssml = build_ssml("Hello world!", "en-US-JennyNeural", "en-US", None, None);

        assert!(ssml.contains("<speak"));
        assert!(ssml.contains("version='1.0'"));
        assert!(ssml.contains("xmlns='http://www.w3.org/2001/10/synthesis'"));
        assert!(ssml.contains("xml:lang='en-US'"));
        assert!(ssml.contains("<voice name='en-US-JennyNeural'>"));
        assert!(ssml.contains("Hello world!"));
        assert!(ssml.contains("</voice>"));
        assert!(ssml.contains("</speak>"));
        assert!(!ssml.contains("<prosody"));
        // No emotion → no express-as wrapper and no mstts namespace.
        assert!(!ssml.contains("mstts:express-as"));
        assert!(!ssml.contains("xmlns:mstts"));
    }

    #[test]
    fn test_build_ssml_with_speaking_rate() {
        let ssml = build_ssml("Fast speech", "en-US-JennyNeural", "en-US", Some(1.5), None);

        assert!(ssml.contains("<prosody rate=\"150%\">"));
        assert!(ssml.contains("Fast speech"));
        assert!(ssml.contains("</prosody>"));
    }

    #[test]
    fn test_build_ssml_with_slow_rate() {
        let ssml = build_ssml(
            "Slow speech",
            "en-US-JennyNeural",
            "en-US",
            Some(0.75),
            None,
        );

        assert!(ssml.contains("<prosody rate=\"75%\">"));
        assert!(ssml.contains("Slow speech"));
    }

    #[test]
    fn test_build_ssml_with_normal_rate() {
        // Rate of exactly 1.0 should not add prosody
        let ssml = build_ssml(
            "Normal speech",
            "en-US-JennyNeural",
            "en-US",
            Some(1.0),
            None,
        );

        assert!(!ssml.contains("<prosody"));
        assert!(ssml.contains("Normal speech"));
    }

    #[test]
    fn test_build_ssml_escapes_special_chars() {
        let ssml = build_ssml(
            "Hello <user> & welcome!",
            "en-US-JennyNeural",
            "en-US",
            None,
            None,
        );

        assert!(ssml.contains("Hello &lt;user&gt; &amp; welcome!"));
        assert!(!ssml.contains("<user>"));
    }

    #[test]
    fn test_build_ssml_different_language() {
        let ssml = build_ssml("Guten Tag!", "de-DE-KatjaNeural", "de-DE", None, None);

        assert!(ssml.contains("xml:lang='de-DE'"));
        assert!(ssml.contains("<voice name='de-DE-KatjaNeural'>"));
    }

    // Emotion wire-level (SSML body) tests for mstts:express-as.
    // Confirmed against the Azure Speech SSML reference (speech-synthesis-markup-voice,
    // doc ms.date 2026-01-30): emotion is expressed via `<mstts:express-as style="...">`
    // inside `<voice>`, requiring the `xmlns:mstts` namespace on `<speak>`. The
    // cognitiveservices/v1 REST synth endpoint consumes SSML, so this body IS the wire.

    #[test]
    fn test_build_ssml_with_emotion_express_as() {
        let ssml = build_ssml("Yay!", "en-US-JennyNeural", "en-US", None, Some("cheerful"));

        // Wire assertion: the express-as style attribute appears in the serialized SSML body.
        assert!(
            ssml.contains("<mstts:express-as style=\"cheerful\">"),
            "express-as wrapper missing from SSML body: {ssml}"
        );
        assert!(ssml.contains("</mstts:express-as>"));
        // Namespace must be declared on <speak> for mstts:* elements to be valid.
        assert!(
            ssml.contains("xmlns:mstts='https://www.w3.org/2001/mstts'"),
            "mstts namespace missing: {ssml}"
        );
        // express-as wraps the spoken text, inside <voice>.
        assert!(ssml.contains("<mstts:express-as style=\"cheerful\">Yay!</mstts:express-as>"));
    }

    #[test]
    fn test_build_ssml_emotion_wraps_prosody() {
        // Emotion + speaking rate: express-as wraps the prosody element (style applies to
        // the whole rate-adjusted span).
        let ssml = build_ssml(
            "Slow and sad",
            "en-US-JennyNeural",
            "en-US",
            Some(0.8),
            Some("sad"),
        );

        assert!(ssml.contains("<mstts:express-as style=\"sad\">"));
        assert!(ssml.contains("<prosody rate=\"80%\">"));
        // express-as is the outer wrapper, prosody nested inside it.
        let express_idx = ssml.find("<mstts:express-as").unwrap();
        let prosody_idx = ssml.find("<prosody").unwrap();
        assert!(
            express_idx < prosody_idx,
            "express-as should wrap prosody: {ssml}"
        );
    }

    #[test]
    fn test_build_ssml_emotion_escapes_style_attr() {
        // A hostile style value must not break SSML well-formedness.
        let ssml = build_ssml("text", "en-US-JennyNeural", "en-US", None, Some("a\"b"));
        assert!(!ssml.contains("style=\"a\"b\""));
        assert!(ssml.contains("&quot;"));
    }

    #[test]
    fn test_build_ssml_empty_emotion_omits_express_as() {
        // An empty-string style would be ignored by Azure anyway; omit the wrapper entirely.
        let ssml = build_ssml("text", "en-US-JennyNeural", "en-US", None, Some(""));
        assert!(!ssml.contains("mstts:express-as"));
        assert!(!ssml.contains("xmlns:mstts"));
    }

    #[test]
    fn test_build_ssml_for_text_emits_emotion_from_config() {
        // The provider-facing path (build_ssml_for_text) must carry the config emotion into
        // the SSML body, not just the build_ssml free function.
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some("en-US-JennyNeural".to_string()),
                ..Default::default()
            },
            use_ssml: true,
            emotion: Some("excited".to_string()),
            ..Default::default()
        };

        let ssml = config.build_ssml_for_text("Big news");
        assert!(ssml.contains("<mstts:express-as style=\"excited\">"));
        assert!(ssml.contains("xmlns:mstts="));
        assert!(ssml.contains("Big news"));
    }

    #[test]
    fn from_standard_maps_emotion_to_express_as_in_ssml() {
        // RED-class guard: prove features.emotion reaches the serialized SSML body via the
        // standardized dispatch, not just that it lands in a config struct field.
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "azure".into(),
                api_key: "k".into(),
                voice_id: Some("en-US-JennyNeural".into()),
                ..Default::default()
            },
            features: TtsFeatures {
                emotion: Some("cheerful".into()),
                ..Default::default()
            },
            extras: crate::core::stt::standard::ProviderExtras(serde_json::Map::new()),
        };
        let cfg = AzureTTSConfig::from_standard(&std).unwrap();
        assert_eq!(cfg.emotion.as_deref(), Some("cheerful"));
        let ssml = cfg.build_ssml_for_text("Hello");
        assert!(
            ssml.contains("<mstts:express-as style=\"cheerful\">"),
            "emotion not wired into SSML body: {ssml}"
        );
        assert!(ssml.contains("xmlns:mstts="));
    }

    // =========================================================================
    // Wired SSML feature tests (build_ssml_with_options + from_standard → SSML body)
    //
    // The cognitiveservices/v1 REST synth endpoint consumes SSML as the body, so the SERIALIZED
    // SSML IS the wire. Each test asserts the documented attribute appears in the SSML string.
    // Element/attribute syntax confirmed against the Azure Speech SSML reference (doc ms.date
    // 2026-01-30, speech-synthesis-markup-voice / -pronunciation).
    // =========================================================================

    fn opts_std(
        build: impl FnOnce(
            &mut crate::core::tts::standard::TtsFeatures,
            &mut serde_json::Map<String, serde_json::Value>,
        ),
    ) -> AzureTTSConfig {
        use crate::core::tts::standard::{StandardTTSConfig, TtsFeatures};
        let mut features = TtsFeatures::default();
        let mut extras = serde_json::Map::new();
        build(&mut features, &mut extras);
        let std = StandardTTSConfig {
            base: TTSConfig {
                provider: "azure".into(),
                api_key: "k".into(),
                voice_id: Some("en-US-JennyNeural".into()),
                ..Default::default()
            },
            features,
            extras: crate::core::stt::standard::ProviderExtras(extras),
        };
        AzureTTSConfig::from_standard(&std).unwrap()
    }

    #[test]
    fn from_standard_typed_pitch_volume_reach_prosody() {
        let cfg = opts_std(|f, _| {
            f.pitch = Some(4.0);
            f.volume = Some(75.0);
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(
            ssml.contains("pitch=\"+4%\""),
            "pitch not in prosody: {ssml}"
        );
        assert!(
            ssml.contains("volume=\"75\""),
            "volume not in prosody: {ssml}"
        );
        assert!(ssml.contains("<prosody"));
    }

    #[test]
    fn from_standard_typed_language_overrides_speak_lang() {
        let cfg = opts_std(|f, _| f.language = Some("fr-FR".into()));
        assert_eq!(cfg.language_code(), "fr-FR");
        let ssml = cfg.build_ssml_for_text("Bonjour");
        assert!(
            ssml.contains("xml:lang='fr-FR'"),
            "lang override missing: {ssml}"
        );
    }

    #[test]
    fn from_standard_extras_prosody_contour_range() {
        let cfg = opts_std(|_, e| {
            e.insert(
                "contour".into(),
                serde_json::json!("(0%,+20Hz) (100%,-10Hz)"),
            );
            e.insert("range".into(), serde_json::json!("+20%"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(
            ssml.contains("contour=\"(0%,+20Hz) (100%,-10Hz)\""),
            "{ssml}"
        );
        assert!(ssml.contains("range=\"+20%\""), "{ssml}");
    }

    #[test]
    fn from_standard_extras_express_as_styledegree_role() {
        let cfg = opts_std(|f, e| {
            f.emotion = Some("sad".into());
            e.insert("style_degree".into(), serde_json::json!("2"));
            e.insert("role".into(), serde_json::json!("YoungAdultFemale"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(ssml.contains("style=\"sad\""), "{ssml}");
        assert!(ssml.contains("styledegree=\"2\""), "{ssml}");
        assert!(ssml.contains("role=\"YoungAdultFemale\""), "{ssml}");
        assert!(ssml.contains("xmlns:mstts="));
    }

    #[test]
    fn from_standard_extras_styledegree_role_without_emotion_still_emit_express_as() {
        // styledegree/role are express-as attributes; they require the wrapper even with no style.
        let cfg = opts_std(|_, e| {
            e.insert("role".into(), serde_json::json!("Boy"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(ssml.contains("<mstts:express-as role=\"Boy\">"), "{ssml}");
        assert!(ssml.contains("xmlns:mstts="));
    }

    #[test]
    fn from_standard_extras_emphasis() {
        let cfg = opts_std(|_, e| {
            e.insert("emphasis".into(), serde_json::json!("strong"));
        });
        let ssml = cfg.build_ssml_for_text("meetings");
        assert!(ssml.contains("<emphasis level=\"strong\">"), "{ssml}");
    }

    #[test]
    fn from_standard_extras_say_as() {
        let cfg = opts_std(|_, e| {
            e.insert("say_as_interpret_as".into(), serde_json::json!("date"));
            e.insert("say_as_format".into(), serde_json::json!("mdy"));
            e.insert("say_as_detail".into(), serde_json::json!("1"));
        });
        let ssml = cfg.build_ssml_for_text("10/19/2010");
        assert!(
            ssml.contains("<say-as interpret-as=\"date\" format=\"mdy\" detail=\"1\">"),
            "{ssml}"
        );
    }

    #[test]
    fn from_standard_extras_phoneme() {
        let cfg = opts_std(|_, e| {
            e.insert("phoneme_alphabet".into(), serde_json::json!("ipa"));
            e.insert("phoneme_ph".into(), serde_json::json!("təˈmeɪtoʊ"));
        });
        let ssml = cfg.build_ssml_for_text("tomato");
        assert!(
            ssml.contains("<phoneme alphabet=\"ipa\" ph=\"təˈmeɪtoʊ\">"),
            "{ssml}"
        );
    }

    #[test]
    fn from_standard_extras_sub_alias() {
        let cfg = opts_std(|_, e| {
            e.insert(
                "sub_alias".into(),
                serde_json::json!("World Wide Web Consortium"),
            );
        });
        let ssml = cfg.build_ssml_for_text("W3C");
        assert!(
            ssml.contains("<sub alias=\"World Wide Web Consortium\">W3C</sub>"),
            "{ssml}"
        );
    }

    #[test]
    fn from_standard_extras_lexicon_and_audioduration_and_silence() {
        let cfg = opts_std(|_, e| {
            e.insert(
                "lexicon_uri".into(),
                serde_json::json!("https://x.example/lex.xml"),
            );
            e.insert("audio_duration".into(), serde_json::json!("20s"));
            e.insert("silence_type".into(), serde_json::json!("Sentenceboundary"));
            e.insert("silence_value".into(), serde_json::json!("200ms"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(
            ssml.contains("<lexicon uri=\"https://x.example/lex.xml\"/>"),
            "{ssml}"
        );
        assert!(
            ssml.contains("<mstts:audioduration value=\"20s\"/>"),
            "{ssml}"
        );
        assert!(
            ssml.contains("<mstts:silence type=\"Sentenceboundary\" value=\"200ms\"/>"),
            "{ssml}"
        );
        assert!(ssml.contains("xmlns:mstts="));
    }

    #[test]
    fn from_standard_extras_voice_effect() {
        let cfg = opts_std(|_, e| {
            e.insert("effect".into(), serde_json::json!("eq_car"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(
            ssml.contains("<voice name='en-US-JennyNeural' effect=\"eq_car\">"),
            "{ssml}"
        );
    }

    #[test]
    fn from_standard_extras_ttsembedding_speaker_profile() {
        let cfg = opts_std(|_, e| {
            e.insert(
                "speaker_profile_id".into(),
                serde_json::json!("profile-123"),
            );
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(
            ssml.contains("<mstts:ttsembedding speakerProfileId=\"profile-123\">"),
            "{ssml}"
        );
        assert!(ssml.contains("xmlns:mstts="));
    }

    #[test]
    fn from_standard_extras_background_audio() {
        let cfg = opts_std(|_, e| {
            e.insert(
                "background_audio_src".into(),
                serde_json::json!("https://x.example/bg.wav"),
            );
            e.insert("background_audio_volume".into(), serde_json::json!("0.7"));
            e.insert("background_audio_fadein".into(), serde_json::json!("3000"));
            e.insert("background_audio_fadeout".into(), serde_json::json!("4000"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(
            ssml.contains("<mstts:backgroundaudio src=\"https://x.example/bg.wav\" volume=\"0.7\" fadein=\"3000\" fadeout=\"4000\"/>"),
            "{ssml}"
        );
        assert!(ssml.contains("xmlns:mstts="));
    }

    #[test]
    fn from_standard_extras_pitch_string_overrides_typed_numeric() {
        // A raw `pitch` string extra (e.g. "high") wins over the typed numeric.
        let cfg = opts_std(|f, e| {
            f.pitch = Some(4.0);
            e.insert("pitch".into(), serde_json::json!("high"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(ssml.contains("pitch=\"high\""), "{ssml}");
        assert!(!ssml.contains("pitch=\"+4%\""), "{ssml}");
    }

    #[test]
    fn from_standard_hostile_attr_values_stay_well_formed() {
        // A malicious extra must not break SSML well-formedness (attribute escaping).
        let cfg = opts_std(|_, e| {
            e.insert("effect".into(), serde_json::json!("a\"><inject/>"));
        });
        let ssml = cfg.build_ssml_for_text("Hi");
        assert!(!ssml.contains("<inject/>"), "injection not escaped: {ssml}");
        assert!(ssml.contains("&quot;") || ssml.contains("&lt;"), "{ssml}");
    }

    #[test]
    fn defaults_reproduce_minimal_ssml() {
        // An all-default options set must produce exactly the original minimal SSML shape.
        let ssml = build_ssml_with_options(
            "Hello",
            "en-US-JennyNeural",
            "en-US",
            None,
            None,
            &AzureSsmlOptions::default(),
        );
        assert!(ssml.contains("<voice name='en-US-JennyNeural'>"));
        assert!(!ssml.contains("<prosody"));
        assert!(!ssml.contains("mstts:"));
        assert!(!ssml.contains("xmlns:mstts"));
        assert!(ssml.contains("Hello"));
    }

    // =========================================================================
    // AzureTTSConfig Tests
    // =========================================================================

    #[test]
    fn test_azure_tts_config_default() {
        let config = AzureTTSConfig::default();

        assert_eq!(config.region, AzureRegion::EastUS);
        assert_eq!(
            config.output_format,
            AzureAudioEncoding::Raw24Khz16BitMonoPcm
        );
        assert!(config.use_ssml);
    }

    #[test]
    fn test_azure_tts_config_from_base() {
        let base = TTSConfig {
            voice_id: Some("en-US-JennyNeural".to_string()),
            audio_format: Some("mp3".to_string()),
            sample_rate: Some(24000),
            speaking_rate: Some(1.2),
            ..Default::default()
        };

        let config = AzureTTSConfig::from_base(base);

        assert_eq!(
            config.output_format,
            AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3
        );
        assert_eq!(config.region, AzureRegion::EastUS);
        assert!(config.use_ssml);
    }

    #[test]
    fn test_azure_tts_config_with_region() {
        let base = TTSConfig::default();
        let config = AzureTTSConfig::with_region(base, AzureRegion::WestEurope);

        assert_eq!(config.region, AzureRegion::WestEurope);
    }

    #[test]
    fn test_azure_tts_config_build_url() {
        let config = AzureTTSConfig {
            region: AzureRegion::WestEurope,
            ..Default::default()
        };

        assert_eq!(
            config.build_tts_url(),
            "https://westeurope.tts.speech.microsoft.com/cognitiveservices/v1"
        );
    }

    #[test]
    fn test_azure_tts_config_build_url_various_regions() {
        let test_cases = vec![
            (AzureRegion::EastUS, "eastus"),
            (AzureRegion::WestEurope, "westeurope"),
            (AzureRegion::JapanEast, "japaneast"),
            (AzureRegion::SoutheastAsia, "southeastasia"),
        ];

        for (region, region_str) in test_cases {
            let config = AzureTTSConfig {
                region,
                ..Default::default()
            };

            let expected = format!(
                "https://{}.tts.speech.microsoft.com/cognitiveservices/v1",
                region_str
            );
            assert_eq!(config.build_tts_url(), expected);
        }
    }

    #[test]
    fn test_azure_tts_config_language_code() {
        // Standard voice name
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some("en-US-JennyNeural".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.language_code(), "en-US");

        // German voice
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some("de-DE-KatjaNeural".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.language_code(), "de-DE");

        // Empty voice_id defaults to en-US
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.language_code(), "en-US");

        // Invalid format defaults to en-US
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some("invalid".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.language_code(), "en-US");
    }

    #[test]
    fn test_azure_tts_config_voice_name() {
        // Has voice_id
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some("en-US-JennyNeural".to_string()),
                model: "fallback-model".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.voice_name(), "en-US-JennyNeural");

        // Empty voice_id falls back to model
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some(String::new()),
                model: "en-US-AriaNeural".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.voice_name(), "en-US-AriaNeural");

        // None voice_id falls back to model
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: None,
                model: "en-US-AriaNeural".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.voice_name(), "en-US-AriaNeural");

        // Both empty defaults to JennyNeural
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: None,
                model: String::new(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.voice_name(), "en-US-JennyNeural");
    }

    #[test]
    fn test_azure_tts_config_build_ssml_for_text() {
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some("en-US-JennyNeural".to_string()),
                speaking_rate: Some(1.2),
                ..Default::default()
            },
            use_ssml: true,
            ..Default::default()
        };

        let ssml = config.build_ssml_for_text("Hello world!");

        assert!(ssml.contains("<speak"));
        assert!(ssml.contains("en-US-JennyNeural"));
        assert!(ssml.contains("xml:lang='en-US'"));
        assert!(ssml.contains("rate=\"120%\""));
        assert!(ssml.contains("Hello world!"));
    }

    #[test]
    fn test_azure_tts_config_build_ssml_disabled() {
        let config = AzureTTSConfig {
            base: TTSConfig {
                voice_id: Some("en-US-JennyNeural".to_string()),
                ..Default::default()
            },
            use_ssml: false,
            ..Default::default()
        };

        let result = config.build_ssml_for_text("Hello world!");

        assert_eq!(result, "Hello world!");
        assert!(!result.contains("<speak"));
    }

    #[test]
    fn test_azure_tts_config_serialization() {
        let config = AzureTTSConfig {
            base: TTSConfig::default(),
            region: AzureRegion::WestEurope,
            output_format: AzureAudioEncoding::Audio24Khz96KbitrateMonoMp3,
            use_ssml: true,
            emotion: None,
            ..Default::default()
        };

        // The output_format should be serializable
        let json = serde_json::to_string(&config.output_format).expect("Failed to serialize");
        assert!(json.contains("Audio24Khz96KbitrateMonoMp3"));
    }
}
