//! OpenAI-compatible multipart: OpenAI, Groq, Azure OpenAI, self-hosted servers and WaaV Infer.
//!
//! One implementation for five vendors. The row decides everything that differs: the full URL
//! (Azure's is deployment-scoped and carries `api-version`), the credential header, the language
//! field (`language`, the `languages[]` list of `gpt-transcribe`, or none for WaaV Infer, which
//! answers 400 to a language), the context fields and the response format.
//!
//! Vocabulary: key terms go to the keywords field when the row has one (`gpt-transcribe`); a row
//! with only a prompt (Whisper) gets them joined into the prompt after the caller's own text, as
//! the gateway's file client does; a row with neither drops them.

use std::time::Duration;

use reqwest::multipart::{Form, Part};
use serde_json::Value;

use super::{
    Auth, Exchanged, Failure, Fields, LanguageFormat, RowLimits, check_limits, checked_url,
    confidence_from_logprob, detected_language, exchange, info_for, languages_to_send,
    refused_field, request_id,
};
use crate::transcriber::{
    RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};
use crate::types::ErrorClass;

/// The only extra fields a row may add: a map row must not be able to inject an arbitrary field.
pub const EXTRA_FIELDS: &[&str] = &["include[]", "timestamp_granularities[]", "temperature"];

/// The response formats this family can read.
pub const RESPONSE_FORMATS: &[&str] = &["json", "verbose_json", "text"];

/// Where the language goes. `languages` with `list: true` is sent as `languages[]=en`, once per
/// code; `gpt-transcribe` takes only that form and must never get the singular field as well.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageDialect {
    pub param: Option<String>,
    pub list: bool,
}

impl LanguageDialect {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn single(param: &str) -> Self {
        Self {
            param: Some(param.to_string()),
            list: false,
        }
    }

    pub fn list(param: &str) -> Self {
        Self {
            param: Some(param.to_string()),
            list: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiCompatConfig {
    /// `openai_transcriptions`, `groq_transcriptions` or `azure_openai_transcriptions`.
    pub adapter: String,
    /// The full endpoint URL, query included.
    pub url: String,
    pub auth: Auth,
    /// The id that reaches the vendor: for Azure the deployment name.
    pub model: String,
    pub language: LanguageDialect,
    pub prompt_param: Option<String>,
    pub keywords_param: Option<String>,
    /// `json`, `verbose_json` or `text`; `None` leaves the server's default (`json`).
    pub response_format: Option<String>,
    /// From [`EXTRA_FIELDS`] only, e.g. `("include[]", "logprobs")`.
    pub extra_fields: Vec<(String, String)>,
    pub client: reqwest::Client,
    pub limits: RowLimits,
}

#[derive(Debug)]
pub struct OpenAiCompatTranscriber {
    cfg: OpenAiCompatConfig,
    info: TranscriberInfo,
}

fn list_name(param: &str) -> String {
    if param.ends_with("[]") {
        param.to_string()
    } else {
        format!("{param}[]")
    }
}

impl OpenAiCompatTranscriber {
    pub fn new(cfg: OpenAiCompatConfig) -> Result<Self, String> {
        checked_url(&cfg.url)?;
        if cfg.model.trim().is_empty() {
            return Err("an OpenAI-compatible transcriber needs a model".into());
        }
        if let Some(f) = cfg
            .response_format
            .as_deref()
            .filter(|f| !RESPONSE_FORMATS.contains(f))
        {
            return Err(format!(
                "response format {f:?} cannot be read; use one of {RESPONSE_FORMATS:?}"
            ));
        }
        if let Some((k, _)) = cfg
            .extra_fields
            .iter()
            .find(|(k, _)| !EXTRA_FIELDS.contains(&k.as_str()))
        {
            return Err(format!(
                "{k:?} is not a field a row may add; allowed: {EXTRA_FIELDS:?}"
            ));
        }
        let mut droppable = Vec::new();
        if let Some(name) = language_field(&cfg.language) {
            droppable.push(name);
        }
        droppable.extend(cfg.prompt_param.clone());
        droppable.extend(cfg.keywords_param.as_deref().map(list_name));
        if cfg.response_format.is_some() {
            droppable.push("response_format".into());
        }
        droppable.extend(cfg.extra_fields.iter().map(|(k, _)| k.clone()));
        let info = info_for(&cfg.adapter, &cfg.url, &cfg.model, cfg.limits, droppable);
        Ok(Self { cfg, info })
    }

    fn fields(&self, ctx: &SegmentContext) -> Fields {
        let cfg = &self.cfg;
        let mut f = Fields::default();
        f.required("model", cfg.model.trim());
        if let Some(fmt) = &cfg.response_format {
            f.optional(ctx, "response_format", [fmt.clone()]);
        }

        if let Some(name) = language_field(&cfg.language) {
            // Only a list can carry candidates; a single field with no language sends none.
            let codes = languages_to_send(ctx, LanguageFormat::Iso639_1, cfg.language.list);
            f.optional(ctx, &name, codes);
        }

        let terms: Vec<String> = ctx
            .keywords
            .iter()
            .map(|k| k.trim())
            .filter(|k| !k.is_empty())
            .map(str::to_string)
            .collect();
        if let Some(name) = cfg.keywords_param.as_deref().map(list_name) {
            f.optional(ctx, &name, terms.clone());
        }
        if let Some(name) = &cfg.prompt_param {
            let own = ctx
                .prompt
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty());
            let folded =
                (cfg.keywords_param.is_none() && !terms.is_empty()).then(|| terms.join(", "));
            let prompt = match (own, folded) {
                (Some(p), Some(t)) => Some(format!("{p} {t}")),
                (Some(p), None) => Some(p.to_string()),
                (None, t) => t,
            };
            f.optional(ctx, name, prompt);
        }
        for (k, v) in &cfg.extra_fields {
            f.optional(ctx, k, [v.clone()]);
        }
        f
    }

    fn classify(&self, ex: &Exchanged, sent: &[String]) -> SegmentError {
        let failure = Failure {
            vendor: &self.cfg.adapter,
            exchanged: ex,
            sent_optional: sent,
            secret: self.cfg.auth.secret(),
        };
        let (mut err, body) = failure.classify();
        if let Some(infer) = &body.infer {
            // WaaV Infer: classify by the engine's code, never by the bare status, so a warming
            // or draining engine is a capacity answer and not an outage.
            match infer.code.as_str() {
                "admission_rejected" | "backpressure" | "model_not_ready" | "draining" => {
                    err.class = ErrorClass::RateLimited;
                    err.retry_after = err.retry_after.or(infer.retry_after);
                }
                "stall_timeout" | "internal" | "slow_consumer" => err.class = ErrorClass::Vendor,
                "not_implemented" | "payload_too_large" => err.class = ErrorClass::BadRequest,
                "unsupported_param" | "bad_config" | "unsupported_format" => {
                    err.class = ErrorClass::BadRequest;
                    err.refused_field =
                        refused_field(&body, &String::from_utf8_lossy(&ex.body), sent);
                }
                "model_not_found" => err.class = ErrorClass::ModelNotServed,
                "unauthorized" | "forbidden" => err.class = ErrorClass::Auth,
                _ => {}
            }
            return err;
        }
        match body.code_key().as_deref() {
            // Out of credit or over a spend cap: no retry helps, so it ends like a refused key.
            Some("insufficientquota" | "billinghardlimitreached") => err.class = ErrorClass::Auth,
            _ if ex.status == 498 => err.class = ErrorClass::RateLimited, // Groq: flex capacity
            _ => {}
        }
        err
    }

    fn parse(&self, ex: &Exchanged, format: &str) -> Result<SegmentTranscript, SegmentError> {
        let mut t = SegmentTranscript {
            vendor_request_id: request_id(
                &ex.headers,
                &["x-request-id", "apim-request-id", "x-ms-request-id"],
            ),
            ..Default::default()
        };
        let plain = |t: &mut SegmentTranscript| {
            t.text = String::from_utf8_lossy(&ex.body).trim().to_string()
        };
        if format == "text" && !ex.is_json() {
            plain(&mut t);
            return Ok(t);
        }
        let v: Value = match serde_json::from_slice(&ex.body) {
            Ok(v) => v,
            Err(_) if is_plain_text(ex) => {
                plain(&mut t);
                return Ok(t);
            }
            Err(e) => return Err(ex.not_a_transcript(&format!("the body is not JSON ({e})"))),
        };
        let Some(text) = v.get("text").and_then(Value::as_str) else {
            return Err(ex.not_a_transcript("it has no text field"));
        };
        t.text = text.trim().to_string();

        let token_mean = mean(
            v.get("logprobs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|l| l.get("logprob").and_then(Value::as_f64)),
        );
        if let Some(segments) = v.get("segments").and_then(Value::as_array) {
            let agg = aggregate_segments(segments);
            t.avg_logprob = agg.avg_logprob;
            t.no_speech_prob = agg.no_speech_prob;
            t.compression_ratio = agg.compression_ratio;
        }
        t.derived_confidence = token_mean
            .or(t.avg_logprob.map(f64::from))
            .map(|m| confidence_from_logprob(m as f32));

        t.detected_language = v
            .get("language")
            .and_then(Value::as_str)
            .and_then(detected_language)
            .or_else(|| {
                // gpt-transcribe: an empty list means the model was unsure.
                v.get("languages")
                    .and_then(Value::as_array)
                    .and_then(|l| l.first())
                    .and_then(|l| l.get("code").or(Some(l)))
                    .and_then(Value::as_str)
                    .and_then(detected_language)
            });
        if let Some(usage) = v
            .get("usage")
            .filter(|u| u.get("type").and_then(Value::as_str) == Some("duration"))
        {
            t.billed_ms = usage
                .get("seconds")
                .and_then(Value::as_f64)
                .map(|s| (s * 1000.0).round() as u32);
        }
        if t.vendor_request_id.is_none() {
            // Groq puts its id in the body.
            t.vendor_request_id = v
                .pointer("/x_groq/id")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        Ok(t)
    }
}

fn language_field(dialect: &LanguageDialect) -> Option<String> {
    let param = dialect
        .param
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())?;
    Some(if dialect.list {
        list_name(param)
    } else {
        param.to_string()
    })
}

fn is_plain_text(ex: &Exchanged) -> bool {
    ex.headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/plain"))
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values.fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    (n > 0).then(|| sum / n as f64)
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct SegmentSignals {
    pub avg_logprob: Option<f32>,
    pub no_speech_prob: Option<f32>,
    pub compression_ratio: Option<f32>,
}

/// Whisper's per-segment signals over a whole upload: `avg_logprob` weighted by duration (plain
/// mean when no segment has one), `no_speech_prob` the mean, `compression_ratio` the maximum, so
/// one looping segment is not averaged away. An upload of at most 25 s fits one 30 s window, so
/// the segments normally agree.
pub(crate) fn aggregate_segments(segments: &[Value]) -> SegmentSignals {
    let num = |s: &Value, k: &str| s.get(k).and_then(Value::as_f64).filter(|v| v.is_finite());
    let (mut weighted, mut weight, mut plain, mut n) = (0.0, 0.0, 0.0, 0usize);
    for s in segments {
        if let Some(lp) = num(s, "avg_logprob") {
            let dur = match (num(s, "start"), num(s, "end")) {
                (Some(a), Some(b)) if b > a => b - a,
                _ => 0.0,
            };
            weighted += lp * dur;
            weight += dur;
            plain += lp;
            n += 1;
        }
    }
    let avg_logprob = match (n, weight > 0.0) {
        (0, _) => None,
        (_, true) => Some((weighted / weight) as f32),
        (_, false) => Some((plain / n as f64) as f32),
    };
    SegmentSignals {
        avg_logprob,
        no_speech_prob: mean(segments.iter().filter_map(|s| num(s, "no_speech_prob")))
            .map(|v| v as f32),
        compression_ratio: segments
            .iter()
            .filter_map(|s| num(s, "compression_ratio"))
            .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
            .map(|v| v as f32),
    }
}

#[async_trait::async_trait]
impl SegmentTranscriber for OpenAiCompatTranscriber {
    fn info(&self) -> &TranscriberInfo {
        &self.info
    }

    async fn transcribe(
        &self,
        audio: &SegmentAudio,
        ctx: &SegmentContext,
        timeout: Duration,
        progress: &RequestProgress,
    ) -> Result<SegmentTranscript, SegmentError> {
        let wav = audio.wav();
        check_limits(&self.info, audio, wav.len())?;
        let fields = self.fields(ctx);
        let file = Part::bytes(wav)
            .file_name("segment.wav")
            .mime_str("audio/wav")
            .expect("audio/wav is a valid MIME type");
        let mut form = Form::new().part("file", file);
        for (k, v) in &fields.items {
            form = form.text(k.clone(), v.clone());
        }
        let req = self
            .cfg
            .auth
            .apply(self.cfg.client.post(&self.cfg.url).multipart(form))?;
        let ex = exchange(req, timeout, progress).await?;
        if !ex.is_success() {
            return Err(self.classify(&ex, &fields.sent_optional));
        }
        let format = if fields.sent_optional.iter().any(|f| f == "response_format") {
            self.cfg.response_format.as_deref().unwrap_or("json")
        } else {
            "json"
        };
        self.parse(&ex, format)
    }

    async fn prewarm(&self, connections: usize) {
        super::super::http::warm(&self.cfg.client, &self.cfg.url, connections).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::transcriber::wire::testkit::{
        self as kit, Harness, MockVendor, Reply, SECRET, contract_tests,
    };

    fn whisper_1(url: &str) -> OpenAiCompatConfig {
        OpenAiCompatConfig {
            adapter: "openai_transcriptions".into(),
            url: url.to_string(),
            auth: Auth::Bearer(SECRET.into()),
            model: "whisper-1".into(),
            language: LanguageDialect::single("language"),
            prompt_param: Some("prompt".into()),
            keywords_param: None,
            response_format: Some("verbose_json".into()),
            extra_fields: vec![],
            client: kit::client(),
            limits: RowLimits {
                max_upload_bytes: Some(26_214_400),
                ..Default::default()
            },
        }
    }

    fn gpt_transcribe(url: &str) -> OpenAiCompatConfig {
        OpenAiCompatConfig {
            model: "gpt-transcribe".into(),
            language: LanguageDialect::list("languages"),
            keywords_param: Some("keywords".into()),
            response_format: Some("json".into()),
            ..whisper_1(url)
        }
    }

    fn url(base: &str) -> String {
        format!("{base}/v1/audio/transcriptions")
    }

    fn build(cfg: OpenAiCompatConfig) -> Arc<dyn SegmentTranscriber> {
        Arc::new(OpenAiCompatTranscriber::new(cfg).unwrap())
    }

    fn ctx() -> SegmentContext {
        SegmentContext {
            language: Some("en-US".into()),
            keywords: vec!["Acme".into(), "Zed".into()],
            ..Default::default()
        }
    }

    const VERBOSE: &str = r#"{"task":"transcribe","language":"english","duration":3.0,"text":" Hello there. Booking please.","segments":[{"id":0,"start":0.0,"end":2.0,"text":" Hello there.","avg_logprob":-0.2,"compression_ratio":1.2,"no_speech_prob":0.1},{"id":1,"start":2.0,"end":3.0,"text":" Booking please.","avg_logprob":-0.5,"compression_ratio":1.6,"no_speech_prob":0.3}],"usage":{"type":"duration","seconds":3}}"#;

    fn harness() -> Harness {
        Harness {
            build: |base| build(whisper_1(&url(base))),
            ctx: ctx(),
            success: Reply::json(200, VERBOSE).header("x-request-id", "req_123"),
            unauthorized: Reply::json(
                401,
                &format!(r#"{{"error":{{"message":"Incorrect API key provided: {SECRET}. You can find your API key at https://platform.openai.com/account/api-keys.","type":"invalid_request_error","param":null,"code":"invalid_api_key"}}}}"#),
            ),
            model_missing: Reply::json(
                404,
                r#"{"error":{"message":"The model `whisper-9` does not exist or you do not have access to it.","type":"invalid_request_error","param":null,"code":"model_not_found"}}"#,
            ),
            rate_limited: Reply::json(
                429,
                r#"{"error":{"message":"Rate limit reached for whisper-1","type":"requests","code":"rate_limit_exceeded"}}"#,
            )
            .header("retry-after", "2"),
            unavailable: Reply::json(503, r#"{"error":{"message":"The server is overloaded or not ready yet.","type":"server_error"}}"#),
            refused: Reply::json(
                400,
                r#"{"error":{"message":"Invalid language 'en'. Language parameter must be specified in ISO-639-1 format.","type":"invalid_request_error","param":"language","code":"invalid_language_format"}}"#,
            ),
            refused_field: "language",
        }
    }

    contract_tests!(super::harness);

    #[tokio::test]
    async fn gpt_transcribe_sends_languages_and_keywords_never_language_or_prompt() {
        let vendor = MockVendor::start(
            Reply::json(
                200,
                r#"{"text":"I'd like to change my booking.","languages":[{"code":"en"}],"usage":{"type":"duration","seconds":3}}"#,
            )
            .header("x-request-id", "req_abc"),
        )
        .await;
        let t = build(gpt_transcribe(&url(&vendor.base)));
        let (out, _) = kit::run(t.as_ref(), &ctx()).await;
        let out = out.unwrap();

        let r = vendor.last();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/v1/audio/transcriptions");
        assert_eq!(
            r.header("authorization").as_deref(),
            Some(format!("Bearer {SECRET}").as_str())
        );
        let file = r.part("file").unwrap();
        assert_eq!(file.file_name.as_deref(), Some("segment.wav"));
        assert_eq!(file.content_type.as_deref(), Some("audio/wav"));
        assert_eq!(
            &file.data[..],
            &kit::audio().wav()[..],
            "the uploaded WAV is exactly the audio handed in"
        );
        assert_eq!(r.text("model").as_deref(), Some("gpt-transcribe"));
        assert_eq!(r.texts("languages[]"), vec!["en"]);
        assert_eq!(r.texts("keywords[]"), vec!["Acme", "Zed"]);
        assert_eq!(r.text("response_format").as_deref(), Some("json"));
        assert!(
            r.part("language").is_none(),
            "never the singular field beside languages[]"
        );
        assert!(
            r.part("prompt").is_none(),
            "key terms go to keywords[], not the prompt"
        );

        assert_eq!(out.text, "I'd like to change my booking.");
        assert_eq!(out.detected_language.as_deref(), Some("en"));
        assert_eq!(out.billed_ms, Some(3000));
        assert_eq!(out.vendor_request_id.as_deref(), Some("req_abc"));
        assert_eq!(out.vendor_confidence, None);
        assert_eq!(out.derived_confidence, None);
    }

    #[tokio::test]
    async fn a_list_dialect_sends_the_candidates_when_no_language_is_pinned() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"text":"hola","languages":[]}"#)).await;
        let t = build(gpt_transcribe(&url(&vendor.base)));
        let ctx = SegmentContext {
            candidate_languages: vec!["en-US".into(), "es".into(), "en-GB".into()],
            ..Default::default()
        };
        let out = kit::run(t.as_ref(), &ctx).await.0.unwrap();
        assert_eq!(vendor.last().texts("languages[]"), vec!["en", "es"]);
        assert_eq!(
            out.detected_language, None,
            "an empty list means the model was unsure"
        );

        // A single-field dialect never sends candidates.
        let t = build(whisper_1(&url(&vendor.base)));
        kit::run(t.as_ref(), &ctx).await.0.ok();
        assert!(vendor.last().part("language").is_none());
    }

    #[tokio::test]
    async fn whisper_1_sends_language_prompt_and_verbose_json_and_reads_segment_signals() {
        let vendor = MockVendor::start(Reply::json(200, VERBOSE)).await;
        let t = build(whisper_1(&url(&vendor.base)));
        let ctx = SegmentContext {
            prompt: Some("Booking call.".into()),
            ..ctx()
        };
        let out = kit::run(t.as_ref(), &ctx).await.0.unwrap();

        let r = vendor.last();
        assert_eq!(r.text("model").as_deref(), Some("whisper-1"));
        assert_eq!(r.text("language").as_deref(), Some("en"));
        assert_eq!(r.text("prompt").as_deref(), Some("Booking call. Acme, Zed"));
        assert_eq!(r.text("response_format").as_deref(), Some("verbose_json"));
        assert!(r.part("languages[]").is_none() && r.part("keywords[]").is_none());

        assert_eq!(out.text, "Hello there. Booking please.");
        assert_eq!(out.detected_language.as_deref(), Some("en"));
        // Duration-weighted: (-0.2 * 2 + -0.5 * 1) / 3 = -0.3.
        assert!((out.avg_logprob.unwrap() + 0.3).abs() < 1e-6);
        assert!((out.no_speech_prob.unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(out.compression_ratio, Some(1.6));
        assert!((out.derived_confidence.unwrap() - (-0.3f32).exp()).abs() < 1e-6);
        assert_eq!(
            out.vendor_confidence, None,
            "a log-probability is not a vendor confidence"
        );
    }

    #[tokio::test]
    async fn gpt_4o_token_logprobs_become_a_derived_confidence() {
        let vendor = MockVendor::start(Reply::json(
            200,
            r#"{"text":"Hi.","logprobs":[{"token":"Hi","logprob":-0.1,"bytes":[72,105]},{"token":".","logprob":-0.3,"bytes":[46]}],"usage":{"type":"tokens","input_tokens":14,"output_tokens":3,"total_tokens":17}}"#,
        ))
        .await;
        let cfg = OpenAiCompatConfig {
            model: "gpt-4o-mini-transcribe".into(),
            response_format: Some("json".into()),
            extra_fields: vec![("include[]".into(), "logprobs".into())],
            ..whisper_1(&url(&vendor.base))
        };
        let out = kit::run(build(cfg).as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(vendor.last().texts("include[]"), vec!["logprobs"]);
        assert!((out.derived_confidence.unwrap() - (-0.2f32).exp()).abs() < 1e-6);
        assert_eq!(
            out.avg_logprob, None,
            "token log-probabilities are not on Whisper's scale"
        );
        assert_eq!(out.billed_ms, None, "tokens are not milliseconds");
    }

    #[tokio::test]
    async fn the_text_format_is_read_as_plain_text() {
        let vendor = MockVendor::start(Reply::text(200, " plain words \n")).await;
        let cfg = OpenAiCompatConfig {
            response_format: Some("text".into()),
            ..whisper_1(&url(&vendor.base))
        };
        let out = kit::run(build(cfg).as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(out.text, "plain words");
    }

    #[tokio::test]
    async fn azure_openai_sends_api_key_and_never_authorization() {
        let vendor = MockVendor::start(
            Reply::json(200, r#"{"text":"ok"}"#).header("apim-request-id", "apim-1"),
        )
        .await;
        let cfg = OpenAiCompatConfig {
            adapter: "azure_openai_transcriptions".into(),
            url: format!(
                "{}/openai/deployments/my-whisper/audio/transcriptions?api-version=2024-02-01",
                vendor.base
            ),
            auth: Auth::AzureApiKey(SECRET.into()),
            model: "my-whisper".into(),
            response_format: Some("json".into()),
            ..whisper_1("http://unused")
        };
        let out = kit::run(build(cfg).as_ref(), &ctx()).await.0.unwrap();
        let r = vendor.last();
        assert_eq!(
            r.path,
            "/openai/deployments/my-whisper/audio/transcriptions"
        );
        assert_eq!(r.query_values("api-version"), vec!["2024-02-01"]);
        assert_eq!(r.header("api-key").as_deref(), Some(SECRET));
        assert!(r.header("authorization").is_none());
        assert_eq!(r.text("model").as_deref(), Some("my-whisper"));
        assert_eq!(out.vendor_request_id.as_deref(), Some("apim-1"));
    }

    #[tokio::test]
    async fn a_keyless_self_hosted_server_gets_no_authorization_header() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"text":"ok"}"#)).await;
        let cfg = OpenAiCompatConfig {
            url: format!("{}/v1/audio/transcriptions", vendor.base),
            auth: Auth::Bearer(String::new()),
            model: "Systran/faster-whisper-small".into(),
            prompt_param: None,
            response_format: Some("json".into()),
            limits: RowLimits {
                single_process_server: true,
                ..Default::default()
            },
            ..whisper_1("http://unused")
        };
        let t = build(cfg);
        assert!(t.info().single_process_server);
        assert!(!t.info().allows_second_request());
        kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        let r = vendor.last();
        assert!(r.header("authorization").is_none());
        assert_eq!(r.text("language").as_deref(), Some("en"));
    }

    #[tokio::test]
    async fn waav_infer_gets_no_language_and_no_prompt() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"text":"ok"}"#)).await;
        let cfg = OpenAiCompatConfig {
            language: LanguageDialect::none(),
            prompt_param: None,
            response_format: Some("json".into()),
            ..whisper_1(&url(&vendor.base))
        };
        kit::run(build(cfg).as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(
            vendor.last().part_names(),
            vec!["file", "model", "response_format"]
        );
    }

    #[tokio::test]
    async fn minimal_sends_only_the_file_and_the_model() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"text":"ok"}"#)).await;
        let cfg = OpenAiCompatConfig {
            extra_fields: vec![("include[]".into(), "logprobs".into())],
            ..gpt_transcribe(&url(&vendor.base))
        };
        let ctx = SegmentContext {
            minimal: true,
            prompt: Some("p".into()),
            ..ctx()
        };
        let out = kit::run(build(cfg).as_ref(), &ctx).await.0.unwrap();
        assert_eq!(vendor.last().part_names(), vec!["file", "model"]);
        assert_eq!(out.text, "ok");
    }

    #[tokio::test]
    async fn an_omitted_field_is_left_out_and_the_rest_still_go() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"text":"ok"}"#)).await;
        let t = build(gpt_transcribe(&url(&vendor.base)));
        let ctx = SegmentContext {
            omit_fields: vec!["languages".into()],
            ..ctx()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        let r = vendor.last();
        assert!(r.part("languages[]").is_none());
        assert_eq!(r.texts("keywords[]"), vec!["Acme", "Zed"]);

        // A refused keywords field does not move the terms into the prompt.
        let ctx = SegmentContext {
            omit_fields: vec!["keywords[]".into()],
            ..super::tests::ctx()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        let r = vendor.last();
        assert!(r.part("keywords[]").is_none() && r.part("prompt").is_none());
        assert_eq!(r.texts("languages[]"), vec!["en"]);
    }

    #[tokio::test]
    async fn an_empty_or_auto_language_is_never_sent() {
        let vendor = MockVendor::start(Reply::json(200, VERBOSE)).await;
        let t = build(whisper_1(&url(&vendor.base)));
        for lang in ["", "  ", "auto"] {
            let ctx = SegmentContext {
                language: Some(lang.into()),
                ..Default::default()
            };
            kit::run(t.as_ref(), &ctx).await.0.unwrap();
            assert!(vendor.last().part("language").is_none(), "{lang:?}");
        }
    }

    #[tokio::test]
    async fn groq_reads_its_request_id_and_reset_header() {
        let vendor = MockVendor::start(Reply::json(
            200,
            r#"{"text":"ok","x_groq":{"id":"req_01groq"}}"#,
        ))
        .await;
        let cfg = OpenAiCompatConfig {
            adapter: "groq_transcriptions".into(),
            model: "whisper-large-v3-turbo".into(),
            extra_fields: vec![("timestamp_granularities[]".into(), "segment".into())],
            ..whisper_1(&format!("{}/openai/v1/audio/transcriptions", vendor.base))
        };
        let t = build(cfg);
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(out.vendor_request_id.as_deref(), Some("req_01groq"));
        assert_eq!(
            vendor.last().texts("timestamp_granularities[]"),
            vec!["segment"]
        );

        vendor.push(
            Reply::json(
                429,
                r#"{"error":{"message":"Rate limit reached","type":"requests"}}"#,
            )
            .header("x-ratelimit-reset-requests", "2m59.56s"),
        );
        let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
        assert_eq!(err.class, ErrorClass::RateLimited);
        assert_eq!(err.retry_after, Some(Duration::from_secs_f64(179.56)));

        vendor.push(Reply::json(
            498,
            r#"{"error":{"message":"Flex Tier Capacity Exceeded","type":"capacity"}}"#,
        ));
        let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
        assert_eq!(
            err.class,
            ErrorClass::RateLimited,
            "a capacity answer, not a bad request"
        );
        assert!(!err.counts_for_breaker());
    }

    #[tokio::test]
    async fn waav_infer_rejections_are_classified_by_code_not_status() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"text":"ok"}"#)).await;
        let t = build(whisper_1(&url(&vendor.base)));
        let infer = |status: u16, code: &str, msg: &str| {
            Reply::json(
                status,
                &format!(
                    r#"{{"error":{{"code":"{code}","message":"{msg}","retriable":true,"retry_after_ms":250}}}}"#
                ),
            )
        };
        let cases: Vec<(Reply, ErrorClass, bool)> = vec![
            (
                infer(429, "admission_rejected", "queue full"),
                ErrorClass::RateLimited,
                false,
            ),
            (
                infer(429, "backpressure", "slow down"),
                ErrorClass::RateLimited,
                false,
            ),
            (
                infer(503, "model_not_ready", "warming"),
                ErrorClass::RateLimited,
                false,
            ),
            (
                infer(503, "draining", "going away"),
                ErrorClass::RateLimited,
                false,
            ),
            (
                infer(503, "stall_timeout", "stalled"),
                ErrorClass::Vendor,
                true,
            ),
            (
                infer(501, "not_implemented", "no"),
                ErrorClass::BadRequest,
                false,
            ),
            (
                infer(404, "model_not_found", "none"),
                ErrorClass::ModelNotServed,
                false,
            ),
        ];
        for (reply, class, breaker) in cases {
            vendor.push(reply);
            let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
            assert_eq!(err.class, class, "{err}");
            assert_eq!(err.counts_for_breaker(), breaker, "{err}");
            if class == ErrorClass::RateLimited {
                assert_eq!(err.retry_after, Some(Duration::from_millis(250)));
            }
        }
        vendor.push(infer(
            400,
            "unsupported_param",
            "transcriptions field 'prompt' is not supported",
        ));
        let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
        assert_eq!(err.refused_field.as_deref(), Some("prompt"));
    }

    #[tokio::test]
    async fn an_exhausted_quota_is_final() {
        let vendor = MockVendor::start(Reply::json(
            429,
            r#"{"error":{"message":"You exceeded your current quota.","type":"insufficient_quota","code":"insufficient_quota"}}"#,
        ))
        .await;
        let err = kit::run(build(whisper_1(&url(&vendor.base))).as_ref(), &ctx())
            .await
            .0
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::Auth);
        assert!(!err.is_fast_retryable());
    }

    #[tokio::test]
    async fn a_success_without_text_is_a_protocol_error() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"segments":[]}"#)).await;
        let err = kit::run(build(whisper_1(&url(&vendor.base))).as_ref(), &ctx())
            .await
            .0
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::Protocol);
        assert!(!err.is_fast_retryable(), "the vendor did the work");
        vendor.set_default(Reply::json(200, "<html>"));
        let err = kit::run(build(whisper_1(&url(&vendor.base))).as_ref(), &ctx())
            .await
            .0
            .unwrap_err();
        assert_eq!(err.class, ErrorClass::Protocol);
    }

    #[tokio::test]
    async fn a_unit_over_the_row_limits_is_refused_before_sending() {
        let vendor = MockVendor::start(Reply::json(200, r#"{"text":"ok"}"#)).await;
        let cfg = OpenAiCompatConfig {
            limits: RowLimits {
                max_audio_ms: Some(400),
                ..Default::default()
            },
            ..whisper_1(&url(&vendor.base))
        };
        let (out, progress) = kit::run(build(cfg).as_ref(), &ctx()).await;
        let err = out.unwrap_err();
        assert_eq!(err.class, ErrorClass::BadRequest);
        assert_eq!(err.phase, crate::transcriber::RequestPhase::BeforeSend);
        assert!(vendor.requests().is_empty());
        assert!(!progress.headers_received());
    }

    #[test]
    fn info_declares_every_optional_field_it_sends() {
        let t = OpenAiCompatTranscriber::new(gpt_transcribe(
            "https://api.openai.com/v1/audio/transcriptions",
        ))
        .unwrap();
        let info = t.info();
        kit::info_is_a_file_target(&t, "openai_transcriptions", "https://api.openai.com");
        assert_eq!(info.model, "gpt-transcribe");
        assert_eq!(info.host_key, "https://api.openai.com:443");
        assert_eq!(
            info.droppable_fields,
            vec!["languages[]", "prompt", "keywords[]", "response_format"]
        );
        assert_eq!(info.max_upload_bytes, Some(26_214_400));
        let groq = OpenAiCompatTranscriber::new(OpenAiCompatConfig {
            extra_fields: vec![("timestamp_granularities[]".into(), "segment".into())],
            ..whisper_1("https://api.groq.com/openai/v1/audio/transcriptions")
        })
        .unwrap();
        assert_eq!(
            groq.info().droppable_fields,
            vec![
                "language",
                "prompt",
                "response_format",
                "timestamp_granularities[]"
            ]
        );
    }

    #[test]
    fn the_config_is_checked_at_plan_time() {
        let bad_url = OpenAiCompatConfig {
            url: "api.openai.com/v1".into(),
            ..whisper_1("x")
        };
        assert!(OpenAiCompatTranscriber::new(bad_url).is_err());
        let injected = OpenAiCompatConfig {
            extra_fields: vec![("stream".into(), "true".into())],
            ..whisper_1("https://h/x")
        };
        assert!(
            OpenAiCompatTranscriber::new(injected)
                .unwrap_err()
                .contains("stream")
        );
        let srt = OpenAiCompatConfig {
            response_format: Some("srt".into()),
            ..whisper_1("https://h/x")
        };
        assert!(OpenAiCompatTranscriber::new(srt).is_err());
        let no_model = OpenAiCompatConfig {
            model: " ".into(),
            ..whisper_1("https://h/x")
        };
        assert!(OpenAiCompatTranscriber::new(no_model).is_err());
    }

    #[tokio::test]
    async fn prewarm_opens_connections_to_the_origin_without_the_key() {
        let vendor = MockVendor::start(
            Reply::json(200, r#"{"text":"ok"}"#).delayed(Duration::from_millis(50)),
        )
        .await;
        let t = build(whisper_1(&url(&vendor.base)));
        t.prewarm(2).await;
        let reqs = vendor.requests();
        assert_eq!(reqs.len(), 2);
        assert!(reqs.iter().all(|r| r.method == "HEAD" && r.path == "/"));
        assert!(reqs.iter().all(|r| r.header("authorization").is_none()));

        // Two uploads in flight together find the two warm connections: neither pays a handshake.
        let warmed: std::collections::HashSet<_> = reqs.iter().map(|r| r.peer).collect();
        assert_eq!(warmed.len(), 2);
        let c = ctx();
        let (a, b) = tokio::join!(kit::run(t.as_ref(), &c), kit::run(t.as_ref(), &c));
        a.0.unwrap();
        b.0.unwrap();
        let uploads: std::collections::HashSet<_> =
            vendor.requests()[2..].iter().map(|r| r.peer).collect();
        assert!(
            uploads.is_subset(&warmed),
            "the first upload after prewarm is not first on its connection"
        );
    }

    #[test]
    fn segment_signals_aggregate_as_designed() {
        let segs: Vec<Value> = serde_json::from_str(
            r#"[{"avg_logprob":-1.0,"no_speech_prob":0.9,"compression_ratio":3.0},{"avg_logprob":-0.2,"no_speech_prob":0.1,"compression_ratio":1.0}]"#,
        )
        .unwrap();
        let s = aggregate_segments(&segs);
        assert!(
            (s.avg_logprob.unwrap() + 0.6).abs() < 1e-6,
            "plain mean without timings"
        );
        assert!((s.no_speech_prob.unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(s.compression_ratio, Some(3.0), "the worst segment");
        assert_eq!(aggregate_segments(&[]), SegmentSignals::default());
    }
}
