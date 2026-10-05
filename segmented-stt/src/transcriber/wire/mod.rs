//! Request formats, one per vendor family.
//!
//! Each family is one [`SegmentTranscriber`](super::SegmentTranscriber) built from a plain config
//! struct, so this module does not depend on the capability map: the plan step reads the row and
//! fills the config. What differs between vendors of one family (the URL, the credential header,
//! which optional fields go out) comes from the config, never from the provider's name.
//!
//! | Family | Adapter ids |
//! | --- | --- |
//! | [`openai_compat`] | `openai_transcriptions`, `groq_transcriptions`, `azure_openai_transcriptions` |
//! | [`elevenlabs`] | `elevenlabs_batch` |
//! | [`deepgram`] | `deepgram_prerecorded` |
//! | [`assemblyai`] | `assemblyai_sync` |
//! | [`azure_fast`] | `azure_fast_transcription` |
//! | [`google_recognize`] | `google_recognize` |
//! | [`openai_realtime`] | `openai_realtime_transcription` (the commit transport, on a socket) |
//!
//! Shared rules, applied by every family through this module:
//!
//! - One call is one request. Nothing here retries, waits, or touches a breaker or a limiter.
//! - An optional field is skipped when [`SegmentContext::minimal`] is set or when it is named in
//!   [`SegmentContext::omit_fields`]; every optional field a family can send is listed in its
//!   [`TranscriberInfo::droppable_fields`]. A 400 or 422 that names one of the optional fields
//!   actually sent comes back as [`SegmentError::refused_field`].
//! - Detected languages are reported as ISO 639-1 when the code or name is recognised, so the
//!   session language vote compares like with like across vendors.
//! - A credential never appears in an error message: headers carrying one are marked sensitive,
//!   and vendor error text that echoes it is redacted.

pub mod assemblyai;
pub mod azure_fast;
pub mod deepgram;
pub mod elevenlabs;
pub mod google_recognize;
pub mod openai_compat;
pub mod openai_realtime;
#[cfg(test)]
pub(crate) mod testkit;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::{Bytes, BytesMut};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

pub use assemblyai::{AssemblyAiSyncConfig, AssemblyAiSyncTranscriber};
pub use azure_fast::{AzureEnhancedMode, AzureFastConfig, AzureFastTranscriber};
pub use deepgram::{DeepgramPrerecordedConfig, DeepgramPrerecordedTranscriber};
pub use elevenlabs::{ElevenLabsConfig, ElevenLabsTranscriber};
pub use google_recognize::{GoogleRecognizeConfig, GoogleRecognizeTranscriber};
pub use openai_compat::{LanguageDialect, OpenAiCompatConfig, OpenAiCompatTranscriber};
pub use openai_realtime::{OpenAiRealtimeConfig, OpenAiRealtimeTranscriber};

use super::{
    RequestPhase, RequestProgress, SegmentAudio, SegmentContext, SegmentError, TranscriberInfo,
};
use crate::types::ErrorClass;

/// The adapter ids this crate can construct a transcriber for. The resolver must never name an
/// adapter outside this list.
pub fn built_adapters() -> &'static [&'static str] {
    &[
        "openai_transcriptions",
        "groq_transcriptions",
        "azure_openai_transcriptions",
        "elevenlabs_batch",
        "deepgram_prerecorded",
        "assemblyai_sync",
        "azure_fast_transcription",
        "google_recognize",
        "openai_realtime_transcription",
    ]
}

/// The largest response body read. A transcript of a 25 s segment is a few kilobytes; a host
/// that streams more is broken or hostile, and an unbounded read is a memory hazard.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// How the credential travels. No `Debug` of the secret: the derived output would print it.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
    /// `Authorization: Bearer <secret>`; no header at all for an empty secret, because a keyless
    /// in-cluster server is normal and an empty bearer is a malformed credential some refuse.
    Bearer(String),
    /// Azure OpenAI: `api-key: <secret>`, never `Authorization`.
    AzureApiKey(String),
    /// A vendor's own header, optionally with a scheme word before the secret.
    Header {
        name: &'static str,
        scheme: Option<&'static str>,
        secret: String,
    },
    None,
}

impl Auth {
    /// `xi-api-key: <key>`.
    pub fn elevenlabs(key: impl Into<String>) -> Self {
        Self::Header {
            name: "xi-api-key",
            scheme: None,
            secret: key.into(),
        }
    }

    /// `Authorization: Token <key>`. A Deepgram JWT goes as [`Auth::Bearer`] instead.
    pub fn deepgram(key: impl Into<String>) -> Self {
        Self::Header {
            name: "authorization",
            scheme: Some("Token"),
            secret: key.into(),
        }
    }

    /// `Authorization: <key>`, the bare key AssemblyAI documents.
    pub fn assemblyai(key: impl Into<String>) -> Self {
        Self::Header {
            name: "authorization",
            scheme: None,
            secret: key.into(),
        }
    }

    /// `Ocp-Apim-Subscription-Key: <key>`. An Entra token goes as [`Auth::Bearer`] instead.
    pub fn azure_speech(key: impl Into<String>) -> Self {
        Self::Header {
            name: "ocp-apim-subscription-key",
            scheme: None,
            secret: key.into(),
        }
    }

    pub fn secret(&self) -> Option<&str> {
        match self {
            Self::Bearer(s) | Self::AzureApiKey(s) | Self::Header { secret: s, .. } => {
                Some(s.as_str()).filter(|s| !s.is_empty())
            }
            Self::None => None,
        }
    }

    /// Attaches the credential, marked sensitive so it never renders in a `Debug`. A secret with
    /// bytes a header cannot carry is refused here, with a message that does not include it.
    pub(crate) fn apply(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder, SegmentError> {
        let (name, value) = match self {
            Self::None => return Ok(req),
            Self::Bearer(s) if s.is_empty() => return Ok(req),
            Self::Bearer(s) => ("authorization", format!("Bearer {s}")),
            Self::AzureApiKey(s) => ("api-key", s.clone()),
            Self::Header { secret, .. } if secret.is_empty() => return Ok(req),
            Self::Header {
                name,
                scheme: Some(scheme),
                secret,
            } => (*name, format!("{scheme} {secret}")),
            Self::Header {
                name,
                scheme: None,
                secret,
            } => (*name, secret.clone()),
        };
        let mut value = HeaderValue::from_str(&value).map_err(|_| {
            SegmentError::new(
                ErrorClass::Internal,
                "the credential holds bytes an HTTP header cannot carry",
            )
            .with_phase(RequestPhase::BeforeSend)
        })?;
        value.set_sensitive(true);
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            SegmentError::new(
                ErrorClass::Internal,
                format!("{name:?} is not an HTTP header name"),
            )
            .with_phase(RequestPhase::BeforeSend)
        })?;
        Ok(req.header(name, value))
    }
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self {
            Self::Bearer(_) => "Bearer",
            Self::AzureApiKey(_) => "AzureApiKey",
            Self::Header { name, .. } => name,
            Self::None => return f.write_str("Auth::None"),
        };
        write!(f, "Auth::{kind}(<redacted>)")
    }
}

/// The capability row's limits for one transport, copied into [`TranscriberInfo`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RowLimits {
    pub min_audio_ms: Option<u32>,
    pub max_audio_ms: Option<u32>,
    pub max_upload_bytes: Option<u64>,
    /// A server that runs one request at a time (some self-hosted servers).
    pub single_process_server: bool,
}

/// The code system a vendor expects for the language.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LanguageFormat {
    /// The two-letter primary subtag (`en-US` becomes `en`).
    #[default]
    Iso639_1,
    /// A BCP-47 tag in canonical case (`en_us` becomes `en-US`).
    Bcp47,
}

pub(crate) fn info_for(
    adapter: &str,
    url: &str,
    model: &str,
    limits: RowLimits,
    droppable: Vec<String>,
) -> TranscriberInfo {
    let mut info = TranscriberInfo::file(adapter, &super::http::host_key(url), model);
    info.min_audio_ms = limits.min_audio_ms;
    info.max_audio_ms = limits.max_audio_ms;
    info.max_upload_bytes = limits.max_upload_bytes;
    info.single_process_server = limits.single_process_server;
    let mut seen = Vec::new();
    for d in droppable {
        if !seen.contains(&d) {
            seen.push(d);
        }
    }
    info.droppable_fields = seen;
    info
}

/// Checks a URL a family was configured with, so a typo fails at plan time and not per utterance.
pub(crate) fn checked_url(url: &str) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url.trim()).map_err(|e| format!("{url:?} is not a URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!("{url:?} must be an http or https URL with a host"));
    }
    Ok(parsed)
}

/// `base` joined with `path`, keeping any path prefix the base already has.
pub(crate) fn join_url(base: &str, path: &str) -> Result<String, String> {
    checked_url(base)?;
    Ok(format!(
        "{}/{}",
        base.trim().trim_end_matches('/'),
        path.trim_start_matches('/')
    ))
}

/// Refuses a unit the row says the vendor would refuse, before anything is sent.
pub(crate) fn check_limits(
    info: &TranscriberInfo,
    audio: &SegmentAudio,
    upload_bytes: usize,
) -> Result<(), SegmentError> {
    let ms = audio.audio_ms();
    let refuse = |what: String| {
        Err(SegmentError::new(
            ErrorClass::BadRequest,
            format!("refused before sending: {what}"),
        )
        .with_phase(RequestPhase::BeforeSend))
    };
    if let Some(min) = info.min_audio_ms.filter(|min| ms < *min) {
        return refuse(format!(
            "{ms} ms of audio is below the vendor minimum of {min} ms"
        ));
    }
    if let Some(max) = info.max_audio_ms.filter(|max| ms > *max) {
        return refuse(format!(
            "{ms} ms of audio is above the vendor maximum of {max} ms"
        ));
    }
    if let Some(max) = info
        .max_upload_bytes
        .filter(|max| upload_bytes as u64 > *max)
    {
        return refuse(format!(
            "an upload of {upload_bytes} bytes is above the vendor maximum of {max}"
        ));
    }
    Ok(())
}

/// One spelling per field, so a vendor's `languages`, `language_codes` or `languageCodes` matches
/// the `languages[]`, `language_codes` or `languageCodes` that was sent.
pub(crate) fn field_key(name: &str) -> String {
    name.trim()
        .trim_end_matches("[]")
        .replace(['_', '-'], "")
        .to_ascii_lowercase()
}

/// Whether an optional field may go out on this request.
pub(crate) fn may_send(ctx: &SegmentContext, name: &str) -> bool {
    if ctx.minimal {
        return false;
    }
    let key = field_key(name);
    !ctx.omit_fields.iter().any(|f| field_key(f) == key)
}

/// The text fields of one request and which optional ones went out.
#[derive(Debug, Default)]
pub(crate) struct Fields {
    pub items: Vec<(String, String)>,
    pub sent_optional: Vec<String>,
}

impl Fields {
    pub fn required(&mut self, name: &str, value: impl Into<String>) {
        self.items.push((name.to_string(), value.into()));
    }

    /// Adds every value under `name` when the field may go out; returns whether it did.
    pub fn optional<I, S>(&mut self, ctx: &SegmentContext, name: &str, values: I) -> bool
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        if !may_send(ctx, name) {
            return false;
        }
        let before = self.items.len();
        for v in values {
            self.items.push((name.to_string(), v.into()));
        }
        let sent = self.items.len() > before;
        if sent && !self.sent_optional.iter().any(|s| s == name) {
            self.sent_optional.push(name.to_string());
        }
        sent
    }
}

/// The session's language as a tag worth sending, or `None` for unset, `auto` and the "no
/// particular language" codes.
pub fn session_language(raw: Option<&str>) -> Option<String> {
    let tag = raw?.trim().replace('_', "-");
    let lower = tag.to_ascii_lowercase();
    if lower.is_empty() || matches!(lower.as_str(), "auto" | "und" | "mul" | "multi" | "zxx") {
        return None;
    }
    Some(tag)
}

/// What a request says about the language: the pinned session language when there is one, else
/// the deduplicated candidates (only for a field that takes a list), each in the vendor's format.
pub(crate) fn languages_to_send(
    ctx: &SegmentContext,
    format: LanguageFormat,
    list: bool,
) -> Vec<String> {
    if let Some(l) =
        session_language(ctx.language.as_deref()).and_then(|l| wire_language(&l, format))
    {
        return vec![l];
    }
    if !list {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for l in ctx
        .candidate_languages
        .iter()
        .filter_map(|c| wire_language(c, format))
    {
        if !out.contains(&l) {
            out.push(l);
        }
    }
    out
}

/// The session language in the vendor's format.
pub fn wire_language(raw: &str, format: LanguageFormat) -> Option<String> {
    match format {
        LanguageFormat::Iso639_1 => iso639_1(raw),
        LanguageFormat::Bcp47 => bcp47(raw),
    }
}

/// The two-letter code of a BCP-47 tag, an ISO 639-2/3 code or a Whisper language name. A
/// three-letter code with no two-letter form (`yue`, `haw`) passes through unchanged: Whisper
/// knows some of them, and a server that does not says so with a 400 the repair handles.
pub fn iso639_1(raw: &str) -> Option<String> {
    let tag = session_language(Some(raw))?;
    let primary = tag
        .split('-')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if primary.len() == 2 && primary.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Some(primary);
    }
    if let Some((_, two)) = ISO639_3.iter().find(|(three, _)| *three == primary) {
        return Some((*two).to_string());
    }
    if let Some((_, two)) = LANGUAGE_NAMES
        .iter()
        .find(|(name, _)| *name == tag.to_ascii_lowercase())
    {
        return Some((*two).to_string());
    }
    (primary.len() == 3 && primary.bytes().all(|b| b.is_ascii_alphabetic())).then_some(primary)
}

/// A BCP-47 tag in canonical case: language lower, script title, region upper.
pub fn bcp47(raw: &str) -> Option<String> {
    let tag = session_language(Some(raw))?;
    let parts: Vec<String> = tag
        .split('-')
        .filter(|p| !p.is_empty())
        .enumerate()
        .map(|(i, p)| match (i, p.len()) {
            (0, _) => p.to_ascii_lowercase(),
            (_, 4) if p.bytes().all(|b| b.is_ascii_alphabetic()) => {
                let mut s = p.to_ascii_lowercase();
                s[..1].make_ascii_uppercase();
                s
            }
            (_, 2) | (_, 3) if p.bytes().all(|b| b.is_ascii_alphanumeric()) => {
                p.to_ascii_uppercase()
            }
            _ => p.to_string(),
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("-"))
}

/// A language a vendor detected, as ISO 639-1 when recognised, else lower-cased as given.
pub fn detected_language(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    iso639_1(trimmed).or_else(|| Some(trimmed.to_ascii_lowercase()))
}

/// ISO 639-3 and 639-2/B codes of the languages Whisper knows, to their 639-1 form.
const ISO639_3: &[(&str, &str)] = &[
    ("afr", "af"),
    ("amh", "am"),
    ("ara", "ar"),
    ("asm", "as"),
    ("aze", "az"),
    ("bak", "ba"),
    ("bel", "be"),
    ("bul", "bg"),
    ("ben", "bn"),
    ("bod", "bo"),
    ("tib", "bo"),
    ("bre", "br"),
    ("bos", "bs"),
    ("cat", "ca"),
    ("ces", "cs"),
    ("cze", "cs"),
    ("cym", "cy"),
    ("wel", "cy"),
    ("dan", "da"),
    ("deu", "de"),
    ("ger", "de"),
    ("ell", "el"),
    ("gre", "el"),
    ("eng", "en"),
    ("spa", "es"),
    ("est", "et"),
    ("eus", "eu"),
    ("baq", "eu"),
    ("fas", "fa"),
    ("per", "fa"),
    ("fin", "fi"),
    ("fao", "fo"),
    ("fra", "fr"),
    ("fre", "fr"),
    ("glg", "gl"),
    ("guj", "gu"),
    ("hau", "ha"),
    ("heb", "he"),
    ("hin", "hi"),
    ("hrv", "hr"),
    ("hat", "ht"),
    ("hun", "hu"),
    ("hye", "hy"),
    ("arm", "hy"),
    ("ind", "id"),
    ("isl", "is"),
    ("ice", "is"),
    ("ita", "it"),
    ("jpn", "ja"),
    ("jav", "jv"),
    ("kat", "ka"),
    ("geo", "ka"),
    ("kaz", "kk"),
    ("khm", "km"),
    ("kan", "kn"),
    ("kor", "ko"),
    ("lat", "la"),
    ("ltz", "lb"),
    ("lin", "ln"),
    ("lao", "lo"),
    ("lit", "lt"),
    ("lav", "lv"),
    ("mlg", "mg"),
    ("mri", "mi"),
    ("mao", "mi"),
    ("mkd", "mk"),
    ("mac", "mk"),
    ("mal", "ml"),
    ("mon", "mn"),
    ("mar", "mr"),
    ("msa", "ms"),
    ("may", "ms"),
    ("zsm", "ms"),
    ("mlt", "mt"),
    ("mya", "my"),
    ("bur", "my"),
    ("nep", "ne"),
    ("nld", "nl"),
    ("dut", "nl"),
    ("nno", "nn"),
    ("nor", "no"),
    ("nob", "nb"),
    ("oci", "oc"),
    ("pan", "pa"),
    ("pol", "pl"),
    ("pus", "ps"),
    ("por", "pt"),
    ("ron", "ro"),
    ("rum", "ro"),
    ("rus", "ru"),
    ("san", "sa"),
    ("snd", "sd"),
    ("sin", "si"),
    ("slk", "sk"),
    ("slo", "sk"),
    ("slv", "sl"),
    ("sna", "sn"),
    ("som", "so"),
    ("sqi", "sq"),
    ("alb", "sq"),
    ("srp", "sr"),
    ("sun", "su"),
    ("swe", "sv"),
    ("swa", "sw"),
    ("tam", "ta"),
    ("tel", "te"),
    ("tgk", "tg"),
    ("tha", "th"),
    ("tuk", "tk"),
    ("tgl", "tl"),
    ("fil", "tl"),
    ("tur", "tr"),
    ("tat", "tt"),
    ("ukr", "uk"),
    ("urd", "ur"),
    ("uzb", "uz"),
    ("vie", "vi"),
    ("yid", "yi"),
    ("yor", "yo"),
    ("zho", "zh"),
    ("chi", "zh"),
    ("cmn", "zh"),
    ("zul", "zu"),
    ("xho", "xh"),
    ("ibo", "ig"),
    ("kin", "rw"),
    ("ori", "or"),
    ("ory", "or"),
];

/// Whisper's `verbose_json` reports the language by name (`"english"`).
const LANGUAGE_NAMES: &[(&str, &str)] = &[
    ("afrikaans", "af"),
    ("amharic", "am"),
    ("arabic", "ar"),
    ("assamese", "as"),
    ("azerbaijani", "az"),
    ("bashkir", "ba"),
    ("belarusian", "be"),
    ("bulgarian", "bg"),
    ("bengali", "bn"),
    ("tibetan", "bo"),
    ("breton", "br"),
    ("bosnian", "bs"),
    ("catalan", "ca"),
    ("czech", "cs"),
    ("welsh", "cy"),
    ("danish", "da"),
    ("german", "de"),
    ("greek", "el"),
    ("english", "en"),
    ("spanish", "es"),
    ("estonian", "et"),
    ("basque", "eu"),
    ("persian", "fa"),
    ("finnish", "fi"),
    ("faroese", "fo"),
    ("french", "fr"),
    ("galician", "gl"),
    ("gujarati", "gu"),
    ("hausa", "ha"),
    ("hebrew", "he"),
    ("hindi", "hi"),
    ("croatian", "hr"),
    ("haitian creole", "ht"),
    ("hungarian", "hu"),
    ("armenian", "hy"),
    ("indonesian", "id"),
    ("icelandic", "is"),
    ("italian", "it"),
    ("japanese", "ja"),
    ("javanese", "jv"),
    ("georgian", "ka"),
    ("kazakh", "kk"),
    ("khmer", "km"),
    ("kannada", "kn"),
    ("korean", "ko"),
    ("latin", "la"),
    ("luxembourgish", "lb"),
    ("lingala", "ln"),
    ("lao", "lo"),
    ("lithuanian", "lt"),
    ("latvian", "lv"),
    ("malagasy", "mg"),
    ("maori", "mi"),
    ("macedonian", "mk"),
    ("malayalam", "ml"),
    ("mongolian", "mn"),
    ("marathi", "mr"),
    ("malay", "ms"),
    ("maltese", "mt"),
    ("myanmar", "my"),
    ("burmese", "my"),
    ("nepali", "ne"),
    ("dutch", "nl"),
    ("nynorsk", "nn"),
    ("norwegian", "no"),
    ("occitan", "oc"),
    ("punjabi", "pa"),
    ("polish", "pl"),
    ("pashto", "ps"),
    ("portuguese", "pt"),
    ("romanian", "ro"),
    ("russian", "ru"),
    ("sanskrit", "sa"),
    ("sindhi", "sd"),
    ("sinhala", "si"),
    ("slovak", "sk"),
    ("slovenian", "sl"),
    ("shona", "sn"),
    ("somali", "so"),
    ("albanian", "sq"),
    ("serbian", "sr"),
    ("sundanese", "su"),
    ("swedish", "sv"),
    ("swahili", "sw"),
    ("tamil", "ta"),
    ("telugu", "te"),
    ("tajik", "tg"),
    ("thai", "th"),
    ("turkmen", "tk"),
    ("tagalog", "tl"),
    ("turkish", "tr"),
    ("tatar", "tt"),
    ("ukrainian", "uk"),
    ("urdu", "ur"),
    ("uzbek", "uz"),
    ("vietnamese", "vi"),
    ("yiddish", "yi"),
    ("yoruba", "yo"),
    ("chinese", "zh"),
];

/// The wait a vendor asked for: `Retry-After` (seconds, fractional seconds or an HTTP date),
/// then Azure's `retry-after-ms`, then the request-limit reset (`x-ratelimit-reset-requests`,
/// `2m59.56s` style, as Groq and OpenAI send it).
pub fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let get = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
    };
    if let Some(d) = get("retry-after").and_then(|v| parse_retry_after(v, SystemTime::now())) {
        return Some(d);
    }
    if let Some(ms) = get("retry-after-ms")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|ms| ms.is_finite() && *ms >= 0.0)
    {
        return Some(Duration::from_secs_f64(ms / 1000.0));
    }
    get("x-ratelimit-reset-requests").and_then(parse_compound_duration)
}

/// A `Retry-After` value: delay seconds (integer or fractional) or an IMF-fixdate.
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(secs) = value.parse::<f64>() {
        return (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs));
    }
    let at = parse_http_date(value)?;
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

/// `Sun, 06 Nov 1994 08:49:37 GMT`, the one date form senders must use (RFC 9110, 5.6.7).
fn parse_http_date(value: &str) -> Option<SystemTime> {
    let rest = value.split_once(", ")?.1;
    let mut it = rest.split_ascii_whitespace();
    let day: u64 = it.next()?.parse().ok()?;
    let month = match it.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = it.next()?.parse().ok()?;
    let mut hms = it.next()?.split(':').map(|p| p.parse::<u64>().ok());
    let (h, m, s) = (hms.next()??, hms.next()??, hms.next()??);
    if it.next()? != "GMT" || !(1..=31).contains(&day) || h > 23 || m > 59 || s > 60 {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = u64::try_from(days).ok()? * 86_400 + h * 3600 + m * 60 + s;
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// `2m59.56s`, `7.66s`, `1h2m`, `250ms` or a bare number of seconds.
pub fn parse_compound_duration(value: &str) -> Option<Duration> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(secs) = value.parse::<f64>() {
        return (secs.is_finite() && secs >= 0.0).then(|| Duration::from_secs_f64(secs));
    }
    let mut total = 0.0f64;
    let mut rest = value;
    while !rest.is_empty() {
        let num_len = rest.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
        if num_len == 0 {
            return None;
        }
        let n: f64 = rest[..num_len].parse().ok()?;
        rest = &rest[num_len..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let scale = match &rest[..unit_len] {
            "h" => 3600.0,
            "m" => 60.0,
            "s" => 1.0,
            "ms" => 0.001,
            _ => return None,
        };
        total += n * scale;
        rest = &rest[unit_len..];
    }
    Some(Duration::from_secs_f64(total))
}

/// The first non-empty header of `names`, at most 200 characters.
pub fn request_id(headers: &HeaderMap, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| {
            headers
                .get(*n)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
        })
        .filter(|v| v.len() <= 200)
        .map(str::to_string)
}

/// A confidence from a mean log-probability, comparable within one model only.
pub fn confidence_from_logprob(mean_logprob: f32) -> f32 {
    mean_logprob.exp().clamp(0.0, 1.0)
}

/// One HTTP answer, read to its end.
#[derive(Debug)]
pub(crate) struct Exchanged {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Exchanged {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_json(&self) -> bool {
        self.headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.to_ascii_lowercase().contains("json"))
    }

    pub fn json(&self) -> Result<Value, SegmentError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| self.not_a_transcript(&format!("the body is not JSON ({e})")))
    }

    /// A 2xx whose body is not a transcript. The vendor did the work, so this is never retried.
    pub fn not_a_transcript(&self, why: &str) -> SegmentError {
        SegmentError::new(
            ErrorClass::Protocol,
            format!("HTTP {} answer is not a transcript: {why}", self.status),
        )
        .with_status(self.status)
        .with_phase(RequestPhase::Headers)
    }
}

/// Sends one request with its own time limit and reads the whole answer, recording on
/// `progress` when the request went out and when headers came back.
pub(crate) async fn exchange(
    req: reqwest::RequestBuilder,
    timeout: Duration,
    progress: &RequestProgress,
) -> Result<Exchanged, SegmentError> {
    let (client, request) = req.timeout(timeout).build_split();
    let request = request.map_err(|e| {
        SegmentError::new(
            ErrorClass::Internal,
            format!("the request could not be built: {}", describe(e)),
        )
        .with_phase(RequestPhase::BeforeSend)
    })?;
    progress.mark_sent();
    let mut resp = client
        .execute(request)
        .await
        .map_err(|e| transport_error(e, false))?;
    progress.mark_headers();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let mut body = BytesMut::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                    return Err(SegmentError::new(
                        ErrorClass::Protocol,
                        format!("the answer is larger than {MAX_RESPONSE_BYTES} bytes"),
                    )
                    .with_status(status)
                    .with_phase(RequestPhase::Headers));
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) => return Err(transport_error(e, true)),
        }
    }
    Ok(Exchanged {
        status,
        headers,
        body: body.freeze(),
    })
}

/// Classifies a failure that produced no complete answer.
///
/// A failed connect (refused, unreachable, TLS, or the 1.5 s connect timeout) reached nobody, so
/// it is `Network` before sending and worth a second request on a fresh connection. The request's
/// own time limit is `Timeout`. A failure after the headers is reported at the `Headers` phase,
/// where the vendor has probably done the work.
pub(crate) fn transport_error(e: reqwest::Error, after_headers: bool) -> SegmentError {
    let (connect, timeout) = (e.is_connect(), e.is_timeout());
    let message = describe(e);
    if connect && !after_headers {
        return SegmentError::new(ErrorClass::Network, message)
            .with_phase(RequestPhase::BeforeSend);
    }
    let phase = if after_headers {
        RequestPhase::Headers
    } else {
        RequestPhase::Sent
    };
    let class = if timeout {
        ErrorClass::Timeout
    } else {
        ErrorClass::Network
    };
    SegmentError::new(class, message).with_phase(phase)
}

/// The error and its sources on one line, without the URL.
fn describe(e: reqwest::Error) -> String {
    use std::error::Error as _;
    let e = e.without_url();
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !out.ends_with(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = s.source();
    }
    out
}

/// What a vendor's error body says, from whichever envelope it uses.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct VendorBody {
    pub message: Option<String>,
    pub code: Option<String>,
    /// OpenAI's `error.param`: the field the vendor refused.
    pub param: Option<String>,
    /// FastAPI validation `detail[].loc`, last element of each.
    pub locs: Vec<String>,
    /// WaaV Infer's envelope, recognised by `error.retriable` being a boolean.
    pub infer: Option<InferEnvelope>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InferEnvelope {
    pub code: String,
    pub retry_after: Option<Duration>,
}

impl VendorBody {
    pub fn parse(body: &str) -> Self {
        let mut out = Self::default();
        let Ok(v) = serde_json::from_str::<Value>(body) else {
            return out;
        };
        let text = |v: &Value| {
            v.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        if let Some(err) = v.get("error") {
            if err.is_object() {
                out.message = err.get("message").and_then(text);
                out.code = err
                    .get("code")
                    .and_then(text)
                    .or_else(|| err.get("status").and_then(text));
                out.param = err.get("param").and_then(text);
                if let (Some(code), Some(_)) =
                    (&out.code, err.get("retriable").and_then(Value::as_bool))
                {
                    out.infer = Some(InferEnvelope {
                        code: code.clone(),
                        retry_after: err
                            .get("retry_after_ms")
                            .and_then(Value::as_u64)
                            .map(Duration::from_millis),
                    });
                }
            } else {
                out.message = text(err);
            }
        }
        match v.get("detail") {
            Some(Value::Array(items)) => {
                let mut msgs = Vec::new();
                for d in items {
                    let loc: Vec<String> = d
                        .get("loc")
                        .and_then(Value::as_array)
                        .map(|l| {
                            l.iter()
                                .filter_map(|p| p.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    if let Some(last) = loc.last() {
                        out.locs.push(last.clone());
                    }
                    let msg = d.get("msg").and_then(text).unwrap_or_default();
                    msgs.push(if loc.is_empty() {
                        msg
                    } else {
                        format!("{}: {msg}", loc.join("."))
                    });
                }
                if out.message.is_none() && !msgs.is_empty() {
                    out.message = Some(msgs.join("; "));
                }
            }
            Some(d @ Value::Object(_)) => {
                out.message = out.message.or_else(|| d.get("message").and_then(text));
                out.code = out
                    .code
                    .or_else(|| d.get("code").and_then(text))
                    .or_else(|| d.get("status").and_then(text));
            }
            Some(d) => out.message = out.message.or_else(|| text(d)),
            None => {}
        }
        out.message = out
            .message
            .or_else(|| v.get("err_msg").and_then(text))
            .or_else(|| v.get("message").and_then(text));
        out.code = out
            .code
            .or_else(|| v.get("error_code").and_then(text))
            .or_else(|| v.get("err_code").and_then(text));
        out
    }

    /// The vendor's code in one spelling: lower case, no separators.
    pub fn code_key(&self) -> Option<String> {
        self.code.as_deref().map(field_key)
    }

    fn names_the_model(&self) -> bool {
        const MODEL_FIELDS: &[&str] = &["model", "modelid", "xaaimodel", "speechmodel"];
        const MODEL_CODES: &[&str] = &[
            "modelnotfound",
            "modelnotavailable",
            "modelnotsupported",
            "invalidmodel",
            "deploymentnotfound",
        ];
        self.param
            .as_deref()
            .is_some_and(|p| MODEL_FIELDS.contains(&field_key(p).as_str()))
            || self
                .locs
                .iter()
                .any(|l| MODEL_FIELDS.contains(&field_key(l).as_str()))
            || self
                .code_key()
                .is_some_and(|c| MODEL_CODES.contains(&c.as_str()))
    }
}

/// One failed answer and what is needed to judge it.
pub(crate) struct Failure<'a> {
    pub vendor: &'a str,
    pub exchanged: &'a Exchanged,
    /// The optional fields this request carried, the only ones a refusal can name.
    pub sent_optional: &'a [String],
    pub secret: Option<&'a str>,
}

impl Failure<'_> {
    /// The status rules shared by every family. Families adjust the result for their own codes.
    pub fn classify(&self) -> (SegmentError, VendorBody) {
        let status = self.exchanged.status;
        let raw = String::from_utf8_lossy(&self.exchanged.body);
        let parsed = VendorBody::parse(&raw);
        let message = parsed
            .message
            .clone()
            .unwrap_or_else(|| raw.trim().to_string());
        // Redact before cutting, so a key that straddles the cut cannot survive in part.
        let message = truncate(&redact(&message, self.secret), 300);
        let message = format!(
            "{}: {}",
            self.vendor,
            if message.is_empty() {
                "no message"
            } else {
                &message
            }
        );

        let mut err = if (300..400).contains(&status) {
            SegmentError::new(
                ErrorClass::Protocol,
                format!("{message} (a redirect, which uploads never follow)"),
            )
            .with_status(status)
            .with_phase(RequestPhase::Headers)
        } else {
            SegmentError::from_status(status, message)
        };
        err.retry_after = retry_after(&self.exchanged.headers);
        if status == 402 {
            // The plan or the balance: as final as a refused key.
            err.class = ErrorClass::Auth;
        }
        if matches!(
            err.class,
            ErrorClass::BadRequest | ErrorClass::ModelNotServed
        ) && parsed.names_the_model()
        {
            err.class = ErrorClass::ModelNotServed;
        }
        if err.class == ErrorClass::BadRequest && matches!(status, 400 | 422) {
            err.refused_field = refused_field(&parsed, &raw, self.sent_optional);
        }
        (err, parsed)
    }
}

/// The optional field a refusal names: the envelope's `param` or a FastAPI `loc` when it is one
/// of the fields sent, else the first sent field whose name appears in the message as a word.
pub(crate) fn refused_field(parsed: &VendorBody, raw: &str, sent: &[String]) -> Option<String> {
    let find = |name: &str| {
        let key = field_key(name);
        let last = key.rsplit('.').next().unwrap_or(&key).to_string();
        sent.iter().find(|s| {
            let k = field_key(s);
            k == key || k == last
        })
    };
    if let Some(f) = parsed.param.as_deref().and_then(find) {
        return Some(f.clone());
    }
    if let Some(f) = parsed.locs.iter().find_map(|l| find(l)) {
        return Some(f.clone());
    }
    let text = parsed.message.as_deref().unwrap_or(raw);
    let words: Vec<String> = text
        .split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '[' | ']' | '-')))
        .map(|w| field_key(w.trim_matches('.')))
        .filter(|w| !w.is_empty())
        .collect();
    sent.iter()
        .find(|s| {
            let k = field_key(s);
            words
                .iter()
                .any(|w| *w == k || w.rsplit('.').next() == Some(k.as_str()))
        })
        .cloned()
}

fn truncate(s: &str, max: usize) -> String {
    let s = s.trim();
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// Removes a credential a vendor echoed back. Short secrets are left alone: replacing a
/// three-character key would mangle ordinary words, and no vendor issues one that short.
pub(crate) fn redact(message: &str, secret: Option<&str>) -> String {
    match secret.map(str::trim).filter(|s| s.len() >= 8) {
        Some(s) => message.replace(s, "[redacted]"),
        None => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_adapters_name_the_map_ids_once_each() {
        let ids = built_adapters();
        let mut sorted = ids.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
        assert!(ids.contains(&"openai_transcriptions"));
        assert!(ids.contains(&"google_recognize"));
        assert!(!ids.contains(&"native"));
    }

    #[test]
    fn languages_reach_the_vendor_in_its_format() {
        assert_eq!(iso639_1("en-US").as_deref(), Some("en"));
        assert_eq!(iso639_1("pt_BR").as_deref(), Some("pt"));
        assert_eq!(iso639_1("zh-Hant-TW").as_deref(), Some("zh"));
        assert_eq!(iso639_1("eng").as_deref(), Some("en"));
        assert_eq!(iso639_1("cmn").as_deref(), Some("zh"));
        assert_eq!(iso639_1("English").as_deref(), Some("en"));
        assert_eq!(iso639_1("yue").as_deref(), Some("yue"));
        assert_eq!(iso639_1("auto"), None);
        assert_eq!(iso639_1(""), None);
        assert_eq!(iso639_1("  "), None);
        assert_eq!(bcp47("en_us").as_deref(), Some("en-US"));
        assert_eq!(bcp47("zh-hant-tw").as_deref(), Some("zh-Hant-TW"));
        assert_eq!(bcp47("es-419").as_deref(), Some("es-419"));
        assert_eq!(bcp47("auto"), None);
        assert_eq!(detected_language("eng").as_deref(), Some("en"));
        assert_eq!(detected_language("english").as_deref(), Some("en"));
        assert_eq!(detected_language("en-US").as_deref(), Some("en"));
        assert_eq!(detected_language("Klingon").as_deref(), Some("klingon"));
        assert_eq!(detected_language(" "), None);
    }

    #[test]
    fn retry_after_reads_every_form_vendors_send() {
        let now = UNIX_EPOCH + Duration::from_secs(784_111_717); // Sun, 06 Nov 1994 08:48:37 GMT
        assert_eq!(parse_retry_after("2", now), Some(Duration::from_secs(2)));
        assert_eq!(
            parse_retry_after("0.5", now),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", now),
            Some(Duration::from_secs(60))
        );
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:47:37 GMT", now),
            Some(Duration::ZERO)
        );
        assert_eq!(parse_retry_after("soon", now), None);
        assert_eq!(parse_retry_after("-3", now), None);

        assert_eq!(
            parse_compound_duration("2m59.56s"),
            Some(Duration::from_secs_f64(179.56))
        );
        assert_eq!(
            parse_compound_duration("7.66s"),
            Some(Duration::from_secs_f64(7.66))
        );
        assert_eq!(
            parse_compound_duration("1h2m"),
            Some(Duration::from_secs(3720))
        );
        assert_eq!(
            parse_compound_duration("250ms"),
            Some(Duration::from_millis(250))
        );
        assert_eq!(parse_compound_duration("3"), Some(Duration::from_secs(3)));
        assert_eq!(parse_compound_duration("3x"), None);
        assert_eq!(parse_compound_duration("m"), None);

        let mut h = HeaderMap::new();
        h.insert("x-ratelimit-reset-requests", "2m59.56s".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_secs_f64(179.56)));
        h.insert("retry-after-ms", "1500".parse().unwrap());
        assert_eq!(retry_after(&h), Some(Duration::from_millis(1500)));
        h.insert("retry-after", "2".parse().unwrap());
        assert_eq!(
            retry_after(&h),
            Some(Duration::from_secs(2)),
            "Retry-After wins"
        );
    }

    #[test]
    fn the_request_id_is_the_first_header_present() {
        let mut h = HeaderMap::new();
        assert_eq!(request_id(&h, &["x-request-id"]), None);
        h.insert("apim-request-id", "az-1".parse().unwrap());
        h.insert("x-request-id", " ".parse().unwrap());
        assert_eq!(
            request_id(&h, &["x-request-id", "apim-request-id"]).as_deref(),
            Some("az-1")
        );
    }

    #[test]
    fn auth_never_prints_its_secret() {
        let a = Auth::Bearer("sk-very-secret".into());
        assert!(!format!("{a:?}").contains("sk-very-secret"));
        assert!(!format!("{:?}", Auth::elevenlabs("xi-secret-key")).contains("xi-secret-key"));
        assert_eq!(Auth::Bearer(String::new()).secret(), None);
        let bad = Auth::Bearer("line\nbreak".into()).apply(reqwest::Client::new().get("http://x/"));
        let err = bad.err().unwrap();
        assert_eq!(err.class, ErrorClass::Internal);
        assert!(!err.message.contains("line"));
    }

    #[test]
    fn vendor_envelopes_are_read() {
        let openai = VendorBody::parse(
            r#"{"error":{"message":"Invalid language 'xx'.","type":"invalid_request_error","param":"language","code":"invalid_language_format"}}"#,
        );
        assert_eq!(openai.param.as_deref(), Some("language"));
        assert_eq!(openai.code.as_deref(), Some("invalid_language_format"));
        assert!(openai.infer.is_none());

        let infer = VendorBody::parse(
            r#"{"error":{"code":"admission_rejected","message":"queue full","retriable":true,"retry_after_ms":250}}"#,
        );
        assert_eq!(
            infer.infer,
            Some(InferEnvelope {
                code: "admission_rejected".into(),
                retry_after: Some(Duration::from_millis(250))
            })
        );

        let fastapi =
            VendorBody::parse(r#"{"detail":[{"loc":["body","language_code"],"msg":"bad"}]}"#);
        assert_eq!(fastapi.locs, vec!["language_code"]);
        assert_eq!(fastapi.message.as_deref(), Some("body.language_code: bad"));

        let el = VendorBody::parse(r#"{"detail":{"status":"system_busy","message":"busy"}}"#);
        assert_eq!(el.code.as_deref(), Some("system_busy"));
        let dg =
            VendorBody::parse(r#"{"err_code":"Bad Request","err_msg":"nope","request_id":"r"}"#);
        assert_eq!(dg.message.as_deref(), Some("nope"));
        let aai = VendorBody::parse(r#"{"error_code":"capacity_exceeded","message":"later"}"#);
        assert_eq!(aai.code.as_deref(), Some("capacity_exceeded"));
        let google = VendorBody::parse(
            r#"{"error":{"code":429,"message":"quota","status":"RESOURCE_EXHAUSTED"}}"#,
        );
        assert_eq!(google.code.as_deref(), Some("RESOURCE_EXHAUSTED"));
        assert_eq!(VendorBody::parse("<html>"), VendorBody::default());
    }

    #[test]
    fn a_refusal_names_only_a_field_that_was_sent() {
        let sent = vec![
            "languages[]".to_string(),
            "prompt".to_string(),
            "response_format".to_string(),
        ];
        let by_param = VendorBody {
            param: Some("languages".into()),
            ..Default::default()
        };
        assert_eq!(
            refused_field(&by_param, "", &sent).as_deref(),
            Some("languages[]")
        );
        let by_word = VendorBody {
            message: Some("'prompt' is too long".into()),
            ..Default::default()
        };
        assert_eq!(
            refused_field(&by_word, "", &sent).as_deref(),
            Some("prompt")
        );
        let infer = VendorBody {
            message: Some("transcriptions field 'response_format' is not supported".into()),
            ..Default::default()
        };
        assert_eq!(
            refused_field(&infer, "", &sent).as_deref(),
            Some("response_format")
        );
        let other = VendorBody {
            message: Some("the file is corrupt; prompts are fine".into()),
            ..Default::default()
        };
        assert_eq!(refused_field(&other, "", &sent), None);
        let dotted = vec!["phraseList".to_string()];
        let azure = VendorBody {
            message: Some("Invalid definition.phraseList value".into()),
            ..Default::default()
        };
        assert_eq!(
            refused_field(&azure, "", &dotted).as_deref(),
            Some("phraseList")
        );
    }

    #[test]
    fn may_send_honours_minimal_and_omitted_fields() {
        let ctx = SegmentContext {
            omit_fields: vec!["keywords".into()],
            ..Default::default()
        };
        assert!(!may_send(&ctx, "keywords[]"));
        assert!(may_send(&ctx, "prompt"));
        let minimal = SegmentContext {
            minimal: true,
            ..Default::default()
        };
        assert!(!may_send(&minimal, "prompt"));
    }

    #[test]
    fn redaction_removes_an_echoed_key() {
        assert_eq!(
            redact("bad key sk-abcdef123 given", Some("sk-abcdef123")),
            "bad key [redacted] given"
        );
        assert_eq!(redact("abc", Some("abc")), "abc");
        assert_eq!(truncate("ééé", 2), "éé…");
    }

    #[test]
    fn a_key_straddling_the_message_cut_does_not_survive_in_part() {
        let secret = "sk-live-0123456789abcdef";
        let body = format!(
            r#"{{"error":{{"message":"{} {secret} tail"}}}}"#,
            "x".repeat(290)
        );
        let exchanged = Exchanged {
            status: 401,
            headers: HeaderMap::new(),
            body: Bytes::from(body),
        };
        let failure = Failure {
            vendor: "v",
            exchanged: &exchanged,
            sent_optional: &[],
            secret: Some(secret),
        };
        let message = failure.classify().0.message;
        assert!(!message.contains("sk-live"), "{message}");
    }

    #[test]
    fn a_vendor_header_name_in_any_case_is_accepted() {
        let auth = Auth::Header {
            name: "X-Api-Key",
            scheme: None,
            secret: "k".into(),
        };
        let req = auth
            .apply(reqwest::Client::new().get("http://x/"))
            .unwrap()
            .build()
            .unwrap();
        assert_eq!(req.headers().get("x-api-key").unwrap(), "k");
        assert!(req.headers().get("x-api-key").unwrap().is_sensitive());
        let bad = Auth::Header {
            name: "bad name",
            scheme: None,
            secret: "k".into(),
        };
        assert_eq!(
            bad.apply(reqwest::Client::new().get("http://x/"))
                .err()
                .unwrap()
                .class,
            ErrorClass::Internal
        );
    }

    #[tokio::test]
    async fn a_body_cut_off_after_the_headers_is_a_network_failure_at_the_headers_phase() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{\"text\":")
                .await
                .unwrap();
            sock.shutdown().await.unwrap();
        });
        let progress = RequestProgress::new();
        let req = reqwest::Client::new().get(format!("http://{addr}/"));
        let err = exchange(req, Duration::from_secs(5), &progress)
            .await
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::Network, "{err}");
        assert_eq!(err.phase, RequestPhase::Headers);
        assert!(!err.is_fast_retryable(), "the vendor did the work");
        assert!(progress.headers_received());
    }

    #[tokio::test]
    async fn a_body_that_stalls_is_a_timeout_at_the_headers_phase() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            sock.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let progress = RequestProgress::new();
        let req = reqwest::Client::new().get(format!("http://{addr}/"));
        let err = exchange(req, Duration::from_millis(300), &progress)
            .await
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::Timeout, "{err}");
        assert_eq!(err.phase, RequestPhase::Headers);
    }

    #[tokio::test]
    async fn an_oversized_answer_is_refused() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = sock.read(&mut buf).await;
            let len = MAX_RESPONSE_BYTES + 10;
            sock.write_all(format!("HTTP/1.1 200 OK\r\ncontent-length: {len}\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let chunk = vec![b'a'; 64 * 1024];
            let mut sent = 0;
            while sent < len {
                if sock.write_all(&chunk).await.is_err() {
                    break;
                }
                sent += chunk.len();
            }
        });
        let progress = RequestProgress::new();
        let req = reqwest::Client::new().get(format!("http://{addr}/"));
        let err = exchange(req, Duration::from_secs(10), &progress)
            .await
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::Protocol, "{err}");
    }
}
