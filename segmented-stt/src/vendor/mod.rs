//! What every speech-to-text upload path needs from a vendor's HTTP exchange, in one copy: the
//! vendor's hosts and its "keep no audio" switch, Azure OpenAI's URL shape, the request id in the
//! response, how long a rate limit asks the caller to wait, and what its error body says.
//!
//! Used by segmented sessions' utterance uploads and by the gateway's upload clients, REST route and
//! TTS error rendering, so a vendor's quirks are fixed once.

pub mod openai;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::header::HeaderMap;
use serde_json::Value;

/// Vendor hosts, by region.
pub mod hosts {
    pub const DEEPGRAM: &str = "https://api.deepgram.com";
    pub const DEEPGRAM_EU: &str = "https://api.eu.deepgram.com";
    pub const ELEVENLABS: &str = "https://api.elevenlabs.io";
    /// ElevenLabs' EU data-residency host.
    pub const ELEVENLABS_EU: &str = "https://api.eu.residency.elevenlabs.io";
    /// AssemblyAI's asynchronous API (`/v2/upload`, `/v2/transcript`).
    pub const ASSEMBLYAI: &str = "https://api.assemblyai.com";
    pub const ASSEMBLYAI_EU: &str = "https://api.eu.assemblyai.com";
    /// AssemblyAI's synchronous API (`/v1/transcribe`).
    pub const ASSEMBLYAI_SYNC: &str = "https://sync.assemblyai.com";
    pub const ASSEMBLYAI_SYNC_EU: &str = "https://sync.eu.assemblyai.com";
    pub const OPENAI: &str = "https://api.openai.com";
    /// OpenAI's EU data-residency host.
    pub const OPENAI_EU: &str = "https://eu.api.openai.com";
    /// Groq's OpenAI-compatible base (`/v1/audio/transcriptions` follows).
    pub const GROQ_OPENAI: &str = "https://api.groq.com/openai";
}

/// The query parameter that asks a vendor to keep no audio, where it has one.
pub mod retention {
    /// Deepgram: leave the model-improvement programme for this request.
    pub const DEEPGRAM: (&str, &str) = ("mip_opt_out", "true");
    /// ElevenLabs: zero-retention mode (the account must be eligible).
    pub const ELEVENLABS: (&str, &str) = ("enable_logging", "false");
}

/// Azure OpenAI audio: deployment-scoped URLs and the `api-key` header.
pub mod azure_openai {
    /// The `api-version` sent when a deployment does not name one.
    pub const DEFAULT_API_VERSION: &str = "2025-04-01-preview";

    /// The header Azure OpenAI reads its key from. It ignores `Authorization: Bearer`.
    pub const API_KEY_HEADER: &str = "api-key";

    /// Which Azure OpenAI audio operation a URL addresses.
    ///
    /// Transcription and translation are separate ROUTES, as on every OpenAI-shaped server: a
    /// translation request sent to the transcription route returns source-language text with a 200.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum AudioRoute {
        Speech,
        Transcriptions,
        Translations,
    }

    impl AudioRoute {
        fn segment(self) -> &'static str {
            match self {
                Self::Speech => "speech",
                Self::Transcriptions => "transcriptions",
                Self::Translations => "translations",
            }
        }
    }

    /// The `api-version` to send: the deployment's own when it names one, else the default.
    pub fn api_version(requested: Option<&str>) -> &str {
        requested
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(DEFAULT_API_VERSION)
    }

    /// `{api_base}/openai/deployments/{deployment}/audio/{route}?api-version={api_version}`.
    ///
    /// `api_base` is the resource endpoint (`https://<resource>.openai.azure.com`), with or without
    /// a trailing slash; a path on it (an API Management prefix) is kept. The deployment is ONE
    /// path segment and is percent-encoded as one, so a `/` in it cannot re-route the request; `.`
    /// and `..` are refused outright, because a URL builder silently drops them.
    pub fn audio_url(
        api_base: &str,
        deployment: &str,
        route: AudioRoute,
        version: Option<&str>,
    ) -> Result<String, String> {
        let deployment = deployment.trim();
        if deployment.is_empty() || deployment == "." || deployment == ".." {
            return Err(format!(
                "an azure_openai endpoint needs a deployment name (voice_table `model`: the \
                 credential's deployment_id, else the model name); got {deployment:?}"
            ));
        }
        let base = api_base.trim().trim_end_matches('/');
        let mut url = url::Url::parse(base)
            .map_err(|e| format!("azure_openai api_base {base:?} is not a URL: {e}"))?;
        url.path_segments_mut()
            .map_err(|()| format!("azure_openai api_base {base:?} cannot carry a path"))?
            .pop_if_empty()
            .extend([
                "openai",
                "deployments",
                deployment,
                "audio",
                route.segment(),
            ]);
        url.query_pairs_mut()
            .append_pair("api-version", api_version(version));
        Ok(url.into())
    }
}

/// One spelling per field, so a vendor's `languages`, `language_codes` or `languageCodes` matches
/// the `languages[]`, `language_codes` or `languageCodes` that was sent.
pub fn field_key(name: &str) -> String {
    name.trim()
        .trim_end_matches("[]")
        .replace(['_', '-'], "")
        .to_ascii_lowercase()
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

/// How long a refusal asks the caller to wait: `retry-after` (seconds or an HTTP date), then
/// Azure's `retry-after-ms`, then the request-limit reset (`x-ratelimit-reset-requests`,
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

/// What a vendor's error body says, from whichever envelope it uses.
///
/// Every vendor reports failure in its own JSON shape, and handing the raw body to a caller buries
/// the fact they need. The message is read from, in order:
///
/// * a **FastAPI validation list**, `{"detail": [{"loc": [...], "msg": "..."}]}`, rendered as
///   `loc.path: msg` joined with `; ` (ElevenLabs' request validation);
/// * an **object under `detail`, then under `error`**, `{"message"|"msg": "..."}` (ElevenLabs'
///   domain errors; OpenAI and everything that copies its envelope);
/// * a **string under** `detail`, `err_msg` (Deepgram), `error` (AssemblyAI), `message` or
///   `reason`.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct VendorBody {
    pub message: Option<String>,
    /// The documented code fields: `detail.code` (ElevenLabs), `detail.status`, `error.code`
    /// (OpenAI).
    pub envelope_code: Option<String>,
    /// [`Self::envelope_code`], else `error.status`, `error_code` or `err_code`.
    pub code: Option<String>,
    /// OpenAI's `error.param`: the field the vendor refused.
    pub param: Option<String>,
    /// FastAPI validation `detail[].loc`, last element of each.
    pub locs: Vec<String>,
    /// WaaV Infer's envelope, recognised by `error.retriable` being a boolean.
    pub infer: Option<InferEnvelope>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InferEnvelope {
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

        if let Some(items) = v.get("detail").and_then(Value::as_array) {
            let mut rendered = Vec::new();
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
                let line = if loc.is_empty() {
                    msg
                } else {
                    format!("{}: {msg}", loc.join("."))
                };
                if !line.is_empty() {
                    rendered.push(line);
                }
            }
            if !rendered.is_empty() {
                out.message = Some(rendered.join("; "));
            }
        }
        for envelope in ["detail", "error"] {
            if out.message.is_none() {
                out.message = v
                    .get(envelope)
                    .and_then(|d| d.get("message").or_else(|| d.get("msg")))
                    .and_then(text);
            }
        }
        for key in ["detail", "err_msg", "error", "message", "reason"] {
            if out.message.is_none() {
                out.message = v.get(key).and_then(text);
            }
        }

        let nested =
            |outer: &str, inner: &str| v.get(outer).and_then(|o| o.get(inner)).and_then(text);
        out.envelope_code = nested("detail", "code")
            .or_else(|| nested("detail", "status"))
            .or_else(|| nested("error", "code"));
        out.code = out
            .envelope_code
            .clone()
            .or_else(|| nested("error", "status"))
            .or_else(|| v.get("error_code").and_then(text))
            .or_else(|| v.get("err_code").and_then(text));
        out.param = nested("error", "param");
        if let Some(err) = v.get("error").filter(|e| e.is_object())
            && let (Some(code), Some(_)) = (
                nested("error", "code").or_else(|| nested("error", "status")),
                err.get("retriable").and_then(Value::as_bool),
            )
        {
            out.infer = Some(InferEnvelope {
                code,
                retry_after: err
                    .get("retry_after_ms")
                    .and_then(Value::as_u64)
                    .map(Duration::from_millis),
            });
        }
        out
    }

    /// The vendor's code in one spelling: lower case, no separators.
    pub fn code_key(&self) -> Option<String> {
        self.code.as_deref().map(field_key)
    }

    /// Whether the refusal is about the model (a field naming it, or a not-found code).
    pub fn names_the_model(&self) -> bool {
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

/// The human-readable message in a vendor's error body; `None` when the body is not JSON or
/// matches no known shape, so the caller shows the raw body, which is still the most information
/// available.
pub fn vendor_message(body: &str) -> Option<String> {
    VendorBody::parse(body).message
}

/// The vendor's machine-readable error code, when its body carries one: `detail.code`
/// (ElevenLabs), then `detail.status`, then `error.code` (OpenAI). Used where the STATUS alone
/// cannot say who can fix the failure: a 403 is a bad key or a plan limit.
pub fn vendor_code(body: &str) -> Option<String> {
    VendorBody::parse(body).envelope_code
}

/// Removes a credential a vendor echoed back. Short secrets are left alone: replacing a
/// three-character key would mangle ordinary words, and no vendor issues one that short.
pub fn redact(message: &str, secret: Option<&str>) -> String {
    match secret.map(str::trim).filter(|s| s.len() >= 8) {
        Some(s) => message.replace(s, "[redacted]"),
        None => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn the_request_id_is_the_first_named_header_with_a_value() {
        let mut h = HeaderMap::new();
        h.insert("dg-request-id", HeaderValue::from_static("  "));
        h.insert("x-request-id", HeaderValue::from_static("req-7"));
        assert_eq!(
            request_id(&h, &["dg-request-id", "request-id", "x-request-id"]).as_deref(),
            Some("req-7"),
            "an empty header does not stop the search"
        );
        h.insert(
            "request-id",
            HeaderValue::from_str(&"x".repeat(201)).unwrap(),
        );
        assert_eq!(
            request_id(&h, &["request-id"]),
            None,
            "an over-long id is dropped"
        );
        assert_eq!(request_id(&HeaderMap::new(), &["x-request-id"]), None);
    }

    #[test]
    fn a_duration_reads_every_form_vendors_send() {
        assert_eq!(
            parse_compound_duration("2m59.56s"),
            Some(Duration::from_secs_f64(179.56))
        );
        assert_eq!(
            parse_compound_duration("7.66s"),
            Some(Duration::from_secs_f64(7.66))
        );
        assert_eq!(
            parse_compound_duration("500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(parse_compound_duration("1m"), Some(Duration::from_secs(60)));
        assert_eq!(
            parse_compound_duration("1h2m"),
            Some(Duration::from_secs(3720))
        );
        assert_eq!(parse_compound_duration("3"), Some(Duration::from_secs(3)));
        assert_eq!(parse_compound_duration("soon"), None);
        assert_eq!(parse_compound_duration(""), None);
    }

    #[test]
    fn an_error_body_is_read_from_each_envelope_in_order() {
        let m = |b: &str| vendor_message(b);
        assert_eq!(
            m(r#"{"detail":[{"loc":["body","model_id"],"msg":"field required"}]}"#).as_deref(),
            Some("body.model_id: field required")
        );
        assert_eq!(
            m(r#"{"detail":{"code":"voice_not_found","message":"No such voice"}}"#).as_deref(),
            Some("No such voice")
        );
        assert_eq!(
            m(r#"{"error":{"message":"bad key"}}"#).as_deref(),
            Some("bad key")
        );
        assert_eq!(
            m(r#"{"err_msg":"Bad model"}"#).as_deref(),
            Some("Bad model")
        );
        assert_eq!(
            m(r#"{"error":"Upload failed"}"#).as_deref(),
            Some("Upload failed")
        );
        assert_eq!(m(r#"{"reason":"quota"}"#).as_deref(), Some("quota"));
        assert_eq!(m("not json"), None);
        assert_eq!(m(r#"{"other":1}"#), None);
        // Both envelopes: the validation list is the more specific answer.
        assert_eq!(
            m(r#"{"error":{"message":"invalid"},"detail":[{"loc":["model"],"msg":"unknown"}]}"#)
                .as_deref(),
            Some("model: unknown")
        );
    }

    #[test]
    fn the_documented_code_is_kept_apart_from_the_fallback_codes() {
        let b = VendorBody::parse(r#"{"error":{"code":403,"status":"PERMISSION_DENIED"}}"#);
        assert_eq!(b.envelope_code, None, "a numeric code is not a code");
        assert_eq!(b.code.as_deref(), Some("PERMISSION_DENIED"));
        assert_eq!(
            vendor_code(r#"{"detail":{"status":"quota_exceeded"}}"#).as_deref(),
            Some("quota_exceeded")
        );
        let refused = VendorBody::parse(
            r#"{"error":{"message":"m","code":"model_not_found","param":"model"}}"#,
        );
        assert!(refused.names_the_model());
        let infer = VendorBody::parse(
            r#"{"error":{"code":"overloaded","retriable":true,"retry_after_ms":250}}"#,
        );
        assert_eq!(
            infer.infer,
            Some(InferEnvelope {
                code: "overloaded".into(),
                retry_after: Some(Duration::from_millis(250))
            })
        );
    }

    #[test]
    fn an_azure_url_encodes_the_deployment_and_names_the_version() {
        use azure_openai::{AudioRoute, audio_url};
        assert_eq!(
            audio_url(
                "https://r.openai.azure.com/",
                "whisper",
                AudioRoute::Transcriptions,
                None
            )
            .unwrap(),
            "https://r.openai.azure.com/openai/deployments/whisper/audio/transcriptions?api-version=2025-04-01-preview"
        );
        assert_eq!(
            audio_url(
                "https://r.openai.azure.com",
                "a/b",
                AudioRoute::Translations,
                Some("v1")
            )
            .unwrap(),
            "https://r.openai.azure.com/openai/deployments/a%2Fb/audio/translations?api-version=v1"
        );
        assert!(audio_url("https://r.openai.azure.com", "..", AudioRoute::Speech, None).is_err());
    }

    #[test]
    fn a_short_secret_is_not_redacted() {
        assert_eq!(
            redact("key sk-12345678 bad", Some("sk-12345678")),
            "key [redacted] bad"
        );
        assert_eq!(redact("abc", Some("abc")), "abc");
    }
}
