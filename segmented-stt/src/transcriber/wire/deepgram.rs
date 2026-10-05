//! Deepgram prerecorded (`deepgram_prerecorded`): `POST {base}/v1/listen?<query>` with the WAV as
//! the raw body.
//!
//! Required for the hosted Whisper ids (`whisper`, `whisper-tiny` … `whisper-large`), which cannot
//! stream; usable for any other Deepgram model. For `whisper*` the request carries `model`, a bare
//! language code, `punctuate` and `smart_format`, and never key terms: Whisper Cloud lists
//! Keywords as unsupported, and whether it ignores or rejects them is unverified, so they are not
//! sent even when a row asks. `keyterm` belongs to Nova-3, `keywords` to Nova-2 and older.
//!
//! With no session language the request asks for detection (`detect_language=true`, which works
//! on prerecorded audio including Whisper); without it Deepgram assumes English.

use crate::vendor::retention;
use std::time::Duration;

use super::{
    Auth, Exchanged, Failure, Fields, LanguageFormat, RowLimits, check_limits, detected_language,
    exchange, info_for, join_url, may_send, request_id, session_language, wire_language,
};
use crate::transcriber::{
    RequestProgress, SegmentAudio, SegmentContext, SegmentError, SegmentTranscriber,
    SegmentTranscript, TranscriberInfo,
};
use crate::types::ErrorClass;

pub const DEFAULT_BASE_URL: &str = crate::vendor::hosts::DEEPGRAM;

#[derive(Debug, Clone)]
pub struct DeepgramPrerecordedConfig {
    /// `https://api.deepgram.com`. Whisper Cloud is not served on the EU, AU or IN hosts; the
    /// resolver refuses those, not this adapter.
    pub base_url: String,
    /// [`Auth::deepgram`] for an API key, [`Auth::Bearer`] for a JWT.
    pub auth: Auth,
    pub model: String,
    /// Whisper takes a bare code; Nova takes BCP-47.
    pub language_format: LanguageFormat,
    /// `keyterm` (Nova-3), `keywords` (Nova-2 and older), or `None`.
    pub keyterms_param: Option<String>,
    /// `mip_opt_out=true`: the audio is not kept for Deepgram's model improvement programme (the
    /// canonical `data_retention: none`, Release 5). Not sent by default.
    pub mip_opt_out: bool,
    pub client: reqwest::Client,
    pub limits: RowLimits,
}

impl DeepgramPrerecordedConfig {
    /// The defaults the vendor documents for the model's family.
    pub fn for_model(base_url: &str, auth: Auth, model: &str, client: reqwest::Client) -> Self {
        let lower = model.trim().to_ascii_lowercase();
        let (language_format, keyterms_param) = if is_whisper(&lower) {
            (LanguageFormat::Iso639_1, None)
        } else if lower.starts_with("nova-3") {
            (LanguageFormat::Bcp47, Some("keyterm".to_string()))
        } else {
            (LanguageFormat::Bcp47, Some("keywords".to_string()))
        };
        Self {
            base_url: base_url.to_string(),
            auth,
            model: model.to_string(),
            language_format,
            keyterms_param,
            mip_opt_out: false,
            client,
            limits: RowLimits::default(),
        }
    }
}

fn is_whisper(model: &str) -> bool {
    model.trim().to_ascii_lowercase().starts_with("whisper")
}

#[derive(Debug)]
pub struct DeepgramPrerecordedTranscriber {
    cfg: DeepgramPrerecordedConfig,
    url: String,
    info: TranscriberInfo,
}

impl DeepgramPrerecordedTranscriber {
    pub fn new(mut cfg: DeepgramPrerecordedConfig) -> Result<Self, String> {
        if cfg.model.trim().is_empty() {
            // An omitted model is `base-general`, a legacy model nobody chose.
            return Err("a Deepgram transcriber needs an explicit model".into());
        }
        if is_whisper(&cfg.model) {
            cfg.keyterms_param = None;
        }
        let url = join_url(&cfg.base_url, "/v1/listen")?;
        let mut droppable: Vec<String> =
            ["language", "detect_language", "punctuate", "smart_format"]
                .map(String::from)
                .into();
        droppable.extend(cfg.keyterms_param.clone());
        let info = info_for(
            "deepgram_prerecorded",
            &url,
            &cfg.model,
            cfg.limits,
            droppable,
        );
        Ok(Self { cfg, url, info })
    }

    fn query(&self, ctx: &SegmentContext) -> Fields {
        let mut f = Fields::default();
        f.required("model", self.cfg.model.trim());
        let lang = session_language(ctx.language.as_deref())
            .and_then(|l| wire_language(&l, self.cfg.language_format));
        let sent_language = match lang {
            Some(l) if may_send(ctx, "language") => f.optional(ctx, "language", [l]),
            _ => false,
        };
        if !sent_language {
            f.optional(ctx, "detect_language", ["true"]);
        }
        f.optional(ctx, "punctuate", ["true"]);
        f.optional(ctx, "smart_format", ["true"]);
        if self.cfg.mip_opt_out {
            f.required(retention::DEEPGRAM.0, retention::DEEPGRAM.1);
        }
        if let Some(name) = &self.cfg.keyterms_param {
            let terms = ctx
                .keywords
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .map(str::to_string);
            f.optional(ctx, name, terms.collect::<Vec<_>>());
        }
        f
    }

    fn classify(&self, ex: &Exchanged, sent: &[String]) -> SegmentError {
        let failure = Failure {
            vendor: "deepgram",
            exchanged: ex,
            sent_optional: sent,
            secret: self.cfg.auth.secret(),
        };
        let (mut err, body) = failure.classify();
        // "No such model/language/tier combination found." names both. With a language sent the
        // language is the field to drop first; without one the model is the problem.
        let no_such = body
            .message
            .as_deref()
            .is_some_and(|m| m.to_ascii_lowercase().contains("no such model"));
        if no_such && err.class == ErrorClass::BadRequest {
            if sent.iter().any(|s| s == "language") {
                err.refused_field = Some("language".into());
            } else {
                err.class = ErrorClass::ModelNotServed;
                err.refused_field = None;
            }
        }
        err
    }

    fn parse(&self, ex: &Exchanged) -> Result<SegmentTranscript, SegmentError> {
        let v = ex.json()?;
        let missing = || ex.not_a_transcript("it has no results.channels[0].alternatives[0]");
        let p = crate::vendor::deepgram::parse_prerecorded(&v).map_err(|_| missing())?;
        let channel = p.channels.first();
        let alt = channel
            .and_then(|c| c.alternatives.first())
            .ok_or_else(missing)?;
        Ok(SegmentTranscript {
            text: alt.transcript.clone(),
            vendor_confidence: alt.confidence.map(|c| c as f32),
            detected_language: channel
                .and_then(|c| c.detected_language.as_deref())
                .and_then(detected_language),
            vendor_request_id: p
                .request_id
                .clone()
                .or_else(|| request_id(&ex.headers, &["dg-request-id", "x-request-id"])),
            // Deepgram bills exactly the audio duration, per second, with no minimum.
            billed_ms: p.duration_secs.map(|s| (s * 1000.0).round() as u32),
            ..Default::default()
        })
    }
}

#[async_trait::async_trait]
impl SegmentTranscriber for DeepgramPrerecordedTranscriber {
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
        let query = self.query(ctx);
        let req = self
            .cfg
            .client
            .post(&self.url)
            .query(&query.items)
            .header(reqwest::header::CONTENT_TYPE, "audio/wav")
            .body(wav);
        let req = self.cfg.auth.apply(req)?;
        let ex = exchange(req, timeout, progress).await?;
        if !ex.is_success() {
            return Err(self.classify(&ex, &query.sent_optional));
        }
        self.parse(&ex)
    }

    async fn prewarm(&self, connections: usize) {
        super::super::http::warm(&self.cfg.client, &self.url, connections).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::transcriber::wire::testkit::{
        self as kit, Harness, MockVendor, Reply, SECRET, contract_tests,
    };

    fn config(base: &str, model: &str) -> DeepgramPrerecordedConfig {
        DeepgramPrerecordedConfig::for_model(base, Auth::deepgram(SECRET), model, kit::client())
    }

    fn build(cfg: DeepgramPrerecordedConfig) -> Arc<dyn SegmentTranscriber> {
        Arc::new(DeepgramPrerecordedTranscriber::new(cfg).unwrap())
    }

    fn ctx() -> SegmentContext {
        SegmentContext {
            language: Some("en-US".into()),
            keywords: vec!["Acme".into(), "Zed".into()],
            ..Default::default()
        }
    }

    const SUCCESS: &str = r#"{"metadata":{"request_id":"dg-req-1","duration":1.25,"channels":1},"results":{"channels":[{"alternatives":[{"transcript":"hello world","confidence":0.93,"words":[{"word":"hello","start":0.1,"end":0.4,"confidence":0.95},{"word":"world","start":0.5,"end":0.9,"confidence":0.91}]}],"detected_language":"en"}]}}"#;

    fn harness() -> Harness {
        Harness {
            build: |base| build(config(base, "whisper-large")),
            ctx: ctx(),
            success: Reply::json(200, SUCCESS),
            unauthorized: Reply::json(
                401,
                r#"{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials.","request_id":"r1"}"#,
            ),
            model_missing: Reply::json(
                400,
                r#"{"err_code":"Bad Request","err_msg":"No such model/language/tier combination found.","request_id":"r2"}"#,
            ),
            rate_limited: Reply::json(
                429,
                r#"{"err_code":"TOO_MANY_REQUESTS","err_msg":"Too many requests. Please try again later.","request_id":"r3"}"#,
            )
            .header("retry-after", "2"),
            unavailable: Reply::json(503, r#"{"err_code":"SERVICE_UNAVAILABLE","err_msg":"Try again.","request_id":"r4"}"#),
            refused: Reply::json(
                400,
                r#"{"err_code":"Bad Request","err_msg":"The punctuate option is not supported for this model.","request_id":"r5"}"#,
            ),
            refused_field: "punctuate",
        }
    }

    contract_tests!(super::harness);

    #[tokio::test]
    async fn no_retention_opts_out_of_model_improvement() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let mut cfg = config(&vendor.base, "nova-3");
        cfg.mip_opt_out = true;
        kit::run(build(cfg).as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(vendor.last().query_values("mip_opt_out"), vec!["true"]);
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        kit::run(build(config(&vendor.base, "nova-3")).as_ref(), &ctx())
            .await
            .0
            .unwrap();
        assert!(
            vendor.last().query_values("mip_opt_out").is_empty(),
            "not sent by default"
        );
    }

    #[tokio::test]
    async fn whisper_gets_a_bare_language_and_never_key_terms() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let mut cfg = config(&vendor.base, "whisper-large");
        cfg.keyterms_param = Some("keyterm".into()); // a row asking for them anyway
        let t = build(cfg);
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();

        let r = vendor.last();
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/v1/listen");
        assert_eq!(
            r.header("authorization").as_deref(),
            Some(format!("Token {SECRET}").as_str())
        );
        assert_eq!(r.header("content-type").as_deref(), Some("audio/wav"));
        assert_eq!(&r.body[..], &kit::audio().wav()[..]);
        assert_eq!(
            r.query_names(),
            vec!["model", "language", "punctuate", "smart_format"]
        );
        assert_eq!(r.query_values("model"), vec!["whisper-large"]);
        assert_eq!(r.query_values("language"), vec!["en"]);
        assert_eq!(r.query_values("punctuate"), vec!["true"]);
        assert_eq!(r.query_values("smart_format"), vec!["true"]);

        assert_eq!(out.text, "hello world");
        assert_eq!(out.vendor_confidence, Some(0.93));
        assert_eq!(out.derived_confidence, None);
        assert_eq!(out.detected_language.as_deref(), Some("en"));
        assert_eq!(out.vendor_request_id.as_deref(), Some("dg-req-1"));
        assert_eq!(out.billed_ms, Some(1250));
        assert!(
            !t.info()
                .droppable_fields
                .iter()
                .any(|f| f.starts_with("key"))
        );
    }

    #[tokio::test]
    async fn nova_3_gets_bcp47_and_keyterm() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        kit::run(build(config(&vendor.base, "nova-3")).as_ref(), &ctx())
            .await
            .0
            .unwrap();
        let r = vendor.last();
        assert_eq!(r.query_values("language"), vec!["en-US"]);
        assert_eq!(r.query_values("keyterm"), vec!["Acme", "Zed"]);

        kit::run(
            build(config(&vendor.base, "nova-2-phonecall")).as_ref(),
            &ctx(),
        )
        .await
        .0
        .unwrap();
        assert_eq!(vendor.last().query_values("keywords"), vec!["Acme", "Zed"]);
    }

    #[tokio::test]
    async fn without_a_language_detection_is_asked_for() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "whisper"));
        kit::run(t.as_ref(), &SegmentContext::default())
            .await
            .0
            .unwrap();
        let r = vendor.last();
        assert!(r.query_values("language").is_empty());
        assert_eq!(r.query_values("detect_language"), vec!["true"]);

        // A refused language falls back to detection, not to Deepgram's English default.
        let ctx = SegmentContext {
            omit_fields: vec!["language".into()],
            ..ctx()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        let r = vendor.last();
        assert!(r.query_values("language").is_empty());
        assert_eq!(r.query_values("detect_language"), vec!["true"]);
    }

    #[tokio::test]
    async fn minimal_sends_only_the_model() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "nova-3"));
        let ctx = SegmentContext {
            minimal: true,
            ..ctx()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        let r = vendor.last();
        assert_eq!(r.query_names(), vec!["model"]);
        assert_eq!(&r.body[..], &kit::audio().wav()[..]);
    }

    #[tokio::test]
    async fn an_omitted_field_is_left_out() {
        let vendor = MockVendor::start(Reply::json(200, SUCCESS)).await;
        let t = build(config(&vendor.base, "whisper-large"));
        let ctx = SegmentContext {
            omit_fields: vec!["punctuate".into()],
            ..ctx()
        };
        kit::run(t.as_ref(), &ctx).await.0.unwrap();
        assert_eq!(
            vendor.last().query_names(),
            vec!["model", "language", "smart_format"]
        );
    }

    #[tokio::test]
    async fn no_such_model_names_the_language_first_when_one_was_sent() {
        let vendor = MockVendor::start(Reply::json(
            400,
            r#"{"err_code":"Bad Request","err_msg":"No such model/language/tier combination found."}"#,
        ))
        .await;
        let t = build(config(&vendor.base, "whisper-large"));
        let err = kit::run(t.as_ref(), &ctx()).await.0.unwrap_err();
        assert_eq!(err.class, ErrorClass::BadRequest);
        assert_eq!(err.refused_field.as_deref(), Some("language"));
    }

    #[tokio::test]
    async fn an_empty_transcript_is_text_not_an_error_and_payment_is_auth() {
        let vendor = MockVendor::start(Reply::json(
            200,
            r#"{"metadata":{"request_id":"r","duration":0.5},"results":{"channels":[{"alternatives":[{"transcript":"","confidence":0.0,"words":[]}]}]}}"#,
        ))
        .await;
        let t = build(config(&vendor.base, "whisper"));
        let out = kit::run(t.as_ref(), &ctx()).await.0.unwrap();
        assert_eq!(out.text, "");
        assert!(!out.vendor_said_no_speech);

        vendor.set_default(Reply::json(
            402,
            r#"{"err_code":"PAYMENT_REQUIRED","err_msg":"Insufficient balance."}"#,
        ));
        assert_eq!(
            kit::run(t.as_ref(), &ctx()).await.0.unwrap_err().class,
            ErrorClass::Auth
        );
        vendor.set_default(Reply::json(200, r#"{"metadata":{}}"#));
        assert_eq!(
            kit::run(t.as_ref(), &ctx()).await.0.unwrap_err().class,
            ErrorClass::Protocol
        );
    }

    #[test]
    fn info_and_config_checks() {
        let t = DeepgramPrerecordedTranscriber::new(config(DEFAULT_BASE_URL, "nova-3")).unwrap();
        kit::info_is_a_file_target(&t, "deepgram_prerecorded", "https://api.deepgram.com");
        assert_eq!(
            t.info().droppable_fields,
            vec![
                "language",
                "detect_language",
                "punctuate",
                "smart_format",
                "keyterm"
            ]
        );
        assert!(DeepgramPrerecordedTranscriber::new(config(DEFAULT_BASE_URL, "")).is_err());
        let jwt = DeepgramPrerecordedConfig {
            auth: Auth::Bearer("jwt".into()),
            ..config(DEFAULT_BASE_URL, "nova-3")
        };
        assert!(DeepgramPrerecordedTranscriber::new(jwt).is_ok());
    }
}
